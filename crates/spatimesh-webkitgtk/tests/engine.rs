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

use spatimesh::engine::{Button, DialogKind, Format, Happened, KeyPress, Modifiers, ProfileSetup, TabId, View};
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

