//! PDFs, by PDFium on a thread of their own.
//!
//! PDFium is loaded at run time from beside the program (or `SPATILOUPE_PDFIUM`, or the copy
//! `tools/fetch-pdfium.sh` put in the tree, for a build being worked on). One thread holds it
//! and the open document, and is asked for pages at a width, for the words on them, and to
//! find text; it answers on a channel the viewer looks at between pictures.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender};

use pdfium_render::prelude::*;

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
}

pub enum Answer {
    Opened { pages: Vec<Page>, outline: Vec<Heading>, title: Option<String> },
    Failed(String),
    Drawn { page: usize, width: u32, height: u32, rgba: Vec<u8> },
    /// The page could not be drawn: not there, or PDFium would not.
    Undrawn { page: usize },
    Found { found: Vec<Found>, ticket: u64 },
    Text { page: usize, text: String },
    /// Where markup was written, or why it was not.
    Saved(Result<PathBuf, String>),
}

/// The document at `path` written with `marks`: over itself (a new file beside it renamed over
/// it), or as a copy beside it, "<name> (edited).pdf". Where it was written.
fn save(pdfium: &Pdfium, path: &Path, marks: &crate::markup::Marks, copy: bool) -> Result<PathBuf, String> {
    let bytes = crate::place::read(path)?;
    let written = crate::annotate::annotate(pdfium, &bytes, marks)?;
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
        // A Mac bundle keeps libraries in Frameworks, beside MacOS.
        places.push(name(&dir.join("../Frameworks")));
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
    /// Open the document at `path`; the first answer says whether it opened.
    pub fn open(path: &Path) -> Pdf {
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
            let loaded = match crate::place::remote(&path) {
                Some(_) => crate::place::read(&path).map_err(|e| e).and_then(|bytes| pdfium.load_pdf_from_byte_vec(bytes, None).map_err(|e| e.to_string())),
                None => pdfium.load_pdf_from_file(&path, None).map_err(|e| e.to_string()),
            };
            let document = match loaded {
                Ok(document) => document,
                Err(e) if e.contains("Password") => {
                    let _ = tell.send(Answer::Failed("This PDF is locked with a password.".into()));
                    return;
                }
                Err(e) => {
                    let _ = tell.send(Answer::Failed(format!("This PDF would not open: {e}")));
                    return;
                }
            };
            let pages: Vec<Page> = document.pages().iter().map(|page| Page { width: page.width().value, height: page.height().value }).collect();
            let title = document.metadata().get(PdfDocumentMetadataTagType::Title).map(|t| t.value().trim().to_string()).filter(|t| !t.is_empty());
            if tell.send(Answer::Opened { pages, outline: outline(&document), title }).is_err() {
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
                        Ask::Save { marks, copy } => Answer::Saved(save(&pdfium, &path, &marks, copy)),
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
/// asks, since a tile's picture is made on a thread already.
pub fn first_page(path: &Path, side: u32) -> Option<(u32, u32, Vec<u8>)> {
    let pdf = Pdf::open(path);
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
