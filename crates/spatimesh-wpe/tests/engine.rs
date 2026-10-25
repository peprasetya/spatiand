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

/// An IOSurface's picture, handed over by the engine as this number (BGRA): what the window
/// would show. Not global: only this process, which listened before it started the engine,
/// can open it.
#[cfg(target_os = "macos")]
fn read_surface(id: u32) -> Option<(u32, u32, Format, Vec<u8>)> {
    use std::ffi::c_void;
    #[link(name = "IOSurface", kind = "framework")]
    extern "C" {
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
        let surface = spatiand_surfaces::open(id) as *mut c_void;
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

/// Held by the tests that set the process's environment for the engines they start (the
/// session's sound socket), and by those that read what it decides, so they take turns.
static ENVIRONMENT: std::sync::Mutex<()> = std::sync::Mutex::new(());

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
        // As SpatiMesh's window does: the engine hands its pictures to whoever started it.
        spatiand_surfaces::listen();
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
            self.receive();
        }
    }

    /// What the engine says in the next moment, kept.
    fn receive(&mut self) {
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

    /// Everything the engine says for `time`, kept, whatever it is.
    fn settle(&mut self, time: Duration) {
        let until = Instant::now() + time;
        while Instant::now() < until && !self.link.is_closed() {
            self.receive();
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

    /// The tab's newest picture as a PPM file where SPATIMESH_TEST_SHOTS names a folder, to be
    /// looked at.
    /// The last title the tab said, if any came while waiting.
    fn latest_title(&mut self, tab: TabId) -> Option<String> {
        self.receive();
        self.seen.iter().rev().find_map(|h| match h { Happened::Title { tab: t, title } if *t == tab => Some(title.clone()), _ => None })
    }

    fn shot(&self, tab: TabId, name: &str) {
        let Some(folder) = std::env::var_os("SPATIMESH_TEST_SHOTS") else { return };
        let (width, height, ..) = self.latest[&tab];
        let mut out = format!("P6 {width} {height} 255\n").into_bytes();
        for y in 0..height {
            for x in 0..width {
                out.extend_from_slice(&self.pixel(tab, x, y));
            }
        }
        let _ = std::fs::write(Path::new(&folder).join(format!("{name}.ppm")), out);
    }

    fn click(&mut self, tab: TabId, x: f32, y: f32) {
        let m = Modifiers::default();
        self.send(ToHost::Pointer { tab, x, y, button: None, modifiers: m });
        self.send(ToHost::Pointer { tab, x, y, button: Some((Button::Left, true)), modifiers: m });
        self.send(ToHost::Pointer { tab, x, y, button: Some((Button::Left, false)), modifiers: m });
    }

    /// As the room types: the key is its unshifted letter, Shift held for a capital, and what
    /// it typed beside.
    fn type_text(&mut self, tab: TabId, text: &str) {
        for ch in text.chars() {
            let modifiers = Modifiers { shift: ch.is_uppercase(), ..Modifiers::default() };
            let key_char = ch.to_lowercase().next().unwrap_or(ch);
            for pressed in [true, false] {
                let key = KeyPress { code: 0, key: spatiand_app::Key::Char(key_char), text: Some(ch.to_string()), pressed, repeat: false, modifiers };
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

/// A password field takes what is typed, as any other: WPE is given the keys by SpatiMesh, with
/// no window of the Mac's to have them, so macOS's secure input has nothing to take them from.
#[test]
fn a_password_is_typed() {
    let Some(mut host) = Host::start(&private()) else { return };
    let url = page(concat!(
        "<input id=u style='position:absolute;left:10px;top:10px;width:200px;height:30px'>",
        "<input id=p type=password style='position:absolute;left:10px;top:60px;width:200px;height:30px' ",
        "onfocus=\"document.title='focused'\" oninput=\"document.title='typed:'+this.value\">"
    ));
    host.send(ToHost::Open { tab: 1, url, view: View::new(400, 300, 1.0) });
    host.loaded(1);
    host.send(ToHost::Focus { tab: 1, focused: true });
    host.click(1, 50.0, 75.0);
    host.title(1, "focused");
    host.type_text(1, "S3cReT");
    host.title(1, "typed:S3cReT");
    host.quit();
}

/// WebGL: WebKit's own ANGLE, drawing through the one the rest of WPE uses.
#[test]
fn webgl_draws() {
    let Some(mut host) = Host::start(&private()) else { return };
    let url = page(concat!(
        "<body style='margin:0'><canvas id=c width=100 height=100></canvas><script>",
        "const gl=document.getElementById('c').getContext('webgl2')||document.getElementById('c').getContext('webgl');",
        "if(!gl){document.title='webgl:none'}else{gl.clearColor(0,0,1,1);gl.clear(gl.COLOR_BUFFER_BIT);",
        "const p=new Uint8Array(4);gl.readPixels(50,50,1,1,gl.RGBA,gl.UNSIGNED_BYTE,p);",
        "document.title='webgl:'+p[0]+','+p[1]+','+p[2]}</script></body>"
    ));
    host.send(ToHost::Open { tab: 1, url, view: View::new(200, 200, 1.0) });
    let title = host.title(1, "webgl:");
    assert_eq!(title, "webgl:0,0,255");
    // And it is on the page: WebGL's picture is composited into the one SpatiMesh is given.
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        host.wait_for("a picture", 20, |h| matches!(h, Happened::Picture { tab: 1 }));
        if near(host.pixel(1, 50, 50), [0, 0, 255]) {
            break;
        }
        assert!(Instant::now() < deadline, "the canvas never showed: {:?}", host.pixel(1, 50, 50));
    }
    host.quit();
}

/// The developer tools: a tab of their own, drawn, that goes when their page does.
#[test]
fn developer_tools_are_a_tab() {
    let Some(mut host) = Host::start(&private()) else { return };
    host.send(ToHost::Open { tab: 1, url: page("<title>inspected</title><p id=x>hello</p>"), view: View::new(400, 300, 1.0) });
    host.loaded(1);
    host.send(ToHost::Inspect { tab: 1, inspector: 2, view: View::new(800, 500, 1.0) });
    // Their own page draws: wait for a picture of theirs with more than one colour in it.
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        host.wait_for("a picture of the developer tools", 30, |h| matches!(h, Happened::Picture { tab: 2 }));
        let (width, height, ..) = host.latest[&2];
        let sample: Vec<[u8; 3]> = (0..20).map(|i| host.pixel(2, width * i / 20, height / 3)).collect();
        if (width, height) == (800, 500) && sample.iter().any(|p| !near(*p, sample[0])) {
            break;
        }
        assert!(Instant::now() < deadline, "the developer tools never drew: {width}x{height}");
    }
    host.shot(2, "developer-tools");
    // Still open: WebKit moving their view between windows as it sets them up is not closing.
    host.settle(Duration::from_secs(2));
    assert!(!host.seen.iter().any(|h| matches!(h, Happened::CloseTab { tab: 2 })), "the developer tools closed by themselves: {:?}", host.seen);
    // Closing the page closes them.
    host.send(ToHost::Close { tab: 1 });
    host.wait_for("the developer tools to close", 20, |h| matches!(h, Happened::CloseTab { tab: 2 }));
    host.quit();
}

/// Video, through WPE's GStreamer player (decoding by gst-libav on Spatiand's FFmpeg, or
/// VideoToolbox): it plays, and its picture is on the page.
#[test]
fn a_video_plays() {
    let Some(mut host) = Host::start(&private()) else { return };
    let media = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/media/red.mp4");
    let folder = std::env::temp_dir().join(format!("spatimesh-wpe-video-{}", std::process::id()));
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::copy(&media, folder.join("red.mp4")).unwrap();
    let file = folder.join("page.html");
    std::fs::write(&file, concat!(
        "<body style='margin:0;background:#000'><video id=v src=red.mp4 muted autoplay loop ",
        "style='width:320px;height:240px' onplaying=\"document.title='playing'\" ",
        "onerror=\"document.title='error:'+(this.error&&this.error.code)\"></video></body>"
    )).unwrap();
    host.send(ToHost::Open { tab: 1, url: format!("file://{}", file.display()), view: View::new(400, 300, 1.0) });
    let title = host.title(1, "");
    let title = if title == "playing" || title.starts_with("error") { title } else { host.title(1, "p") };
    assert_eq!(title, "playing");
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        host.wait_for("a picture", 20, |h| matches!(h, Happened::Picture { tab: 1 }));
        let p = host.pixel(1, 160, 120);
        if p[0] > 200 && p[1] < 60 && p[2] < 60 {
            break;
        }
        if Instant::now() >= deadline {
            host.shot(1, "video");
        }
        assert!(Instant::now() < deadline, "the video's picture never showed: {p:?}");
    }
    host.quit();
    let _ = std::fs::remove_dir_all(folder);
}

/// Emoji in colour, and a reflection (-webkit-box-reflect), as Safari draws them: looked at by
/// eye (SPATIMESH_TEST_SHOTS), and the emoji found not blank.
#[test]
fn emoji_and_reflections() {
    let Some(mut host) = Host::start(&private()) else { return };
    let url = page(concat!(
        "<body style='margin:0;background:#fff;font-size:48px'>",
        "<div style='position:absolute;left:10px;top:10px'>\u{1F600}\u{1F3AC}\u{2764}\u{FE0F}\u{1F44D}</div>",
        "<div style='position:absolute;left:10px;top:90px;width:120px;height:60px;background:linear-gradient(red,blue);",
        "-webkit-box-reflect:below 4px linear-gradient(transparent,rgba(0,0,0,0.6))'></div>",
        // As Vibrefy has them: on a layer of their own (translateZ), and emoji in an SVG image.
        "<div style='position:absolute;left:200px;top:90px;width:120px;height:60px;background:linear-gradient(green,orange);",
        "transform:translateZ(0);-webkit-box-reflect:below 4px linear-gradient(transparent,rgba(0,0,0,0.6))'></div>",
        "<div style=\"position:absolute;left:200px;top:10px;width:160px;height:70px;background-size:contain;background-repeat:no-repeat;",
        "background-image:url(&quot;data:image/svg+xml,%3Csvg viewBox='0 0 160 90' xmlns='http://www.w3.org/2000/svg'%3E%3Ctext x='50%' y='90%' font-size='80' text-anchor='middle'%3E\u{1F3AC}%3C/text%3E%3C/svg%3E&quot;)\"></div></body>"
    ));
    host.send(ToHost::Open { tab: 1, url, view: View::new(400, 260, 1.0) });
    host.loaded(1);
    host.settle(Duration::from_secs(2));
    host.wait_for("a picture", 20, |h| matches!(h, Happened::Picture { tab: 1 }));
    host.shot(1, "emoji-reflection");
    // The smiling face's yellow, somewhere in its box.
    let yellow = (10..60).flat_map(|x| (12..62).map(move |y| (x, y))).any(|(x, y)| {
        let p = host.pixel(1, x, y);
        p[0] > 200 && p[1] > 150 && p[2] < 100
    });
    assert!(yellow, "the emoji is not drawn in colour");
    host.quit();
}

/// Emoji larger than the colour font's biggest picture (160 pixels), as a poster's placeholder
/// is on a large window: scaled from it, not a missing glyph.
#[test]
fn large_emoji() {
    let Some(mut host) = Host::start(&private()) else { return };
    let url = page("<body style='margin:0;background:#fff;font-size:240px;line-height:1'>\u{1F600}</body>");
    host.send(ToHost::Open { tab: 1, url, view: View::new(400, 300, 1.0) });
    host.loaded(1);
    host.settle(Duration::from_secs(2));
    host.wait_for("a picture", 20, |h| matches!(h, Happened::Picture { tab: 1 }));
    host.shot(1, "large-emoji");
    let yellow = (20..200).step_by(4).flat_map(|x| (20..200).step_by(4).map(move |y| (x, y))).filter(|&(x, y)| {
        let p = host.pixel(1, x, y);
        p[0] > 200 && p[1] > 150 && p[2] < 100
    }).count();
    assert!(yellow > 50, "a large emoji is not drawn in colour ({yellow} yellow points)");
    host.quit();
}

/// Pages are told light or dark as the platform is (prefers-color-scheme): on a Mac, its
/// Appearance setting; and form controls are in the platform's interface font.
#[test]
fn the_platform_look() {
    let _turn = ENVIRONMENT.lock().unwrap_or_else(|e| e.into_inner());
    let Some(mut host) = Host::start(&private()) else { return };
    let url = page(concat!(
        "<button id=b>OK</button><script>",
        "document.title='scheme:'+(matchMedia('(prefers-color-scheme: dark)').matches?'dark':'light')",
        "+' font:'+getComputedStyle(document.getElementById('b')).fontFamily</script>"
    ));
    host.send(ToHost::Open { tab: 1, url, view: View::new(300, 200, 1.0) });
    let title = host.title(1, "scheme:");
    #[cfg(target_os = "macos")]
    {
        // The wearer's choice in Spatiand's settings first, then the Mac's.
        let config = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap()).join(".config"));
        let chosen = std::fs::read_to_string(config.join("spatiand/prefs.toml")).ok().and_then(|t| {
            t.lines().find(|l| l.trim_start().starts_with("look")).map(|l| l.contains("\"dark\"") as u8 + 2 * l.contains("\"light\"") as u8)
        });
        let dark = match chosen {
            Some(1) => true,
            Some(2) => false,
            _ => std::process::Command::new("defaults").args(["read", "-g", "AppleInterfaceStyle"]).output()
                .is_ok_and(|o| String::from_utf8_lossy(&o.stdout).trim().eq_ignore_ascii_case("dark")),
        };
        assert!(title.starts_with(if dark { "scheme:dark" } else { "scheme:light" }), "{title}");
    }
    eprintln!("{title}");
    host.quit();
}

/// A page's sound goes to the Spatiand session (its `spatiand-sound` socket, under the window's
/// application id), to be placed in the room through the HRTF: here, an audio element's tone,
/// under its tab's own name.
#[test]
fn sound_goes_to_the_session() {
    let _turn = ENVIRONMENT.lock().unwrap_or_else(|e| e.into_inner());
    use std::os::unix::net::UnixListener;
    let dir = std::env::temp_dir().join(format!("spatimesh-wpe-sound-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let listener = UnixListener::bind(dir.join("spatiand-sound")).unwrap();
    // The engine inherits these, as from SpatiMesh's window in a session.
    std::env::set_var("XDG_RUNTIME_DIR", &dir);
    std::env::set_var("SPATIAND_APP_ID", "id.prasetya.spatiand.spatimesh");
    let Some(mut host) = Host::start(&private()) else { return };
    // A second of a tone, as a WAV beside the page.
    let mut wav = Vec::new();
    let samples: Vec<i16> = (0..48000).map(|i| ((i as f32 / 10.0).sin() * 9000.0) as i16).collect();
    let data = (samples.len() * 2) as u32;
    for part in [&b"RIFF"[..], &(36 + data).to_le_bytes(), b"WAVEfmt ", &16u32.to_le_bytes(), &1u16.to_le_bytes(),
        &1u16.to_le_bytes(), &48000u32.to_le_bytes(), &96000u32.to_le_bytes(), &2u16.to_le_bytes(), &16u16.to_le_bytes(),
        b"data", &data.to_le_bytes()] { wav.extend_from_slice(part); }
    for s in &samples { wav.extend_from_slice(&s.to_le_bytes()); }
    std::fs::write(dir.join("tone.wav"), wav).unwrap();
    let file = dir.join("page.html");
    std::fs::write(&file, "<body style='margin:0;height:100vh' onclick='a.play()'>\
        <audio id=a src=tone.wav loop onplaying=\"document.title='tone:playing'\" \
        onerror=\"document.title='tone:error'\"></audio></body>").unwrap();
    host.send(ToHost::Open { tab: 1, url: format!("file://{}", file.display()), view: View::new(200, 100, 1.0) });
    host.settle(Duration::from_secs(1));
    // Sound plays after a click on the page, as WebKit's autoplay rule has it.
    host.click(1, 50.0, 50.0);
    eprintln!("{}", host.title(1, "tone:"));
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        // The sink first only looks whether anyone is there (a connection that says nothing).
        let (mut s, head) = loop {
            let (mut s, _) = listener.accept().unwrap();
            let mut head = [0u8; 12];
            if s.read_exact(&mut head).is_ok() {
                break (s, head);
            }
        };
        let mut app = vec![0u8; u16::from_le_bytes([head[10], head[11]]) as usize];
        s.read_exact(&mut app).unwrap();
        let mut sound = vec![0u8; 48000];
        s.read_exact(&mut sound).unwrap();
        let loudest = sound.chunks(2).map(|b| i16::from_le_bytes([b[0], b[1]]).unsigned_abs()).max().unwrap();
        tx.send((head, String::from_utf8(app).unwrap(), loudest)).unwrap();
    });
    let deadline = Instant::now() + Duration::from_secs(20);
    let (head, app, loudest) = loop {
        if let Ok(got) = rx.try_recv() { break got; }
        assert!(Instant::now() < deadline, "no sound reached the session");
        host.link.wait_readable(100);
        host.receive();
    };
    assert_eq!(&head[..4], b"SPSN");
    assert_eq!(u32::from_le_bytes(head[6..10].try_into().unwrap()), 48000);
    // Under the tab's own name, which the engine said, for the window to route to its place.
    let said = host.wait_for("the tab's sound name", 5, |h| matches!(h, Happened::SoundName { tab: 1, .. }));
    let Happened::SoundName { name, .. } = said else { unreachable!() };
    assert!(name.starts_with("id.prasetya.spatiand.spatimesh/page"), "{name}");
    assert_eq!(app, name);
    assert!(loudest > 1000, "silence: {loudest}");
    eprintln!("channels {} loudest {loudest}", u16::from_le_bytes([head[4], head[5]]));
    host.quit();
    let _ = std::fs::remove_dir_all(dir);
}

/// A sign-in window, as an OpenID login opens one: the page opened by window.open is a tab of
/// its own that still knows its opener -- it posts a message back and closes itself.
#[test]
fn a_window_opened_tells_its_opener_and_closes() {
    let Some(mut host) = Host::start(&private()) else { return };
    let dir = std::env::temp_dir().join(format!("spatimesh-wpe-opener-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("login.html"), "<script>window.opener.postMessage('signed in','*');window.close()</script>").unwrap();
    std::fs::write(dir.join("main.html"), "<body style='margin:0;height:100vh' onclick=\"window.open('login.html','login','width=400,height=300')\">\
        <script>addEventListener('message',e=>document.title='told:'+e.data)</script></body>").unwrap();
    host.send(ToHost::Open { tab: 1, url: format!("file://{}", dir.join("main.html").display()), view: View::new(400, 300, 1.0) });
    host.settle(Duration::from_secs(1));
    host.click(1, 50.0, 50.0);
    // As SpatiMesh's window does: a tab of its own for what the page opened.
    let opened = host.wait_for("the page opening a window", 20, |h| matches!(h, Happened::OpenTab { opener: 1, .. }));
    let Happened::OpenTab { url, .. } = opened else { unreachable!() };
    host.send(ToHost::Open { tab: 2, url, view: View::new(400, 300, 1.0) });
    assert_eq!(host.title(1, "told:"), "told:signed in");
    host.wait_for("the opened window closing itself", 20, |h| matches!(h, Happened::CloseTab { tab: 2 }));
    host.quit();
    let _ = std::fs::remove_dir_all(dir);
}

/// An emoji drawn as SVG text in a CSS background, as Vibrefy draws every picture it has none
/// for: in colour, not a box.
#[test]
fn an_emoji_in_a_css_background() {
    let Some(mut host) = Host::start(&private()) else { return };
    let url = page(concat!(
        "<div style=\"width:320px;height:180px;background-size:contain;background-repeat:no-repeat;",
        "background-image:url(&quot;data:image/svg+xml,%3Csvg viewBox='0 0 160 90' xmlns='http://www.w3.org/2000/svg'%3E",
        "%3Ctext x='50%' y='90%' font-size='80' text-anchor='middle'%3E\u{1F3AC}%3C/text%3E%3C/svg%3E&quot;)\"></div>",
        "<script>onload=()=>document.title='drawn'</script>"
    ));
    host.send(ToHost::Open { tab: 1, url, view: View::new(320, 180, 1.0) });
    host.title(1, "drawn");
    host.settle(Duration::from_secs(1));
    host.shot(1, "css-emoji");
    // The clapperboard is a large dark shape; a missing glyph is a thin outlined box.
    let mut dark = 0;
    for y in (0..180).step_by(4) {
        for x in (0..320).step_by(4) {
            if host.pixel(1, x, y).iter().all(|c| *c < 90) {
                dark += 1;
            }
        }
    }
    assert!(dark > 150, "no emoji drawn: {dark} dark samples");
    host.quit();
}

/// A picture of any page, by hand: SPATIMESH_LOOK_AT (a file or an address),
/// SPATIMESH_LOOK_WAIT seconds, into SPATIMESH_TEST_SHOTS/look.ppm.
#[test]
#[ignore]
fn look_at_a_file() {
    // SPATIMESH_LOOK_PROFILE: a copy of a profile's data, to look as someone signed in does.
    let setup = std::env::var_os("SPATIMESH_LOOK_PROFILE").map_or_else(private, |root| kept(Path::new(&root)));
    let Some(mut host) = Host::start(&setup) else { return };
    let at = std::env::var("SPATIMESH_LOOK_AT").unwrap();
    let url = if at.contains("://") { at } else { format!("file://{at}") };
    let zoom = std::env::var("SPATIMESH_LOOK_ZOOM").ok().and_then(|z| z.parse().ok()).unwrap_or(1.0);
    host.send(ToHost::Open { tab: 1, url, view: View::new(1100, 700, zoom) });
    let wait = std::env::var("SPATIMESH_LOOK_WAIT").ok().and_then(|w| w.parse().ok()).unwrap_or(2);
    host.settle(Duration::from_secs(wait));
    // SPATIMESH_LOOK_CLICK=x,y clicks there, then waits as long again.
    if let Some((x, y)) = std::env::var("SPATIMESH_LOOK_CLICK").ok().and_then(|c| c.split_once(',').map(|(x, y)| (x.parse().unwrap(), y.parse().unwrap()))) {
        host.click(1, x, y);
        host.settle(Duration::from_secs(wait));
    }
    host.shot(1, "look");
    if let Some(title) = host.latest_title(1) {
        eprintln!("title: {title}");
    }
    host.quit();
}

/// After a sign-in window its page opened has closed (it shared the page's web process), the
/// page's sound is still handed over under the page's own name, which its window routes.
#[test]
fn sound_after_a_sign_in_window_is_still_the_pages() {
    let _turn = ENVIRONMENT.lock().unwrap_or_else(|e| e.into_inner());
    use std::os::unix::net::UnixListener;
    let dir = PathBuf::from(format!("/tmp/smw-popup-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let listener = UnixListener::bind(dir.join("spatiand-sound")).unwrap();
    std::env::set_var("XDG_RUNTIME_DIR", &dir);
    std::env::set_var("SPATIAND_APP_ID", "id.prasetya.spatiand.spatimesh");
    let Some(mut host) = Host::start(&private()) else { return };
    let mut wav = Vec::new();
    let samples: Vec<i16> = (0..48000).map(|i| ((i as f32 / 10.0).sin() * 9000.0) as i16).collect();
    let data = (samples.len() * 2) as u32;
    for part in [&b"RIFF"[..], &(36 + data).to_le_bytes(), b"WAVEfmt ", &16u32.to_le_bytes(), &1u16.to_le_bytes(),
        &1u16.to_le_bytes(), &48000u32.to_le_bytes(), &96000u32.to_le_bytes(), &2u16.to_le_bytes(), &16u16.to_le_bytes(),
        b"data", &data.to_le_bytes()] { wav.extend_from_slice(part); }
    for s in &samples { wav.extend_from_slice(&s.to_le_bytes()); }
    std::fs::write(dir.join("tone.wav"), wav).unwrap();
    std::fs::write(dir.join("login.html"), "<script>window.opener.postMessage('in','*');window.close()</script>").unwrap();
    std::fs::write(dir.join("main.html"), "<body style='margin:0;height:100vh' onclick=\"signed?a.play():window.open('login.html')\">\
        <audio id=a src=tone.wav loop></audio><script>var signed=false;addEventListener('message',()=>{signed=true;document.title='signed in'})</script></body>").unwrap();
    host.send(ToHost::Open { tab: 1, url: format!("file://{}", dir.join("main.html").display()), view: View::new(400, 300, 1.0) });
    let said = host.wait_for("the tab's sound name", 10, |h| matches!(h, Happened::SoundName { tab: 1, .. }));
    let Happened::SoundName { name, .. } = said else { unreachable!() };
    host.settle(Duration::from_secs(1));
    host.click(1, 50.0, 50.0);
    let opened = host.wait_for("the sign-in window", 20, |h| matches!(h, Happened::OpenTab { opener: 1, .. }));
    let Happened::OpenTab { url, .. } = opened else { unreachable!() };
    host.send(ToHost::Open { tab: 2, url, view: View::new(400, 300, 1.0) });
    host.title(1, "signed in");
    host.wait_for("the sign-in window closing", 20, |h| matches!(h, Happened::CloseTab { tab: 2 }));
    host.send(ToHost::Close { tab: 2 });
    host.settle(Duration::from_secs(1));
    host.click(1, 50.0, 50.0);
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || loop {
        let (mut s, _) = listener.accept().unwrap();
        let mut head = [0u8; 12];
        if s.read_exact(&mut head).is_err() {
            continue;
        }
        let mut app = vec![0u8; u16::from_le_bytes([head[10], head[11]]) as usize];
        s.read_exact(&mut app).unwrap();
        let _ = tx.send(String::from_utf8(app).unwrap());
        return;
    });
    let deadline = Instant::now() + Duration::from_secs(20);
    let heard = loop {
        if let Ok(got) = rx.try_recv() { break got; }
        assert!(Instant::now() < deadline, "no sound reached the session");
        host.link.wait_readable(100);
        host.receive();
    };
    assert_eq!(heard, name, "the page's sound under the page's own name");
    host.quit();
    let _ = std::fs::remove_dir_all(dir);
}
