//! The WebKitGTK engine host as SpatiMesh runs it: a process of its own, spoken to over the
//! wire, drawing into shared memory, with real pages in WebKit's own processes.
//!
//! These need a display for GTK (Xvfb in CI: `xvfb-run cargo test`). Without one, or where the
//! program was built without WebKitGTK, they say so and pass.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::Child;
use std::time::{Duration, Instant};

use spatimesh::engine::{Button, Cookie, DialogKind, Format, Happened, KeyPress, Modifiers, ProfileSetup, TabId, View};
use spatimesh::wire::{FromHost, Link, Pictures, ToHost};

const PROGRAM: &str = env!("CARGO_BIN_EXE_spatimesh-webkitgtk");

struct Host {
    child: Child,
    link: Link,
    pictures: HashMap<TabId, Pictures>,
    /// Each tab's newest picture, copied out: (width, height, format, bytes).
    latest: HashMap<TabId, (u32, u32, Format, Vec<u8>)>,
    seen: Vec<Happened>,
}

fn can_run() -> bool {
    if std::env::var_os("DISPLAY").is_none() && std::env::var_os("WAYLAND_DISPLAY").is_none() {
        eprintln!("no display: the WebKitGTK engine is not tested here (run under xvfb-run)");
        return false;
    }
    true
}

fn private() -> ProfileSetup {
    ProfileSetup { id: "private".into(), private: true, data: PathBuf::new(), cache: PathBuf::new() }
}

fn kept(root: &Path) -> ProfileSetup {
    ProfileSetup { id: "default".into(), private: false, data: root.join("data"), cache: root.join("cache") }
}

impl Host {
    fn start(setup: &ProfileSetup) -> Option<Host> {
        if !can_run() {
            return None;
        }
        let (child, link) = spatimesh::host::start(Path::new(PROGRAM), setup).expect("the engine program starts");
        let mut host = Host { child, link, pictures: HashMap::new(), latest: HashMap::new(), seen: Vec::new() };
        // The first thing a host says is what it is; "failed" if WebKitGTK could not start.
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            assert!(Instant::now() < deadline, "the engine said nothing");
            host.link.wait_readable(100);
            for (message, _) in host.link.receive_from_host() {
                if let FromHost::Hello { engine } = message {
                    eprintln!("engine: {engine}");
                    if engine.starts_with("failed") || engine.contains("cannot show pages") {
                        eprintln!("the engine could not start here; not tested");
                        return None;
                    }
                    return Some(host);
                }
            }
            if host.link.is_closed() {
                eprintln!("the engine program ended at once (built without WebKitGTK?); not tested");
                return None;
            }
        }
    }

    fn send(&mut self, message: ToHost) {
        self.link.send(&message.encode(), &[]).expect("the engine is listening");
    }

    /// Read what has come, keeping pictures, until `want` matches something that happened.
    fn wait_for(&mut self, what: &str, seconds: u64, mut want: impl FnMut(&Happened) -> bool) -> Happened {
        let deadline = Instant::now() + Duration::from_secs(seconds);
        loop {
            if let Some(found) = self.seen.iter().position(&mut want) {
                return self.seen.remove(found);
            }
            assert!(Instant::now() < deadline, "waited {seconds} s for {what}; saw {:?}", self.seen);
            assert!(!self.link.is_closed(), "the engine went while waiting for {what}");
            self.link.wait_readable(50);
            for (message, fds) in self.link.receive_from_host() {
                match message {
                    FromHost::Buffers { tab, width, height, format, .. } => {
                        self.pictures.insert(tab, Pictures::map(width, height, format, fds).unwrap());
                    }
                    FromHost::Picture { tab, buffer, .. } => {
                        let p = &self.pictures[&tab];
                        self.latest.insert(tab, (p.width, p.height, p.format, p.buffer(buffer).unwrap().to_vec()));
                        self.send(ToHost::Release { tab, buffer });
                        self.seen.push(Happened::Picture { tab });
                    }
                    FromHost::Happened(h) => self.seen.push(h),
                    // Only a Mac's WebKit hands pictures over on the GPU.
                    FromHost::Surface { tab, id, .. } => self.send(ToHost::ReleaseSurface { tab, id }),
                    FromHost::Hello { .. } => {}
                }
            }
        }
    }

    fn title(&mut self, tab: TabId, prefix: &str) -> String {
        match self.wait_for(&format!("a title starting {prefix:?}"), 20, |h| matches!(h, Happened::Title { tab: t, title } if *t == tab && title.starts_with(prefix))) {
            Happened::Title { title, .. } => title,
            _ => unreachable!(),
        }
    }

    fn loaded(&mut self, tab: TabId) {
        self.wait_for("the page to finish loading", 30, |h| matches!(h, Happened::Progress { tab: t, loading: false, .. } if *t == tab));
    }

    /// The colour at a point of a tab's newest picture, red, green, blue.
    fn pixel(&self, tab: TabId, x: u32, y: u32) -> [u8; 3] {
        let (width, _, format, bytes) = &self.latest[&tab];
        let i = ((y * width + x) * 4) as usize;
        match format {
            Format::Bgra => [bytes[i + 2], bytes[i + 1], bytes[i]],
            Format::Rgba => [bytes[i], bytes[i + 1], bytes[i + 2]],
        }
    }

    fn click(&mut self, tab: TabId, x: f32, y: f32) {
        let m = Modifiers::default();
        self.send(ToHost::Pointer { tab, x, y, button: None, modifiers: m });
        self.send(ToHost::Pointer { tab, x, y, button: Some((Button::Left, true)), modifiers: m });
        self.send(ToHost::Pointer { tab, x, y, button: Some((Button::Left, false)), modifiers: m });
    }

    fn type_text(&mut self, tab: TabId, text: &str) {
        for ch in text.chars() {
            for pressed in [true, false] {
                let key = KeyPress { code: 0, key: spatiand_app::Key::Char(ch), text: Some(ch.to_string()), pressed, repeat: false, modifiers: Modifiers::default() };
                self.send(ToHost::Key { tab, key });
            }
        }
    }

    fn cookies(&mut self) -> Vec<Cookie> {
        self.send(ToHost::AskCookies { addresses: Vec::new() });
        match self.wait_for("the profile's cookies", 20, |h| matches!(h, Happened::Cookies { .. })) {
            Happened::Cookies { cookies } => cookies,
            _ => unreachable!(),
        }
    }

    fn quit(mut self) {
        let _ = self.link.send(&ToHost::Quit.encode(), &[]);
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            if let Ok(Some(_)) = self.child.try_wait() {
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let _ = self.child.kill();
    }
}

impl Drop for Host {
    fn drop(&mut self) {
        if let Ok(None) = self.child.try_wait() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

fn page(html: &str) -> String {
    format!("data:text/html;charset=utf-8,{}", spatimesh::address::form_encode(html).replace('+', "%20"))
}

fn near(a: [u8; 3], b: [u8; 3]) -> bool {
    a.iter().zip(b).all(|(x, y)| (*x as i32 - y as i32).abs() <= 3)
}

#[test]
fn a_page_is_drawn() {
    let Some(mut host) = Host::start(&private()) else { return };
    let url = page("<body style='margin:0;background:rgb(10,200,30)'><div style='width:40px;height:40px;background:rgb(250,0,0)'></div></body>");
    host.send(ToHost::Open { tab: 1, url, view: View::new(320, 240, 1.0) });
    host.loaded(1);
    // Pictures until one shows the page.
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        host.wait_for("a picture", 20, |h| matches!(h, Happened::Picture { tab: 1 }));
        if near(host.pixel(1, 200, 150), [10, 200, 30]) {
            break;
        }
        assert!(Instant::now() < deadline, "the page never showed: {:?} at the middle", host.pixel(1, 200, 150));
    }
    assert!(near(host.pixel(1, 10, 10), [250, 0, 0]), "the red square: {:?}", host.pixel(1, 10, 10));
    let (width, height, ..) = host.latest[&1];
    assert_eq!((width, height), (320, 240));
    host.quit();
}

#[test]
fn typing_clicking_and_scrolling() {
    let Some(mut host) = Host::start(&private()) else { return };
    let url = page(concat!(
        "<body style='margin:0;height:5000px'>",
        "<input id=i style='position:absolute;left:10px;top:10px;width:200px;height:30px' ",
        "onfocus=\"document.title='focused'\" oninput=\"document.title='typed:'+this.value\">",
        "<button style='position:absolute;left:10px;top:60px;width:100px;height:40px' ",
        "onclick=\"document.title='clicked'\">B</button>",
        "<script>addEventListener('scroll',()=>document.title='scrolled:'+scrollY)</script></body>"
    ));
    host.send(ToHost::Open { tab: 1, url, view: View::new(400, 300, 1.0) });
    host.loaded(1);
    host.send(ToHost::Focus { tab: 1, focused: true });
    host.click(1, 50.0, 25.0);
    // As a person waits for the caret: WebKit sends mouse and key events to the page in
    // separate queues, and on a busy machine a key can reach it before the click that
    // chose the field.
    host.title(1, "focused");
    host.type_text(1, "hi");
    host.title(1, "typed:hi");
    host.click(1, 50.0, 80.0);
    host.title(1, "clicked");
    host.send(ToHost::Scroll { tab: 1, x: 200.0, y: 200.0, dx: 0.0, dy: 400.0, modifiers: Modifiers::default() });
    let scrolled = host.title(1, "scrolled:");
    let by: f32 = scrolled["scrolled:".len()..].parse().unwrap();
    assert!(by > 0.0, "{scrolled}");
    host.quit();
}

#[test]
fn history_and_new_tabs() {
    let Some(mut host) = Host::start(&private()) else { return };
    let first = page("<title>first</title><a id=a target=_blank href='https://example.invalid/new' style='display:block;width:300px;height:50px'>new</a>");
    let second = page("<title>second</title>");
    host.send(ToHost::Open { tab: 1, url: first.clone(), view: View::new(400, 300, 1.0) });
    host.title(1, "first");
    host.loaded(1);
    host.send(ToHost::Load { tab: 1, url: second });
    host.title(1, "second");
    host.wait_for("going back to be possible", 20, |h| matches!(h, Happened::History { tab: 1, back: true, forward: false }));
    host.send(ToHost::Back { tab: 1 });
    host.title(1, "first");
    host.wait_for("going forward to be possible", 20, |h| matches!(h, Happened::History { tab: 1, forward: true, .. }));
    host.loaded(1);
    // A link to a new window is SpatiMesh's to open, as a tab.
    host.click(1, 20.0, 20.0);
    let opened = host.wait_for("a tab to be asked for", 20, |h| matches!(h, Happened::OpenTab { opener: 1, .. }));
    assert!(matches!(opened, Happened::OpenTab { ref url, .. } if url == "https://example.invalid/new"), "{opened:?}");
    host.quit();
}

#[test]
fn lists_and_dialogs_are_ours_to_show() {
    let Some(mut host) = Host::start(&private()) else { return };
    let url = page(concat!(
        "<select id=s style='position:absolute;left:10px;top:10px;width:150px;height:30px' ",
        "onchange=\"document.title='chose:'+this.value\">",
        "<option value=a>Apple</option><option value=b>Banana</option><option value=c>Cherry</option></select>",
        "<button style='position:absolute;left:10px;top:80px;width:100px;height:40px' ",
        "onclick=\"document.title=confirm('Sure?')?'yes':'no'\">C</button>"
    ));
    host.send(ToHost::Open { tab: 1, url, view: View::new(400, 300, 1.0) });
    host.loaded(1);
    host.send(ToHost::Focus { tab: 1, focused: true });
    host.click(1, 50.0, 25.0);
    let list = host.wait_for("the list", 20, |h| matches!(h, Happened::Choose { tab: 1, .. }));
    let Happened::Choose { choices, .. } = list else { unreachable!() };
    assert_eq!(choices.iter().map(|c| c.label.as_str()).collect::<Vec<_>>(), vec!["Apple", "Banana", "Cherry"]);
    assert!(choices[0].selected);
    host.send(ToHost::Choose { tab: 1, index: Some(2) });
    assert_eq!(host.title(1, "chose:"), "chose:c");
    host.click(1, 50.0, 100.0);
    let dialog = host.wait_for("the question", 20, |h| matches!(h, Happened::Dialog { tab: 1, .. }));
    assert!(matches!(dialog, Happened::Dialog { kind: DialogKind::Confirm, ref message, .. } if message == "Sure?"), "{dialog:?}");
    host.send(ToHost::Answer { tab: 1, accepted: true, text: None });
    host.title(1, "yes");
    host.quit();
}

/// Every process under `pid`, by its name.
fn descendants(pid: u32) -> Vec<(u32, String)> {
    let mut found = Vec::new();
    let mut parents = vec![pid];
    while let Some(parent) = parents.pop() {
        for entry in std::fs::read_dir("/proc").unwrap().flatten() {
            let Some(p) = entry.file_name().to_str().and_then(|n| n.parse::<u32>().ok()) else { continue };
            let Ok(stat) = std::fs::read_to_string(format!("/proc/{p}/stat")) else { continue };
            // pid (comm) state ppid ...
            let Some(close) = stat.rfind(')') else { continue };
            let name = stat[stat.find('(').unwrap() + 1..close].to_string();
            let ppid: u32 = stat[close + 2..].split_whitespace().nth(1).and_then(|s| s.parse().ok()).unwrap_or(0);
            if ppid == parent {
                found.push((p, name));
                parents.push(p);
            }
        }
    }
    found
}

#[test]
fn a_crashed_page_is_told_and_the_rest_go_on() {
    let Some(mut host) = Host::start(&private()) else { return };
    host.send(ToHost::Open { tab: 1, url: page("<title>one</title>"), view: View::new(200, 100, 1.0) });
    host.title(1, "one");
    host.loaded(1);
    let before: Vec<u32> = descendants(host.child.id()).into_iter().filter(|(_, n)| n.starts_with("WebKitWebProces")).map(|(p, _)| p).collect();
    host.send(ToHost::Open { tab: 2, url: page("<title>two</title>"), view: View::new(200, 100, 1.0) });
    host.title(2, "two");
    host.loaded(2);
    // The second tab's own process: the one that was not there before it.
    let second: Vec<u32> = descendants(host.child.id())
        .into_iter()
        .filter(|(p, n)| n.starts_with("WebKitWebProces") && !before.contains(p))
        .map(|(p, _)| p)
        .collect();
    assert!(!second.is_empty(), "the second tab has a process of its own");
    for pid in &second {
        unsafe { libc::kill(*pid as i32, libc::SIGKILL) };
    }
    host.wait_for("the second tab's crash", 20, |h| matches!(h, Happened::Crashed { tab: 2, .. }));
    // The first tab is unharmed: it still answers.
    host.send(ToHost::Load { tab: 1, url: page("<title>one again</title>") });
    host.title(1, "one again");
    assert!(!host.seen.iter().any(|h| matches!(h, Happened::Crashed { tab: 1, .. })));
    // And the crashed one loads again.
    host.send(ToHost::Reload { tab: 2 });
    host.title(2, "two");
    host.quit();
}

/// A web server on this machine that sets a cookie at /set and shows the page's cookies at
/// /show (in the title).
fn cookie_server() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let mut stream = stream;
            let mut request = [0u8; 4096];
            let n = stream.read(&mut request).unwrap_or(0);
            let request = String::from_utf8_lossy(&request[..n]);
            let (extra, body) = if request.starts_with("GET /set") {
                ("Set-Cookie: kept=yes; Max-Age=3600; Path=/\r\n", "<title>set</title>")
            } else {
                ("", "<script>document.title='cookie:'+document.cookie</script>")
            };
            let _ = write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\n{extra}Content-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
        }
    });
    address
}

fn visit(setup: &ProfileSetup, url: &str, title: &str) -> Option<String> {
    let mut host = Host::start(setup)?;
    host.send(ToHost::Open { tab: 1, url: url.to_string(), view: View::new(200, 100, 1.0) });
    let got = host.title(1, title);
    host.loaded(1);
    // The network process writes its cookies down a moment after it has them.
    std::thread::sleep(Duration::from_millis(1500));
    host.quit();
    Some(got)
}

#[test]
fn a_profile_keeps_its_cookies_and_the_private_one_does_not() {
    if !can_run() {
        return;
    }
    let server = cookie_server();
    let root = std::env::temp_dir().join(format!("spatimesh-webkitgtk-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let profile = kept(&root);
    let Some(_) = visit(&profile, &format!("{server}/set"), "set") else { return };
    assert_eq!(visit(&profile, &format!("{server}/show"), "cookie:").unwrap(), "cookie:kept=yes", "kept across two runs of the engine");
    assert!(root.join("data").exists(), "the profile's website data is in its own folder");

    visit(&private(), &format!("{server}/set"), "set").unwrap();
    assert_eq!(visit(&private(), &format!("{server}/show"), "cookie:").unwrap(), "cookie:", "the private profile kept nothing");
    let _ = std::fs::remove_dir_all(&root);
}


/// The window's side of logins on every machine (docs/spatimesh.md): the engine says all its
/// cookies, and makes and removes those it is given.
#[test]
fn cookies_are_said_made_and_removed() {
    if !can_run() {
        return;
    }
    let server = cookie_server();
    let root = std::env::temp_dir().join(format!("spatimesh-webkitgtk-cookies-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let Some(mut host) = Host::start(&kept(&root)) else { return };
    host.send(ToHost::Open { tab: 1, url: format!("{server}/set"), view: View::new(200, 100, 1.0) });
    host.title(1, "set");
    host.loaded(1);
    let jar = host.cookies();
    let kept = jar.iter().find(|c| c.name == "kept").unwrap_or_else(|| panic!("the page's cookie is said: {jar:?}")).clone();
    assert_eq!((kept.value.as_str(), kept.domain.as_str(), kept.path.as_str()), ("yes", "127.0.0.1", "/"));
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs_f64();
    assert!(kept.expires.is_some_and(|e| (e - now - 3600.0).abs() < 120.0), "its expiry: {:?}", kept.expires);

    let made = Cookie {
        name: "made".into(),
        value: "elsewhere".into(),
        domain: "127.0.0.1".into(),
        path: "/".into(),
        expires: Some((now + 7200.0).round()),
        secure: false,
        http_only: false,
        same_site: String::new(),
    };
    host.send(ToHost::SetCookies { set: vec![made.clone()], remove: vec![kept] });
    std::thread::sleep(Duration::from_millis(500));
    host.send(ToHost::Load { tab: 1, url: format!("{server}/show") });
    assert_eq!(host.title(1, "cookie:"), "cookie:made=elsewhere", "the page has the cookie made, not the one removed");
    let jar = host.cookies();
    assert!(jar.iter().any(|c| c.name == "made" && c.expires.is_some_and(|e| (e - made.expires.unwrap()).abs() < 2.0)), "{jar:?}");
    assert!(!jar.iter().any(|c| c.name == "kept"), "{jar:?}");
    host.quit();
    let _ = std::fs::remove_dir_all(&root);
}

/// A server of fixed answers by path, each with headers of its own: its address.
fn server(answers: Vec<(&'static str, &'static str, Vec<u8>)>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let mut stream = stream;
            let mut request = [0u8; 4096];
            let n = stream.read(&mut request).unwrap_or(0);
            let request = String::from_utf8_lossy(&request[..n]);
            let path = request.split_whitespace().nth(1).unwrap_or("/").to_string();
            let (headers, body) = answers.iter().find(|(p, ..)| *p == path).map(|(_, h, b)| (*h, b.clone())).unwrap_or(("Content-Type: text/html\r\n", b"<title>none</title>".to_vec()));
            let _ = write!(stream, "HTTP/1.1 200 OK\r\n{headers}Content-Length: {}\r\nConnection: close\r\n\r\n", body.len());
            let _ = stream.write_all(&body);
        }
    });
    address
}

/// A right click is the window's menu to show, with what is under it; the link under the
/// pointer is said; finding in the page counts what it found.
#[test]
fn menus_links_under_the_pointer_and_finding() {
    let Some(mut host) = Host::start(&private()) else { return };
    let url = page(concat!(
        "<title>menus</title><body style='margin:0'>",
        "<a href='https://example.invalid/linked' style='position:absolute;left:10px;top:10px;width:200px;height:40px;display:block'>A link</a>",
        "<input style='position:absolute;left:10px;top:80px;width:200px;height:30px'>",
        "<p style='position:absolute;left:10px;top:150px'>needle and needle and NEEDLE</p></body>"
    ));
    host.send(ToHost::Open { tab: 1, url, view: View::new(400, 300, 1.0) });
    host.loaded(1);
    let m = Modifiers::default();
    host.send(ToHost::Pointer { tab: 1, x: 50.0, y: 25.0, button: None, modifiers: m });
    let hovered = host.wait_for("the link under the pointer", 20, |h| matches!(h, Happened::Hovered { tab: 1, link } if !link.is_empty()));
    assert!(matches!(hovered, Happened::Hovered { ref link, .. } if link == "https://example.invalid/linked"), "{hovered:?}");
    for pressed in [true, false] {
        host.send(ToHost::Pointer { tab: 1, x: 50.0, y: 25.0, button: Some((Button::Right, pressed)), modifiers: m });
    }
    let menu = host.wait_for("the menu over the link", 20, |h| matches!(h, Happened::ContextMenu { tab: 1, .. }));
    let Happened::ContextMenu { hit, .. } = menu else { unreachable!() };
    assert_eq!(hit.link.as_deref(), Some("https://example.invalid/linked"));
    assert_eq!(hit.link_text, "A link");
    assert!(!hit.editable);
    for pressed in [true, false] {
        host.send(ToHost::Pointer { tab: 1, x: 50.0, y: 95.0, button: Some((Button::Right, pressed)), modifiers: m });
    }
    let menu = host.wait_for("the menu over the field", 20, |h| matches!(h, Happened::ContextMenu { tab: 1, .. }));
    let Happened::ContextMenu { hit, .. } = menu else { unreachable!() };
    assert!(hit.editable && hit.link.is_none(), "{hit:?}");
    host.send(ToHost::Find { tab: 1, text: "needle".into(), forward: true });
    let found = host.wait_for("what was found", 20, |h| matches!(h, Happened::Found { tab: 1, .. }));
    assert_eq!(found, Happened::Found { tab: 1, matches: 3 });
    host.send(ToHost::Find { tab: 1, text: "haystack".into(), forward: true });
    let found = host.wait_for("nothing found", 20, |h| matches!(h, Happened::Found { tab: 1, .. }));
    assert_eq!(found, Happened::Found { tab: 1, matches: 0 });
    host.send(ToHost::Find { tab: 1, text: String::new(), forward: true });
    host.quit();
}

/// A file the window asked for comes down into the file it named, its news told; and a page's
/// <input type=file> is answered with the files the window gives.
#[test]
fn downloads_and_files_for_a_page() {
    let Some(mut host) = Host::start(&private()) else { return };
    let body: Vec<u8> = (0..200_000u32).map(|i| (i % 251) as u8).collect();
    let address = server(vec![("/file.bin", "Content-Type: application/octet-stream\r\nContent-Disposition: attachment; filename=\"file.bin\"\r\n", body.clone())]);
    let folder = std::env::temp_dir().join(format!("spatimesh-download-{}", std::process::id()));
    let target = folder.join("here.bin");
    let url = page(concat!(
        "<title>files</title><body style='margin:0'>",
        "<input type=file multiple accept='image/png' style='position:absolute;left:10px;top:10px;width:300px;height:40px' ",
        "onchange=\"document.title='chosen:'+this.files.length+':'+this.files[0].name\"></body>"
    ));
    host.send(ToHost::Open { tab: 1, url, view: View::new(400, 300, 1.0) });
    host.loaded(1);
    host.send(ToHost::Download { tab: 1, id: 5, url: format!("{address}/file.bin"), to: target.to_string_lossy().into() });
    let started = host.wait_for("the download to begin", 20, |h| matches!(h, Happened::Download { id: 5, event: spatimesh::engine::Download::Started { .. }, .. }));
    assert!(matches!(started, Happened::Download { tab: 1, event: spatimesh::engine::Download::Started { ref file, .. }, .. } if Path::new(file) == target), "{started:?}");
    let finished = host.wait_for("the download to end", 30, |h| matches!(h, Happened::Download { id: 5, event: spatimesh::engine::Download::Finished { .. } | spatimesh::engine::Download::Failed { .. }, .. }));
    assert!(matches!(finished, Happened::Download { event: spatimesh::engine::Download::Finished { .. }, .. }), "{finished:?}");
    assert_eq!(std::fs::read(&target).unwrap(), body);
    // The page's own file field.
    let picked = folder.join("picked.png");
    std::fs::write(&picked, b"not really a picture").unwrap();
    host.send(ToHost::Focus { tab: 1, focused: true });
    host.click(1, 60.0, 30.0);
    let asked = host.wait_for("the page to ask for files", 20, |h| matches!(h, Happened::ChooseFiles { tab: 1, .. }));
    assert!(matches!(asked, Happened::ChooseFiles { multiple: true, ref accept, .. } if accept.iter().any(|a| a == "image/png")), "{asked:?}");
    host.send(ToHost::Files { tab: 1, files: Some(vec![picked.to_string_lossy().into()]) });
    assert_eq!(host.title(1, "chosen:"), "chosen:1:picked.png");
    let _ = std::fs::remove_dir_all(&folder);
    host.quit();
}

/// A link shown in one tab and visited in another should look visited in the first, by itself,
/// as browsers' tabs do -- and does not yet: WebKit's GLib API (WebKitGTK and WPE alike, 2.54)
/// gives every web view a visited-link store of its own (`webkitWebViewCreatePageConfiguration`
/// never sets one), shared only with the views a page opens with `window.open`. A link looks
/// visited only in the tab that followed it. `related-view` would share it, and the web process
/// with it: one tab's crash would take the others. The fix is WebKit's: one store for a web
/// context's views (docs/spatimesh.md, "Visited links"). This is the test for it.
#[test]
#[ignore = "WebKit keeps visited links per web view: needs the WebKit change in docs/spatimesh.md, \"Visited links\""]
fn a_link_visited_in_another_tab_looks_visited() {
    let root = std::env::temp_dir().join(format!("spatimesh-visited-{}", std::process::id()));
    let Some(mut host) = Host::start(&kept(&root)) else { return };
    let style = "<style>body{margin:0}a{display:block;width:300px;height:80px;background-color:rgb(0,0,255)}a:visited{background-color:rgb(255,0,0)}</style>";
    let first = format!("<title>first</title>{style}<a href='/target'>x</a><a href='/clicked'>y</a>");
    let address = server(vec![
        ("/first", "Content-Type: text/html\r\n", first.into_bytes()),
        ("/target", "Content-Type: text/html\r\n", b"<title>target</title>".to_vec()),
        ("/clicked", "Content-Type: text/html\r\n", b"<title>clicked</title>".to_vec()),
        ("/other", "Content-Type: text/html\r\n", format!("<title>other</title>{style}<a href='/clicked'>y</a>").into_bytes()),
    ]);
    // The colour of a tab's link at `y` once a picture shows one of the two.
    let colour = |host: &mut Host, tab: TabId, y: u32, want: [u8; 3]| {
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            host.wait_for("a picture", 15, |h| matches!(h, Happened::Picture { tab: t } if *t == tab));
            let p = host.pixel(tab, 50, y);
            if near(p, want) {
                return;
            }
            assert!(Instant::now() < deadline, "tab {tab}'s link at {y} is {p:?}, not {want:?}");
        }
    };
    host.send(ToHost::Open { tab: 1, url: format!("{address}/first"), view: View::new(400, 300, 1.0) });
    host.loaded(1);
    colour(&mut host, 1, 40, [0, 0, 255]);
    // Another tab is given the link's address: the first tab's link turns visited.
    host.send(ToHost::Open { tab: 2, url: format!("{address}/target"), view: View::new(400, 300, 1.0) });
    host.title(2, "target");
    host.loaded(2);
    colour(&mut host, 1, 40, [255, 0, 0]);
    // A link followed in a third tab shows visited in the first, and in a fourth opened after.
    host.send(ToHost::Open { tab: 3, url: format!("{address}/other"), view: View::new(400, 300, 1.0) });
    host.loaded(3);
    host.click(3, 50.0, 40.0);
    host.title(3, "clicked");
    colour(&mut host, 1, 120, [255, 0, 0]);
    host.send(ToHost::Open { tab: 4, url: format!("{address}/first"), view: View::new(400, 300, 1.0) });
    host.loaded(4);
    colour(&mut host, 4, 120, [255, 0, 0]);
    host.quit();
    let _ = std::fs::remove_dir_all(&root);
}
