//! SpatiMesh's profiles, bookmarks and settings on two machines, kept the same by
//! spatiand-sync as Spatiand keeps them whenever two machines are linked: what either changes
//! reaches the other, two changes made apart are both kept, and the website data and the cache
//! never leave the machine.

use std::path::{Path, PathBuf};

use spatiand_sync::Sync;
use spatimesh::bookmarks::{self, At};
use spatimesh::profiles;
use spatimesh::settings::Settings;
use spatimesh::store::Store;

struct Machine {
    root: PathBuf,
    store: Store,
    sync: Sync,
}

impl Machine {
    fn new(name: &str) -> Machine {
        let root = std::env::temp_dir().join(format!("spatimesh-sync-{}-{}-{name}", std::process::id(), spatimesh::store::new_id()));
        let _ = std::fs::remove_dir_all(&root);
        // SpatiMesh's shared folder is `apps/spatimesh`; the synchroniser's is all of `apps`.
        let store = Store::at(&root);
        let sync = Sync::open(root.join("apps"), root.join("local"));
        Machine { root, store, sync }
    }

    fn names(&self) -> Vec<String> {
        profiles::list(&self.store).into_iter().map(|p| p.name).collect()
    }

    fn bookmarked(&self, profile: &str) -> Vec<String> {
        let mut titles: Vec<String> = bookmarks::all(&self.store, profile).into_iter().map(|b| b.title).collect();
        titles.sort();
        titles
    }
}

impl Drop for Machine {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// Two machines linked: they talk until neither has anything to say, as a link does.
fn link(a: &mut Machine, b: &mut Machine) {
    let mut to_b = vec![a.sync.hello()];
    let mut to_a = vec![b.sync.hello()];
    for _ in 0..10 {
        let mut next_a = Vec::new();
        for m in to_b.drain(..) {
            next_a.extend(b.sync.handle(m));
        }
        let mut next_b = Vec::new();
        for m in to_a.drain(..) {
            next_b.extend(a.sync.handle(m));
        }
        to_a = next_a;
        to_b = next_b;
        if to_a.is_empty() && to_b.is_empty() {
            break;
        }
    }
}

/// Every file under `dir`, by its path under it.
fn files(dir: &Path) -> Vec<String> {
    let mut found = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for entry in std::fs::read_dir(&d).into_iter().flatten().flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else {
                found.push(path.strip_prefix(dir).unwrap().to_string_lossy().into_owned());
            }
        }
    }
    found.sort();
    found
}

#[test]
fn two_machines_keep_one_set_of_profiles_bookmarks_and_settings() {
    let mut deck = Machine::new("deck");
    let mut mac = Machine::new("mac");

    // Each machine starts with a first profile of its own making. On the Mac it is renamed and
    // a second one made; only then is SpatiMesh first started on the Deck.
    assert_eq!(mac.names(), vec!["Personal"]);
    profiles::rename(&mac.store, profiles::FIRST, "Me").unwrap();
    let work = profiles::add(&mac.store, "Work").unwrap();
    std::thread::sleep(std::time::Duration::from_millis(20));
    assert_eq!(deck.names(), vec!["Personal"]);

    // Apart, each bookmarks a page, and each changes a different setting.
    bookmarks::add(&deck.store, profiles::FIRST, "", "From the Deck", Some("https://deck.example/"), At::End).unwrap();
    bookmarks::add(&mac.store, profiles::FIRST, "", "From the Mac", Some("https://mac.example/"), At::End).unwrap();
    let mut settings = Settings::load(&deck.store);
    settings.zoom = 1.5;
    settings.save(&deck.store).unwrap();
    let mut settings = Settings::load(&mac.store);
    settings.search = "Google".into();
    settings.save(&mac.store).unwrap();

    // What is the Mac's alone: a profile's website data and cache.
    for dir in [mac.store.data_dir(profiles::FIRST), mac.store.cache_dir(profiles::FIRST)] {
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("Cookies"), b"the mac's").unwrap();
    }

    link(&mut deck, &mut mac);

    for machine in [&deck, &mac] {
        // The Mac's rename wins over the Deck's own first profile, made later but not chosen.
        assert_eq!(machine.names(), vec!["Me", "Work"]);
        assert_eq!(machine.bookmarked(profiles::FIRST), vec!["From the Deck", "From the Mac"]);
        let settings = Settings::load(&machine.store);
        assert_eq!(settings.zoom, 1.5);
        assert_eq!(settings.search_engine().name, "Google");
    }
    // Website data and the cache stayed where they were.
    assert!(!deck.store.data_dir(profiles::FIRST).exists());
    assert!(!deck.store.cache_dir(profiles::FIRST).exists());
    assert!(files(&deck.root.join("apps")).iter().all(|f| !f.contains("Cookies")), "{:?}", files(&deck.root.join("apps")));

    // A bookmark removed on the Mac, and a profile on the Deck, go from both.
    let mac_one = bookmarks::all(&mac.store, profiles::FIRST).into_iter().find(|b| b.title == "From the Mac").unwrap();
    bookmarks::remove(&mac.store, profiles::FIRST, &mac_one.id);
    profiles::remove(&deck.store, &work.id).unwrap();
    link(&mut deck, &mut mac);
    for machine in [&deck, &mac] {
        assert_eq!(machine.bookmarked(profiles::FIRST), vec!["From the Deck"]);
        assert_eq!(machine.names(), vec!["Me"]);
    }
}
