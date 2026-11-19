//! PDFs, by PDFium on a thread of their own.
//!
//! PDFium is loaded at run time from beside the program, or from the libraries' folder beside its
//! own (a Mac bundle's Frameworks, an installed release's lib), or `SPATILOUPE_PDFIUM`, or the copy
//! `tools/fetch-pdfium.sh` put in the tree, for a build being worked on. One thread holds it
//! and the open document, and is asked for pages at a width, for the words on them, and to
//! find text; it answers on a channel the viewer looks at between pictures. A document locked
//! with a password says so, and is given one to try (`Ask::Unlock`); the thread alone keeps it,
//! while the document is open, to open the file again when markup is written into it.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender};

use pdfium_render::prelude::*;
use zeroize::Zeroizing;

/// A document's password: never written anywhere, logged or said, held only while the document
/// it opens is open (its thread has it, to open the file again when markup is written into it),
/// and wiped from memory when let go of.
pub struct Secret(Zeroizing<String>);

impl Secret {
    pub fn new(text: String) -> Secret {
        Secret(Zeroizing::new(text))
    }

    pub(crate) fn expose(&self) -> &str {
        &self.0
    }

    /// The same password, for the same document opened again: after it is saved, or its copy.
    pub fn again(&self) -> Secret {
        Secret::new(self.0.to_string())
    }
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Secret")
    }
}

/// What the document's owner allows done to it, as its security says -- all of it, unless it is
/// encrypted with permissions, and opened with the password to open it rather than the owner's.
/// What markup offers, as Preview does: annotating at all, and a crop (changing the pages).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rights {
    pub encrypted: bool,
    /// Notes may be added: the permissions' bit 6.
    pub annotate: bool,
    /// The pages may be changed: bit 4. A crop box changes a page.
    pub modify: bool,
}

impl Rights {
    pub const ALL: Rights = Rights { encrypted: false, annotate: true, modify: true };

    /// From PDFium's security handler revision (-1, none) and the permissions it gives (all of them
    /// for a document opened with its owner's password).
    pub fn of(revision: i32, permissions: u32) -> Rights {
        if revision < 0 {
            return Rights::ALL;
        }
        Rights { encrypted: true, annotate: permissions & (1 << 5) != 0, modify: permissions & (1 << 3) != 0 }
    }
}

/// The rights over the document at `path` (or of `bytes`, a remote's), opened with `password`:
/// PDFium's own numbers, which pdfium-render does not give for AES-256 (revision 6).
pub(crate) fn rights(pdfium: &Pdfium, path: &Path, bytes: Option<&[u8]>, password: Option<&str>) -> Rights {
    let b = pdfium.bindings();
    let doc = match (bytes, path.to_str()) {
        (Some(bytes), _) => unsafe { b.FPDF_LoadMemDocument64(bytes, password) },
        (None, Some(name)) => unsafe { b.FPDF_LoadDocument(name, password) },
        (None, None) => match std::fs::read(path) {
            Ok(bytes) => return rights(pdfium, path, Some(&bytes), password),
            Err(_) => std::ptr::null_mut(),
        },
    };
    if doc.is_null() {
        // Opened by pdfium-render a moment ago: allowed nothing rather than guessed.
        return Rights { encrypted: true, annotate: false, modify: false };
    }
    let rights = unsafe { Rights::of(b.FPDF_GetSecurityHandlerRevision(doc), b.FPDF_GetDocPermissions(doc) as u32) };
    unsafe { b.FPDF_CloseDocument(doc) };
    rights
}

/// A page's size in points, and its label if the document gives it one.
#[derive(Debug, Clone)]
pub struct Page {
    pub width: f32,
    pub height: f32,
}

/// A heading in the document's outline, and the page it goes to.
#[derive(Debug, Clone)]
pub struct Heading {
    pub title: String,
    pub depth: usize,
    pub page: Option<usize>,
}

/// Where a search found the text: a page, and rectangles on it as fractions of the page,
/// left, top, right, bottom, top down.
#[derive(Debug, Clone)]
pub struct Found {
    pub page: usize,
    pub rects: Vec<[f32; 4]>,
}

pub enum Ask {
    /// Draw page `page`, `width` pixels across and no more than `most_height` down.
    Draw { page: usize, width: u32, most_height: u32 },
    Find { text: String, ticket: u64 },
    /// The words on page `page`, for the room's speech to know what is being read.
    Text { page: usize },
    /// Markup written into the document as annotations and crop boxes (`annotate`): over the
    /// file, or as a copy beside it.
    Save { marks: crate::markup::Marks, copy: bool },
    /// The password to open a locked document with.
    Unlock(Secret),
    /// Page `page`'s own characters and where each is, for Live Text -- or word that it has none.
    Letters { page: usize },
    /// Page `page` drawn to be read: at 300 dots to the inch, which keeps small print readable,
    /// within the 50 million pixels hemera reads.
    Scan { page: usize },
}

/// One run of a page's own text, as Live Text takes it: its characters, the rectangle they are
/// in (left, top, right, bottom, in the page's points from its top left), and each character's
/// bytes in the text and its left and right.
#[derive(Debug, Clone, PartialEq)]
pub struct Lettered {
    pub text: String,
    pub rect: [f32; 4],
    pub places: Vec<(std::ops::Range<usize>, f32, f32)>,
}

/// How many letters a page must have of its own to be taken for text rather than a scan.
const LETTERS_OF_ITS_OWN: usize = 4;

/// Page `page`'s own text, run by run as PDFium lays it out -- `None` for a page with next to none
/// of its own (a scan), and for one turned by its /Rotate, whose characters' places are in its
/// unturned space: both are read from a picture of them instead.
fn letters(page: &PdfPage) -> Option<Vec<Lettered>> {
    if !matches!(page.rotation(), Ok(PdfPageRenderRotation::None)) {
        return None;
    }
    let text = page.text().ok()?;
    let height = page.height().value;
    let mut lines = Vec::new();
    let mut letters = 0;
    for segment in text.segments().iter() {
        let Ok(chars) = segment.chars() else { continue };
        let bounds = segment.bounds();
        let mut line = String::new();
        let mut places = Vec::new();
        for c in chars.iter() {
            let Some(s) = c.unicode_string().filter(|s| !s.is_empty() && !s.chars().all(char::is_control)) else { continue };
            let Ok(b) = c.loose_bounds() else { continue };
            let at = line.len();
            line.push_str(&s);
            places.push((at..line.len(), b.left().value, b.right().value));
        }
        if line.trim().is_empty() {
            continue;
        }
        letters += line.chars().filter(|c| c.is_alphanumeric()).count();
        // PDF's y is up from the foot of the page.
        lines.push(Lettered { text: line, rect: [bounds.left().value, height - bounds.top().value, bounds.right().value, height - bounds.bottom().value], places });
    }
    (letters >= LETTERS_OF_ITS_OWN).then_some(lines)
}

pub enum Answer {
    Opened { pages: Vec<Page>, outline: Vec<Heading>, title: Option<String>, rights: Rights },
    /// It is locked with a password, which is asked for: again, if `wrong`, the one given not
    /// opening it.
    Locked { wrong: bool },
    Failed(String),
    Drawn { page: usize, width: u32, height: u32, rgba: Vec<u8> },
    /// The page could not be drawn: not there, or PDFium would not.
    Undrawn { page: usize },
    Found { found: Vec<Found>, ticket: u64 },
    Text { page: usize, text: String },
    /// Where markup was written, or why it was not; and the password it was opened with, for the
    /// file written -- encrypted as it was, with the same password -- to be opened again with.
    Saved { result: Result<PathBuf, String>, password: Option<Secret> },
    /// Page `page`'s own text (`Ask::Letters`), or `None`: it has none, and is to be read.
    Letters { page: usize, lines: Option<Vec<Lettered>> },
    /// Page `page` drawn to be read (`Ask::Scan`).
    Scanned { page: usize, width: u32, height: u32, rgba: Vec<u8> },
}

/// The document at `path` written with `marks`: over itself (a new file beside it renamed over
/// it), or as a copy beside it, "<name> (edited).pdf". Where it was written. An encrypted one is
/// opened with its `password` and stays encrypted as it was: PDFium's update encrypts what it adds
/// with the document's own key, and keeps its security dictionary and file identifier.
fn save(pdfium: &Pdfium, path: &Path, marks: &crate::markup::Marks, copy: bool, password: Option<&str>) -> Result<PathBuf, String> {
    let bytes = crate::place::read(path)?;
    let written = crate::annotate::annotate(pdfium, &bytes, marks, password)?;
    if !copy {
        crate::place::replace(path, &written)?;
        return Ok(path.to_path_buf());
    }
    let folder = crate::place::parent(path).ok_or("it is in no folder")?;
    let name = crate::save::copy_name(&crate::place::name(path), None, |candidate| crate::place::exists(&crate::place::join(&folder, candidate)));
    let to = crate::place::join(&folder, &name);
    crate::place::write_new(&to, &written)?;
    Ok(to)
}

pub struct Pdf {
    asks: Sender<Ask>,
    pub answers: Receiver<Answer>,
}

/// Where the library is, the first that exists.
fn library() -> Option<PathBuf> {
    let name = |dir: &Path| Pdfium::pdfium_platform_library_name_at_path(dir);
    let mut places: Vec<PathBuf> = Vec::new();
    if let Some(named) = std::env::var_os("SPATILOUPE_PDFIUM") {
        places.push(named.into());
    }
    if let Some(dir) = std::env::var_os("SPATIAND_APPS_DIR") {
        places.push(name(Path::new(&dir)));
    }
    if let Some(dir) = std::env::current_exe().ok().and_then(|exe| exe.parent().map(Path::to_path_buf)) {
        places.push(name(&dir));
        // A Mac bundle keeps libraries in Frameworks, beside MacOS; an installed Linux release in
        // lib, beside bin (tools/make-release.sh).
        places.push(name(&dir.join("../Frameworks")));
        places.push(name(&dir.join("../lib")));
    }
    let platform = if cfg!(target_os = "android") {
        "android-arm64"
    } else if cfg!(target_os = "macos") && cfg!(target_arch = "aarch64") {
        "mac-arm64"
    } else if cfg!(target_os = "macos") {
        "mac-x64"
    } else {
        "linux-x64"
    };
    places.push(name(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../third_party/pdfium").join(platform).join("lib")));
    places.into_iter().find(|p| p.is_file())
}

/// PDFium, bound to its library: by the first thread to ask, for the whole process.
pub fn bind() -> Result<Pdfium, String> {
    let found = library().ok_or("PDFium is not installed beside SpatiLoupe")?;
    match Pdfium::bind_to_library(&found) {
        Ok(bindings) => Ok(Pdfium::new(bindings)),
        Err(PdfiumError::PdfiumLibraryBindingsAlreadyInitialized) => Ok(Pdfium::default()),
        Err(e) => Err(format!("PDFium would not load: {e}")),
    }
}

fn outline(document: &PdfDocument) -> Vec<Heading> {
    fn walk(bookmark: PdfBookmark, depth: usize, out: &mut Vec<Heading>) {
        if out.len() > 2000 || depth > 8 {
            return;
        }
        let page = bookmark.destination().and_then(|d| d.page_index().ok()).map(|i| i as usize);
        out.push(Heading { title: bookmark.title().unwrap_or_default().trim().to_string(), depth, page });
        for child in bookmark.iter_direct_children() {
            walk(child, depth + 1, out);
        }
    }
    let mut out = Vec::new();
    if let Some(root) = document.bookmarks().root() {
        for top in root.iter_siblings() {
            walk(top, 0, &mut out);
        }
    }
    out.retain(|h| !h.title.is_empty());
    out
}

impl Pdf {
    /// Open the document at `path`; the first answer says whether it opened, or that it is
    /// locked and its password is wanted (`Ask::Unlock`). `password` is one it was opened with
    /// before, tried first: the document saved, or its copy.
    pub fn open(path: &Path, password: Option<Secret>) -> Pdf {
        let (asks, asked) = channel::<Ask>();
        let (tell, answers) = channel();
        let path = path.to_path_buf();
        let _ = std::thread::Builder::new().name("pdf".into()).spawn(move || {
            let pdfium = match bind() {
                Ok(pdfium) => pdfium,
                Err(e) => {
                    let _ = tell.send(Answer::Failed(e));
                    return;
                }
            };
            // A remote's file is read once, and opened from memory as often as a password is tried.
            let remote = match crate::place::remote(&path) {
                Some(_) => match crate::place::read(&path) {
                    Ok(bytes) => Some(bytes),
                    Err(e) => {
                        let _ = tell.send(Answer::Failed(format!("This PDF would not open: {e}")));
                        return;
                    }
                },
                None => None,
            };
            let mut password = password;
            // Whether the password tried was the wearer's, and so wrong if it does not open it.
            let mut typed = false;
            let document = loop {
                let given = password.as_ref().map(Secret::expose);
                let loaded = match &remote {
                    Some(bytes) => pdfium.load_pdf_from_byte_vec(bytes.clone(), given),
                    None => pdfium.load_pdf_from_file(&path, given),
                };
                match loaded {
                    Ok(document) => break document,
                    Err(PdfiumError::PdfiumLibraryInternalError(PdfiumInternalError::PasswordError)) => {
                        // Let go of at once: a password that does not open it is not kept.
                        drop(password.take());
                        if tell.send(Answer::Locked { wrong: typed }).is_err() {
                            return;
                        }
                        // Nothing else is asked of a document not yet open.
                        loop {
                            match asked.recv() {
                                Ok(Ask::Unlock(secret)) => {
                                    password = Some(secret);
                                    typed = true;
                                    break;
                                }
                                Ok(_) => continue,
                                Err(_) => return,
                            }
                        }
                    }
                    Err(PdfiumError::PdfiumLibraryInternalError(PdfiumInternalError::SecurityError)) => {
                        let _ = tell.send(Answer::Failed("This PDF is locked in a way SpatiLoupe cannot open: by a certificate, not a password.".into()));
                        return;
                    }
                    Err(e) => {
                        let _ = tell.send(Answer::Failed(format!("This PDF would not open: {e}")));
                        return;
                    }
                }
            };
            let given = password.as_ref().map(Secret::expose);
            let rights = rights(&pdfium, &path, remote.as_deref(), given);
            drop(remote);
            let pages: Vec<Page> = document.pages().iter().map(|page| Page { width: page.width().value, height: page.height().value }).collect();
            let title = document.metadata().get(PdfDocumentMetadataTagType::Title).map(|t| t.value().trim().to_string()).filter(|t| !t.is_empty());
            if tell.send(Answer::Opened { pages, outline: outline(&document), title, rights }).is_err() {
                return;
            }
            let mut queue: Vec<Ask> = Vec::new();
            loop {
                // What was asked last first: the page now on show, not the ones scrolled past.
                match asked.recv() {
                    Ok(ask) => queue.push(ask),
                    Err(_) => return,
                }
                while let Ok(ask) = asked.try_recv() {
                    queue.push(ask);
                }
                while let Some(ask) = queue.pop() {
                    let answer = match ask {
                        Ask::Draw { page, width, most_height } => {
                            let Ok(at) = document.pages().get(page as PdfPageIndex) else {
                                // Answered all the same: whoever asked may be waiting for it.
                                let _ = tell.send(Answer::Undrawn { page });
                                continue;
                            };
                            let config = PdfRenderConfig::new().set_target_width(width.max(1) as Pixels).set_maximum_height(most_height.max(1) as Pixels).render_form_data(true).render_annotations(true);
                            let rendered = at.render_with_config(&config).map(|bitmap| (bitmap.width() as u32, bitmap.height() as u32, bitmap.as_rgba_bytes()));
                            match rendered {
                                Ok((w, h, mut rgba)) => {
                                    // White behind it: a page with nothing printed on part of
                                    // it is paper there, not a hole.
                                    for px in rgba.chunks_exact_mut(4) {
                                        let a = px[3] as u32;
                                        for c in &mut px[..3] {
                                            *c = ((*c as u32 * a + 255 * (255 - a)) / 255) as u8;
                                        }
                                        px[3] = 255;
                                    }
                                    Answer::Drawn { page, width: w, height: h, rgba }
                                }
                                Err(_) => Answer::Undrawn { page },
                            }
                        }
                        Ask::Text { page } => {
                            let text = document.pages().get(page as PdfPageIndex).ok().and_then(|at| at.text().ok().map(|t| t.all())).unwrap_or_default();
                            Answer::Text { page, text }
                        }
                        Ask::Letters { page } => {
                            let lines = document.pages().get(page as PdfPageIndex).ok().and_then(|at| letters(&at));
                            Answer::Letters { page, lines }
                        }
                        Ask::Scan { page } => {
                            let Ok(at) = document.pages().get(page as PdfPageIndex) else { continue };
                            // At 300 dots to the inch, as a scanner would; within what hemera reads.
                            let (w, h) = (at.width().value / 72.0 * 300.0, at.height().value / 72.0 * 300.0);
                            let (w, h) = spatiand_app::live_text::within_limit((w.round().max(1.0) as u32, h.round().max(1.0) as u32));
                            let config = PdfRenderConfig::new().set_target_width(w as Pixels).set_maximum_height(h.max(1) as Pixels).render_form_data(true).render_annotations(true);
                            match at.render_with_config(&config).map(|bitmap| (bitmap.width() as u32, bitmap.height() as u32, bitmap.as_rgba_bytes())) {
                                Ok((w, h, mut rgba)) => {
                                    // Paper behind it, as when it is drawn to be shown.
                                    for px in rgba.chunks_exact_mut(4) {
                                        let a = px[3] as u32;
                                        for c in &mut px[..3] {
                                            *c = ((*c as u32 * a + 255 * (255 - a)) / 255) as u8;
                                        }
                                        px[3] = 255;
                                    }
                                    Answer::Scanned { page, width: w, height: h, rgba }
                                }
                                Err(_) => continue,
                            }
                        }
                        Ask::Save { marks, copy } => {
                            let result = save(&pdfium, &path, &marks, copy, password.as_ref().map(Secret::expose));
                            // The file written opens with the same password: given back for it.
                            let again = result.is_ok().then(|| password.as_ref().map(Secret::again)).flatten();
                            Answer::Saved { result, password: again }
                        }
                        // Open already.
                        Ask::Unlock(_) => continue,
                        Ask::Find { text, ticket } => {
                            let mut found = Vec::new();
                            for (index, page) in document.pages().iter().enumerate() {
                                let Ok(words) = page.text() else { continue };
                                let Ok(search) = words.search(&text, &PdfSearchOptions::new()) else { continue };
                                let (pw, ph) = (page.width().value.max(1.0), page.height().value.max(1.0));
                                for segments in search.iter(PdfSearchDirection::SearchForward).take(500) {
                                    let rects = segments
                                        .iter()
                                        .map(|s| {
                                            let b = s.bounds();
                                            // PDF's y is up from the foot of the page.
                                            [b.left().value / pw, 1.0 - b.top().value / ph, b.right().value / pw, 1.0 - b.bottom().value / ph]
                                        })
                                        .collect();
                                    found.push(Found { page: index, rects });
                                }
                                if found.len() > 2000 {
                                    break;
                                }
                            }
                            Answer::Found { found, ticket }
                        }
                    };
                    if tell.send(answer).is_err() {
                        return;
                    }
                    // Something newer may have been asked while that was drawn.
                    while let Ok(ask) = asked.try_recv() {
                        queue.push(ask);
                    }
                }
            }
        });
        Pdf { asks, answers }
    }

    pub fn ask(&self, ask: Ask) {
        let _ = self.asks.send(ask);
    }

    /// A document whose thread is played by a test: what it answers, and what it is asked.
    #[cfg(test)]
    pub fn played() -> (Pdf, Sender<Answer>, Receiver<Ask>) {
        let (asks, asked) = channel();
        let (tell, answers) = channel();
        (Pdf { asks, answers }, tell, asked)
    }
}

/// How wide a page `width` by `height` is drawn to fit within a square `side` across: a long
/// strip of a page is as tall as the square, not `side` wide and a hundred times as tall.
pub fn width_within(width: f32, height: f32, side: u32) -> u32 {
    let (w, h) = (width.max(1.0), height.max(1.0));
    ((side as f32 * (w / h).min(1.0)).round() as u32).clamp(1, side.max(1))
}

/// The first page of a PDF, fitted within a square `side` across, for a tile: on the thread that
/// asks, since a tile's picture is made on a thread already. A locked one has none.
pub fn first_page(path: &Path, side: u32) -> Option<(u32, u32, Vec<u8>)> {
    let pdf = Pdf::open(path, None);
    let width = match pdf.answers.recv().ok()? {
        Answer::Opened { pages, .. } => {
            let first = pages.first()?;
            width_within(first.width, first.height, side)
        }
        _ => return None,
    };
    pdf.ask(Ask::Draw { page: 0, width, most_height: side });
    match pdf.answers.recv().ok()? {
        Answer::Drawn { width, height, rgba, .. } => Some((width, height, rgba)),
        _ => None,
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    #[test]
    fn a_page_is_drawn_within_its_tile() {
        // A4, a long strip, a wide one, and nothing at all.
        assert_eq!(width_within(595.0, 842.0, 300), 212);
        assert_eq!(width_within(10.0, 14400.0, 300), 1);
        assert_eq!(width_within(14400.0, 10.0, 300), 300);
        assert_eq!(width_within(0.0, 0.0, 300), 300);
    }

    /// A PDF of these pages -- each its size in points and what is printed on it -- written to a
    /// file of its own.
    pub fn made(name: &str, pages: &[(f32, f32, &str)]) -> PathBuf {
        made_turned(name, &pages.iter().map(|&(w, h, words)| (w, h, words, 0)).collect::<Vec<_>>())
    }

    /// The same, each page turned as its `/Rotate` says.
    pub fn made_turned(name: &str, pages: &[(f32, f32, &str, u32)]) -> PathBuf {
        let mut objects = vec!["<< /Type /Catalog /Pages 2 0 R >>".to_string(), String::new(), "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_string()];
        let mut kids = Vec::new();
        for (w, h, words, rotate) in pages {
            let content = format!("BT /F1 12 Tf 10 10 Td ({words}) Tj ET");
            objects.push(format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len()));
            let contents = objects.len();
            objects.push(format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {w} {h}] /Rotate {rotate} /Contents {contents} 0 R /Resources << /Font << /F1 3 0 R >> >> >>"));
            kids.push(format!("{} 0 R", objects.len()));
        }
        objects[1] = format!("<< /Type /Pages /Kids [{}] /Count {} >>", kids.join(" "), kids.len());
        let mut out = b"%PDF-1.4\n".to_vec();
        let mut at = Vec::new();
        for (i, object) in objects.iter().enumerate() {
            at.push(out.len());
            out.extend_from_slice(format!("{} 0 obj\n{object}\nendobj\n", i + 1).as_bytes());
        }
        let xref = out.len();
        out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes());
        for offset in at {
            out.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
        }
        out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objects.len() + 1).as_bytes());
        let dir = std::env::temp_dir().join(format!("spatiloupe-pdf-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        std::fs::write(&path, out).unwrap();
        path
    }

    /// `first_page`, given at most ten seconds.
    fn first_page_soon(path: &Path, side: u32) -> Option<Option<(u32, u32, Vec<u8>)>> {
        let (tell, told) = channel();
        let path = path.to_path_buf();
        std::thread::spawn(move || {
            let _ = tell.send(first_page(&path, side));
        });
        told.recv_timeout(std::time::Duration::from_secs(10)).ok()
    }

    /// The PDF's next answer, given ten seconds.
    fn next(pdf: &Pdf) -> Answer {
        pdf.answers.recv_timeout(std::time::Duration::from_secs(10)).expect("an answer")
    }

    #[test]
    fn a_locked_pdf_opens_with_its_password_and_its_copy_opens_with_it_again() {
        if library().is_none() {
            eprintln!("PDFium is not here (tools/fetch-pdfium.sh): not tried");
            return;
        }
        use crate::markup::{Area, Ink, Marks, Note, Shape, Weight};
        let (name, bytes, password, ..) = crate::annotate::tests::LOCKED[5];
        let dir = std::env::temp_dir().join(format!("spatiloupe-locked-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        std::fs::write(&path, bytes).unwrap();
        let pdf = Pdf::open(&path, None);
        assert!(matches!(next(&pdf), Answer::Locked { wrong: false }));
        // Asked of it before it is open: let go of, not answered.
        pdf.ask(Ask::Text { page: 0 });
        pdf.ask(Ask::Unlock(Secret::new("wrong".into())));
        assert!(matches!(next(&pdf), Answer::Locked { wrong: true }));
        pdf.ask(Ask::Unlock(Secret::new(password.unwrap().into())));
        match next(&pdf) {
            Answer::Opened { pages, rights, .. } => {
                assert_eq!(pages.len(), 2);
                assert_eq!(rights, Rights { encrypted: true, annotate: true, modify: true });
            }
            _ => panic!("opened"),
        }
        // Marked up and saved as a copy: written beside it, and its password given back for it.
        let marks = Marks { notes: vec![Note { page: 0, shape: Shape::Box(Area::new(100.0, 100.0, 300.0, 200.0)), ink: Ink::Red, weight: Weight::Medium, size: 4.0 }], ..Marks::default() };
        pdf.ask(Ask::Save { marks: marks.clone(), copy: true });
        let (copy, again) = loop {
            if let Answer::Saved { result, password } = next(&pdf) {
                break (result.unwrap(), password.expect("its password"));
            }
        };
        assert_eq!(copy, dir.join("aes-256-user (edited).pdf"));
        // The copy opens with it at once, nothing asked; without it, it is locked as the first is.
        assert!(matches!(next(&Pdf::open(&copy, Some(again))), Answer::Opened { .. }));
        assert!(matches!(next(&Pdf::open(&copy, None)), Answer::Locked { wrong: false }));
        // Saved over itself: the same.
        pdf.ask(Ask::Save { marks, copy: false });
        let (over, again) = loop {
            if let Answer::Saved { result, password } = next(&pdf) {
                break (result.unwrap(), password.expect("its password"));
            }
        };
        assert_eq!(over, path);
        assert!(matches!(next(&Pdf::open(&path, Some(again))), Answer::Opened { .. }));
        drop(pdf);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A PDF of one A4 page that is only a picture -- a scan, a grey square drawn over the whole
    /// page -- and no text of its own.
    fn scanned_page(name: &str) -> PathBuf {
        let content = "q 595 0 0 842 0 0 cm BI /W 2 /H 2 /CS /G /BPC 8 ID \u{0}\u{80}\u{80}\u{ff} EI Q";
        let objects = [
            "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595 842] /Contents 4 0 R >>".to_string(),
            format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len()),
        ];
        let mut out = b"%PDF-1.4\n".to_vec();
        let mut at = Vec::new();
        for (i, object) in objects.iter().enumerate() {
            at.push(out.len());
            out.extend_from_slice(format!("{} 0 obj\n{object}\nendobj\n", i + 1).as_bytes());
        }
        let xref = out.len();
        out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes());
        for offset in at {
            out.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
        }
        out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objects.len() + 1).as_bytes());
        let dir = std::env::temp_dir().join(format!("spatiloupe-pdf-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        std::fs::write(&path, out).unwrap();
        path
    }

    #[test]
    fn a_page_s_own_text_is_its_characters_and_a_scan_is_drawn_to_be_read() {
        if library().is_none() {
            eprintln!("PDFium is not here (tools/fetch-pdfium.sh): not tried");
            return;
        }
        // A page with text of its own: its characters, where they are, from the page's top left.
        let pdf = Pdf::open(&made("lettered.pdf", &[(595.0, 842.0, "Live Text in a PDF")]), None);
        assert!(matches!(next(&pdf), Answer::Opened { .. }));
        pdf.ask(Ask::Letters { page: 0 });
        let lines = loop {
            if let Answer::Letters { lines, .. } = next(&pdf) {
                break lines.expect("its own text");
            }
        };
        let all: String = lines.iter().map(|l| l.text.as_str()).collect();
        assert!(all.contains("Live Text"), "{all:?}");
        // Set 10 points in and 10 up from the foot (Helvetica 12): near the page's foot, from its top.
        let first = &lines[0];
        assert!(first.rect[0] >= 5.0 && first.rect[0] <= 15.0 && first.rect[3] > 820.0 && first.rect[1] < first.rect[3], "{:?}", first.rect);
        assert_eq!(first.places.len(), first.text.chars().count());
        assert!(first.places.windows(2).all(|w| w[0].1 <= w[1].1), "left to right");
        // A page that is only a picture: no text of its own; drawn at 300 dots to the inch.
        let pdf = Pdf::open(&scanned_page("scanned.pdf"), None);
        assert!(matches!(next(&pdf), Answer::Opened { .. }));
        pdf.ask(Ask::Letters { page: 0 });
        assert!(matches!(next(&pdf), Answer::Letters { lines: None, .. }));
        pdf.ask(Ask::Scan { page: 0 });
        match next(&pdf) {
            Answer::Scanned { width, height, rgba, .. } => {
                assert!((2470..=2490).contains(&width) && (3500..=3515).contains(&height), "{width}x{height}");
                assert_eq!(rgba.len(), (width * height * 4) as usize);
            }
            _ => panic!("drawn to be read"),
        }
    }

    #[test]
    fn what_a_pdf_s_owner_allows_is_read_from_its_permissions() {
        // Not encrypted: everything. The permissions' bit 6 is notes, bit 4 changing the pages.
        assert_eq!(Rights::of(-1, 0), Rights::ALL);
        assert_eq!(Rights::of(4, 0xFFFF_FFFC), Rights { encrypted: true, annotate: true, modify: true });
        assert_eq!(Rights::of(4, (-36i32) as u32), Rights { encrypted: true, annotate: false, modify: true });
        assert_eq!(Rights::of(6, (-12i32) as u32), Rights { encrypted: true, annotate: true, modify: false });
    }

    #[test]
    fn a_tile_of_a_pdf_with_no_pages_is_none_and_comes() {
        if library().is_none() {
            eprintln!("PDFium is not here (tools/fetch-pdfium.sh): not tried");
            return;
        }
        let path = made("empty.pdf", &[]);
        assert_eq!(first_page_soon(&path, 300), Some(None));
        // And a page: drawn, within the tile, however long the page is.
        let path = made("strip.pdf", &[(100.0, 14400.0, "x")]);
        let (w, h, rgba) = first_page_soon(&path, 300).expect("an answer").expect("a page");
        assert!(w <= 300 && h <= 300, "{w}x{h}");
        assert_eq!(rgba.len(), (w * h * 4) as usize);
        // A page too thin to be a pixel wide in its tile: answered, whatever the answer is.
        let path = made("thread.pdf", &[(10.0, 14400.0, "x")]);
        assert!(first_page_soon(&path, 300).is_some());
    }
}
