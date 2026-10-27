//! The extension stores: the Chrome Web Store and Firefox Add-ons, which a browser that is
//! neither Chrome nor Firefox is offered no button by. A store's page for one extension is
//! recognised here, and the address its package is downloaded from -- which SpatiMesh then
//! installs as it installs a package from a file ([`super::install`]).

/// Where extensions are found, for the menu's links to them.
pub const CHROME_WEB_STORE: &str = "https://chromewebstore.google.com/category/extensions";
pub const FIREFOX_ADD_ONS: &str = "https://addons.mozilla.org/firefox/extensions/";

/// The version of Chrome the Web Store is told it serves: what its packages are built for.
const CHROME_VERSION: &str = "131.0.0.0";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Store {
    Chrome,
    Firefox,
}

/// One extension's page in a store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Listing {
    pub store: Store,
    /// Chrome's 32-letter id, or the add-on's name in Firefox's addresses.
    pub id: String,
    /// Where its package comes from: a `.crx` or an `.xpi`.
    pub package: String,
}

impl Listing {
    /// What its package is called in the cache, for the installer to tell its kind by.
    pub fn file_name(&self) -> String {
        let safe: String = self.id.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect();
        match self.store {
            Store::Chrome => format!("{safe}.crx"),
            Store::Firefox => format!("{safe}.xpi"),
        }
    }
}

fn chrome_id(part: &str) -> bool {
    part.len() == 32 && part.bytes().all(|b| (b'a'..=b'p').contains(&b))
}

/// The listing a store's page is, if it is one.
pub fn listing(url: &str) -> Option<Listing> {
    let rest = url.strip_prefix("https://")?;
    let (host, path) = rest.split_once('/').unwrap_or((rest, ""));
    let path = path.split(['?', '#']).next().unwrap_or("");
    let parts: Vec<&str> = path.split('/').filter(|p| !p.is_empty()).collect();
    match host {
        // chromewebstore.google.com/detail/<name>/<id>, or /detail/<id>; the old store's
        // chrome.google.com/webstore/detail/<name>/<id>.
        "chromewebstore.google.com" | "chrome.google.com" => {
            let detail = parts.iter().position(|p| *p == "detail")?;
            let id = parts[detail + 1..].iter().find(|p| chrome_id(p))?;
            let package = format!(
                "https://clients2.google.com/service/update2/crx?response=redirect&prodversion={CHROME_VERSION}&acceptformat=crx2,crx3&x=id%3D{id}%26uc"
            );
            Some(Listing { store: Store::Chrome, id: id.to_string(), package })
        }
        // addons.mozilla.org/<language>/firefox/addon/<name>/ (or android).
        "addons.mozilla.org" => {
            let addon = parts.iter().position(|p| *p == "addon")?;
            let name = parts.get(addon + 1).filter(|n| !n.is_empty())?;
            let package = format!("https://addons.mozilla.org/firefox/downloads/latest/{name}/latest.xpi");
            Some(Listing { store: Store::Firefox, id: name.to_string(), package })
        }
        _ => None,
    }
}

/// Whether a file downloaded is an extension's package, to be offered for installing.
pub fn is_package(file: &str) -> bool {
    let lower = file.to_ascii_lowercase();
    lower.ends_with(".crx") || lower.ends_with(".xpi")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn store_pages_are_recognised() {
        let ublock = listing("https://chromewebstore.google.com/detail/ublock-origin-lite/ddkjiahejlhfcafbddmgiahcphecmpfh?hl=en").unwrap();
        assert_eq!(ublock.store, Store::Chrome);
        assert_eq!(ublock.id, "ddkjiahejlhfcafbddmgiahcphecmpfh");
        assert!(ublock.package.contains("x=id%3Dddkjiahejlhfcafbddmgiahcphecmpfh%26uc"));
        assert_eq!(ublock.file_name(), "ddkjiahejlhfcafbddmgiahcphecmpfh.crx");
        assert_eq!(listing("https://chromewebstore.google.com/detail/ddkjiahejlhfcafbddmgiahcphecmpfh").unwrap().id, "ddkjiahejlhfcafbddmgiahcphecmpfh");
        assert_eq!(listing("https://chrome.google.com/webstore/detail/x/ddkjiahejlhfcafbddmgiahcphecmpfh").unwrap().store, Store::Chrome);
        let firefox = listing("https://addons.mozilla.org/en-US/firefox/addon/ublock-origin/?utm_source=x").unwrap();
        assert_eq!(firefox.store, Store::Firefox);
        assert_eq!(firefox.package, "https://addons.mozilla.org/firefox/downloads/latest/ublock-origin/latest.xpi");
        assert_eq!(firefox.file_name(), "ublock-origin.xpi");
        assert!(listing("https://chromewebstore.google.com/category/extensions").is_none());
        assert!(listing("https://addons.mozilla.org/en-US/firefox/").is_none());
        assert!(listing("http://addons.mozilla.org/en-US/firefox/addon/x/").is_none(), "only over HTTPS");
        assert!(listing("https://example.com/detail/ddkjiahejlhfcafbddmgiahcphecmpfh").is_none());
        assert!(is_package("/home/a/Downloads/Thing.XPI") && !is_package("a.zip"));
    }
}
