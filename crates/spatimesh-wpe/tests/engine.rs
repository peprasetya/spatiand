//! The WPE engine host as SpatiMesh runs it: a process of its own, spoken to over the
//! wire, drawing into shared memory, with real pages in WebKit's own processes.
//!
//! The WebKitGTK engine's tests (crates/spatimesh-webkitgtk), run against WPE: the same
//! interface, the same behaviour asked of it. Where the program was built without WPE, they
//! say so and pass.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::Child;
use std::time::{Duration, Instant};

use spatimesh::engine::{Button, Cookie, DialogKind, Format, Happened, KeyPress, Modifiers, ProfileSetup, TabId, View};
use spatimesh::wire::{FromHost, Link, Pictures, ToHost};

const PROGRAM: &str = env!("CARGO_BIN_EXE_spatimesh-wpe");

/// The engine program to test: this crate's, or another copy named by SPATIMESH_WPE_PROGRAM,
/// such as the one in the app, to see that the app's copy needs nothing from the build folder.
fn program() -> PathBuf {
    std::env::var_os("SPATIMESH_WPE_PROGRAM").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(PROGRAM))
}

struct Host {
    child: Child,
    link: Link,
    pictures: HashMap<TabId, Pictures>,
    /// Each tab's newest picture, copied out: (width, height, format, bytes).
    latest: HashMap<TabId, (u32, u32, Format, Vec<u8>)>,
    seen: Vec<Happened>,
}

/// An IOSurface's picture, by its number (BGRA): what the window would show.
#[cfg(target_os = "macos")]
fn read_surface(id: u32) -> Option<(u32, u32, Format, Vec<u8>)> {
    use std::ffi::c_void;
    #[link(name = "IOSurface", kind = "framework")]
    extern "C" {
        fn IOSurfaceLookup(id: u32) -> *mut c_void;
        fn IOSurfaceLock(surface: *mut c_void, options: u32, seed: *mut u32) -> i32;
        fn IOSurfaceUnlock(surface: *mut c_void, options: u32, seed: *mut u32) -> i32;
        fn IOSurfaceGetWidth(surface: *mut c_void) -> usize;
        fn IOSurfaceGetHeight(surface: *mut c_void) -> usize;
        fn IOSurfaceGetBytesPerRow(surface: *mut c_void) -> usize;
        fn IOSurfaceGetBaseAddress(surface: *mut c_void) -> *const u8;
    }
    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        fn CFRelease(object: *mut c_void);
    }
    unsafe {
        let surface = IOSurfaceLookup(id);
        if surface.is_null() {
            return None;
        }
        IOSurfaceLock(surface, 1, std::ptr::null_mut());
        let (width, height, stride) = (IOSurfaceGetWidth(surface), IOSurfaceGetHeight(surface), IOSurfaceGetBytesPerRow(surface));
        let base = IOSurfaceGetBaseAddress(surface);
        let mut pixels = Vec::with_capacity(width * height * 4);
        for row in 0..height {
            pixels.extend_from_slice(std::slice::from_raw_parts(base.add(row * stride), width * 4));
        }
        IOSurfaceUnlock(surface, 1, std::ptr::null_mut());
        CFRelease(surface);
        Some((width as u32, height as u32, Format::Bgra, pixels))
    }
}

#[cfg(not(target_os = "macos"))]
fn read_surface(_id: u32) -> Option<(u32, u32, Format, Vec<u8>)> {
    None
}

fn can_run() -> bool {
    // WPE draws through SpatiMesh's own platform, on no display.
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
        let (child, link) = spatimesh::host::start(&program(), setup).expect("the engine program starts");
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
                    // A Mac's pictures come on the GPU, as IOSurfaces: read here, as the window shows them.
                    FromHost::Surface { tab, id, .. } => {
                        if let Some(picture) = read_surface(id) {
                            self.latest.insert(tab, picture);
                            self.seen.push(Happened::Picture { tab });
                        }
                        self.send(ToHost::ReleaseSurface { tab, id });
                    }
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

/// Every process under `pid`, by its name: from `ps`, on the Mac and Linux alike.
fn descendants(pid: u32) -> Vec<(u32, String)> {
    let out = std::process::Command::new("ps").args(["-A", "-o", "pid=,ppid=,comm="]).output().unwrap();
    let all: Vec<(u32, u32, String)> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|line| {
            let mut f = line.split_whitespace();
            let p = f.next()?.parse().ok()?;
            let parent = f.next()?.parse().ok()?;
            let name = f.collect::<Vec<_>>().join(" ");
            Some((p, parent, name.rsplit('/').next().unwrap_or("").to_string()))
        })
        .collect();
    let mut found = Vec::new();
    let mut parents = vec![pid];
    while let Some(parent) = parents.pop() {
        for (p, pp, name) in &all {
            if *pp == parent {
                found.push((*p, name.clone()));
                parents.push(*p);
            }
        }
    }
    found
}

/// WPE's web process (WebKitGTK's is "WebKitWebProcess").
fn is_web_process(name: &str) -> bool {
    name.starts_with("WPEWebProcess") || name.starts_with("WebKitWebProces")
}

#[test]
fn a_crashed_page_is_told_and_the_rest_go_on() {
    let Some(mut host) = Host::start(&private()) else { return };
    host.send(ToHost::Open { tab: 1, url: page("<title>one</title>"), view: View::new(200, 100, 1.0) });
    host.title(1, "one");
    host.loaded(1);
    let before: Vec<u32> = descendants(host.child.id()).into_iter().filter(|(_, n)| is_web_process(n)).map(|(p, _)| p).collect();
    host.send(ToHost::Open { tab: 2, url: page("<title>two</title>"), view: View::new(200, 100, 1.0) });
    host.title(2, "two");
    host.loaded(2);
    // The second tab's own process: the one that was not there before it.
    let second: Vec<u32> = descendants(host.child.id())
        .into_iter()
        .filter(|(p, n)| is_web_process(n) && !before.contains(p))
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

#[test]
fn a_file_on_disk_is_shown() {
    let Some(mut host) = Host::start(&private()) else { return };
    let folder = std::env::temp_dir().join(format!("spatimesh-wpe-file-{}", std::process::id()));
    std::fs::create_dir_all(&folder).unwrap();
    let file = folder.join("page.html");
    std::fs::write(&file, "<title>from disk</title><p>here</p>").unwrap();
    host.send(ToHost::Open { tab: 1, url: format!("file://{}", file.display()), view: View::new(200, 100, 1.0) });
    host.title(1, "from disk");
    host.quit();
    let _ = std::fs::remove_dir_all(folder);
}
