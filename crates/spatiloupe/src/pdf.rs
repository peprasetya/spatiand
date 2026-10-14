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
    /// Draw page `page`, `width` pixels across. `ticket` comes back with it.
    Draw { page: usize, width: u32, ticket: u64 },
    Find { text: String, ticket: u64 },
}

pub enum Answer {
    Opened { pages: Vec<Page>, outline: Vec<Heading>, title: Option<String> },
    Failed(String),
    Drawn { page: usize, width: u32, height: u32, rgba: Vec<u8>, ticket: u64 },
    Found { found: Vec<Found>, ticket: u64 },
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
            let Some(found) = library() else {
                let _ = tell.send(Answer::Failed("PDFium is not installed beside SpatiLoupe".into()));
                return;
            };
            let pdfium = match Pdfium::bind_to_library(&found) {
                Ok(bindings) => Pdfium::new(bindings),
                // Bound once for the whole process, by whichever thread came first.
                Err(PdfiumError::PdfiumLibraryBindingsAlreadyInitialized) => Pdfium::default(),
                Err(e) => {
                    let _ = tell.send(Answer::Failed(format!("PDFium would not load: {e}")));
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
                        Ask::Draw { page, width, ticket } => {
                            let Ok(at) = document.pages().get(page as PdfPageIndex) else { continue };
                            let config = PdfRenderConfig::new().set_target_width(width.max(16) as Pixels).render_form_data(true).render_annotations(true);
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
                                    Answer::Drawn { page, width: w, height: h, rgba, ticket }
                                }
                                Err(_) => continue,
                            }
                        }
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
}

/// The first page of a PDF, `width` across, for a tile: on the thread that asks, since a
/// tile's picture is made on a thread already.
pub fn first_page(path: &Path, width: u32) -> Option<(u32, u32, Vec<u8>)> {
    let pdf = Pdf::open(path);
    match pdf.answers.recv().ok()? {
        Answer::Opened { .. } => {}
        _ => return None,
    }
    pdf.ask(Ask::Draw { page: 0, width, ticket: 0 });
    match pdf.answers.recv().ok()? {
        Answer::Drawn { width, height, rgba, .. } => Some((width, height, rgba)),
        _ => None,
    }
}
