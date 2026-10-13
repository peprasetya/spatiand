//! DLNA: whatever on the network says it has films, browsed and played. A DLNA server keeps
//! nobody's place, so nothing is told to it.
//!
//! Finding one is a question shouted to the whole network (SSDP) and whoever answers;
//! browsing one is a SOAP call whose answer is XML inside XML.

use std::net::UdpSocket;
use std::time::{Duration, Instant};

use super::{Item, Picture, Play, Source};

/// A server found on the network.
#[derive(Debug, Clone, PartialEq)]
pub struct Found {
    pub name: String,
    /// Where its content directory takes its calls.
    pub control: String,
}

pub struct Server {
    found: Found,
    agent: ureq::Agent,
}

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new().timeout_connect(Duration::from_secs(4)).timeout_read(Duration::from_secs(15)).build()
}

const DIRECTORY: &str = "urn:schemas-upnp-org:service:ContentDirectory:1";

/// Everything on the network that answers within `wait` to being asked for a media server.
pub fn discover(wait: Duration) -> Vec<Found> {
    let Ok(socket) = UdpSocket::bind("0.0.0.0:0") else { return Vec::new() };
    let _ = socket.set_read_timeout(Some(Duration::from_millis(250)));
    let question = "M-SEARCH * HTTP/1.1\r\nHOST: 239.255.255.250:1900\r\nMAN: \"ssdp:discover\"\r\nMX: 2\r\nST: urn:schemas-upnp-org:device:MediaServer:1\r\n\r\n";
    // Twice: it is UDP, and one may be lost.
    for _ in 0..2 {
        let _ = socket.send_to(question.as_bytes(), "239.255.255.250:1900");
    }
    let mut locations: Vec<String> = Vec::new();
    let until = Instant::now() + wait;
    let mut buffer = [0u8; 2048];
    while Instant::now() < until {
        let Ok((length, _)) = socket.recv_from(&mut buffer) else { continue };
        let answer = String::from_utf8_lossy(&buffer[..length]);
        let location = answer.lines().find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.trim().eq_ignore_ascii_case("location").then(|| value.trim().to_string())
        });
        if let Some(location) = location.filter(|l| !locations.contains(l)) {
            locations.push(location);
        }
    }
    let agent = agent();
    let mut found: Vec<Found> = locations.iter().filter_map(|location| describe(&agent, location)).collect();
    found.sort_by_key(|f| f.name.to_lowercase());
    found.dedup_by(|a, b| a.control == b.control);
    found
}

/// What a server says of itself at `location`: its name, and where its directory is.
fn describe(agent: &ureq::Agent, location: &str) -> Option<Found> {
    let text = agent.get(location).call().ok()?.into_string().ok()?;
    described(&text, location)
}

pub fn described(text: &str, location: &str) -> Option<Found> {
    let document = roxmltree::Document::parse(text).ok()?;
    let named = |node: roxmltree::Node, name: &str| node.children().find(|c| c.tag_name().name() == name).and_then(|c| c.text()).map(|t| t.trim().to_string());
    let name = document.descendants().find(|n| n.tag_name().name() == "friendlyName")?.text()?.trim().to_string();
    let service = document.descendants().find(|n| n.tag_name().name() == "service" && named(*n, "serviceType").as_deref() == Some(DIRECTORY))?;
    let control = named(service, "controlURL")?;
    // Relative to the description's base, or to where the description was.
    let base = document.descendants().find(|n| n.tag_name().name() == "URLBase").and_then(|n| n.text()).map(str::trim).unwrap_or(location);
    Some(Found { name, control: join(base, &control) })
}

/// `path` as an address, given the address of the thing that named it.
fn join(base: &str, path: &str) -> String {
    if path.contains("://") {
        return path.to_string();
    }
    let (scheme, rest) = base.split_once("://").unwrap_or(("http", base));
    let host = rest.split('/').next().unwrap_or(rest);
    if path.starts_with('/') {
        format!("{scheme}://{host}{path}")
    } else {
        let folder = rest.rsplit_once('/').map_or(host, |(folder, _)| folder);
        format!("{scheme}://{folder}/{path}")
    }
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// What a `Browse` answered with, as items. The answer's `Result` is a document of its own,
/// carried as text.
pub fn browsed(answer: &str) -> Result<Vec<Item>, String> {
    let envelope = roxmltree::Document::parse(answer).map_err(|e| format!("the server's answer was not XML: {e}"))?;
    let result = envelope.descendants().find(|n| n.tag_name().name() == "Result").and_then(|n| n.text()).ok_or("the server's answer had nothing in it")?;
    let listing = roxmltree::Document::parse(result).map_err(|e| format!("the server's list was not XML: {e}"))?;
    let child = |node: roxmltree::Node, name: &str| node.children().find(|c| c.tag_name().name() == name).and_then(|c| c.text()).map(|t| t.trim().to_string());
    let mut items = Vec::new();
    for node in listing.root_element().children().filter(|n| n.is_element()) {
        let Some(id) = node.attribute("id") else { continue };
        let title = child(node, "title").unwrap_or_default();
        match node.tag_name().name() {
            "container" => items.push(Item { key: format!("c{id}"), title: title.clone(), name: title, folder: true, ..Item::default() }),
            "item" => {
                // The film itself: the first resource that is video, which is the original
                // where a server also offers smaller ones.
                let resource = node.children().find(|c| c.tag_name().name() == "res" && c.attribute("protocolInfo").is_some_and(|p| p.contains("video/")));
                let Some(url) = resource.and_then(|r| r.text()).map(str::trim).filter(|u| !u.is_empty()) else { continue };
                let art = child(node, "albumArtURI").unwrap_or_default();
                items.push(Item {
                    // The address to play, and the picture's after it: a DLNA item is not
                    // asked for again by its number.
                    key: format!("i{url}\u{1}{art}"),
                    title: crate::library::title(&title),
                    detail: crate::library::told_by_name(&title),
                    name: title,
                    aspect: Some(16.0 / 9.0),
                    pictured: !art.is_empty(),
                    ..Item::default()
                });
            }
            _ => {}
        }
    }
    Ok(items)
}

impl Server {
    pub fn new(found: Found) -> Server {
        Server { found, agent: agent() }
    }
}

impl Source for Server {
    fn label(&self) -> String {
        self.found.name.clone()
    }

    fn list(&self, folder: Option<&Item>) -> Result<Vec<Item>, String> {
        let id = folder.map_or("0", |f| f.key.strip_prefix('c').unwrap_or("0"));
        let body = format!(
            "<?xml version=\"1.0\" encoding=\"utf-8\"?><s:Envelope xmlns:s=\"http://schemas.xmlsoap.org/soap/envelope/\" s:encodingStyle=\"http://schemas.xmlsoap.org/soap/encoding/\"><s:Body><u:Browse xmlns:u=\"{DIRECTORY}\"><ObjectID>{}</ObjectID><BrowseFlag>BrowseDirectChildren</BrowseFlag><Filter>*</Filter><StartingIndex>0</StartingIndex><RequestedCount>500</RequestedCount><SortCriteria></SortCriteria></u:Browse></s:Body></s:Envelope>",
            escape(id)
        );
        let answer = self
            .agent
            .post(&self.found.control)
            .set("Content-Type", "text/xml; charset=\"utf-8\"")
            .set("SOAPACTION", &format!("\"{DIRECTORY}#Browse\""))
            .send_string(&body)
            .map_err(|e| format!("{} did not answer: {e}", self.found.name))?
            .into_string()
            .map_err(|e| format!("{}'s answer could not be read: {e}", self.found.name))?;
        browsed(&answer)
    }

    fn picture(&self, item: &Item) -> Option<Picture> {
        let art = item.key.split_once('\u{1}')?.1;
        let reply = self.agent.get(art).call().ok()?;
        let mut bytes = Vec::new();
        std::io::Read::read_to_end(&mut std::io::Read::take(reply.into_reader(), 8 * 1024 * 1024), &mut bytes).ok()?;
        Some(Picture::Encoded(bytes))
    }

    fn play(&self, item: &Item) -> Result<Play, String> {
        let url = item.key.strip_prefix('i').and_then(|rest| rest.split('\u{1}').next()).ok_or("not a film")?;
        Ok(Play { url: url.to_string(), options: Vec::new() })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_servers_description_gives_its_name_and_where_to_ask() {
        let text = r#"<?xml version="1.0"?><root xmlns="urn:schemas-upnp-org:device-1-0"><device><friendlyName>Attic NAS</friendlyName><serviceList>
            <service><serviceType>urn:schemas-upnp-org:service:ConnectionManager:1</serviceType><controlURL>/ctl/CM</controlURL></service>
            <service><serviceType>urn:schemas-upnp-org:service:ContentDirectory:1</serviceType><controlURL>/ctl/ContentDir</controlURL></service>
            </serviceList></device></root>"#;
        let found = described(text, "http://192.168.1.4:8200/rootDesc.xml").unwrap();
        assert_eq!(found, Found { name: "Attic NAS".into(), control: "http://192.168.1.4:8200/ctl/ContentDir".into() });
    }

    #[test]
    fn a_relative_address_is_relative_to_what_named_it() {
        assert_eq!(join("http://h:1/a/desc.xml", "ctl"), "http://h:1/a/ctl");
        assert_eq!(join("http://h:1/a/desc.xml", "/ctl"), "http://h:1/ctl");
        assert_eq!(join("http://h:1/", "http://other/x"), "http://other/x");
    }

    #[test]
    fn a_browse_gives_folders_and_films_and_leaves_out_what_is_neither() {
        let didl = r#"<DIDL-Lite xmlns="urn:schemas-upnp-org:metadata-1-0/DIDL-Lite/" xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:upnp="urn:schemas-upnp-org:metadata-1-0/upnp/">
            <container id="64" parentID="0"><dc:title>Films &amp; Shows</dc:title></container>
            <item id="64$1"><dc:title>Gravity.HSBS.mkv</dc:title><upnp:albumArtURI>http://h/art/1.jpg</upnp:albumArtURI>
              <res protocolInfo="http-get:*:video/x-matroska:*">http://h/MediaItems/1.mkv</res></item>
            <item id="64$2"><dc:title>Song</dc:title><res protocolInfo="http-get:*:audio/mpeg:*">http://h/2.mp3</res></item>
            </DIDL-Lite>"#;
        let answer = format!(
            "<s:Envelope xmlns:s=\"http://schemas.xmlsoap.org/soap/envelope/\"><s:Body><u:BrowseResponse xmlns:u=\"{DIRECTORY}\"><Result>{}</Result></u:BrowseResponse></s:Body></s:Envelope>",
            escape(didl)
        );
        let items = browsed(&answer).unwrap();
        assert_eq!(items.len(), 2);
        assert!(items[0].folder && items[0].title == "Films & Shows" && items[0].key == "c64");
        assert_eq!(items[1].title, "Gravity.HSBS");
        assert_eq!(items[1].detail.as_deref(), Some("3D"));
        let server = Server::new(Found { name: "x".into(), control: "http://h/ctl".into() });
        assert_eq!(server.play(&items[1]).unwrap().url, "http://h/MediaItems/1.mkv");
    }
}
