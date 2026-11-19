//! What the open menu says.
//!
//! One description of a menu — a title, a column of rows, an explanation, a line of hints —
//! that every menu in the shell is expressed as. The scene knows how to draw *that* and
//! nothing about settings, environments or directories, which is why adding a fourth menu is
//! a function here rather than a branch in the renderer.
//!
//! This replaced a formatted string. The string carried its own selection marker, `\u{25b8}`,
//! baked into the text — which meant the whole panel had to be rasterised and re-uploaded
//! every time the cursor moved a row, and the panel's width depended on which row was
//! selected, because the marker is wider than the spaces standing in for it. Describing the
//! selection as *data* lets the renderer draw it as a shape, so moving the cursor now costs no
//! uploads at all and cannot change the layout.

use spatiand_shell::{files::PARENT_LABEL, Mode, Shell};

/// One line of a menu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MenuRow {
    pub label: String,
    /// Set to the right of the label, dimmer: a status rather than a name. Whether an
    /// environment is the one you are inside, whether a listing entry is a folder.
    pub trailing: Option<String>,
    /// The name of the group the rows under it belong to: drawn small and dim, never chosen.
    pub heading: bool,
}

impl MenuRow {
    fn plain(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            trailing: None,
            heading: false,
        }
    }

    fn with(label: impl Into<String>, trailing: &str) -> Self {
        Self {
            label: label.into(),
            trailing: Some(trailing.into()),
            heading: false,
        }
    }

    fn heading(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            trailing: None,
            heading: true,
        }
    }
}

/// A menu, ready to be laid out and drawn.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MenuModel {
    pub title: String,
    pub rows: Vec<MenuRow>,
    pub cursor: usize,
    /// One paragraph about whatever is selected, or about where you are.
    pub detail: String,
    /// Which buttons do what.
    pub footer: String,
}

impl MenuModel {
    /// Everything except the cursor, which is what decides whether the textures need rebuilding.
    ///
    /// The cursor is deliberately excluded: it moves constantly and changes nothing that was
    /// rasterised, since the selection is drawn as a shape. The detail *is* included, because
    /// in the settings list it is the focused row's explanation and does change with the cursor.
    fn content_key(&self) -> String {
        let mut key = format!("{}\u{1f}{}\u{1f}", self.title, self.footer);
        for row in &self.rows {
            key.push_str(&row.label);
            key.push('\u{1e}');
            key.push_str(row.trailing.as_deref().unwrap_or(""));
            key.push('\u{1e}');
        }
        key.push('\u{1f}');
        key.push_str(&self.detail);
        key
    }

    /// True when nothing rasterised for `other` can be reused for this.
    pub fn differs_from(&self, other: &Self) -> bool {
        self.content_key() != other.content_key()
    }
}

/// What goes in front of a card's title, which is also the pointer's way back.
///
/// The pointer can choose every row, and without this it could not leave: backing out was B,
/// and a Beam Pro has no B. The title line is where "up a level" sits on every phone and
/// every window, so the arrow is drawn there and the whole line is the target
/// ([`crate::scene::MenuTarget::Back`]).
pub const BACK_MARK: &str = "\u{2039}  ";

/// The menu the shell currently has open, if any.
///
/// `None` means there is nothing to draw: the world, or a launcher, which is a grid of glass
/// bubbles rather than a card and draws itself.
pub fn model(shell: &Shell) -> Option<MenuModel> {
    card(shell).map(|mut m| {
        m.title.insert_str(0, BACK_MARK);
        m
    })
}

fn card(shell: &Shell) -> Option<MenuModel> {
    match shell.mode() {
        Mode::World => None,
        Mode::Hud => {
            let hud = shell.hud();
            Some(MenuModel {
                title: "Settings".into(),
                rows: hud
                    .items()
                    .iter()
                    .map(|i| MenuRow::plain(i.label))
                    .collect(),
                cursor: hud.cursor(),
                detail: hud.focused().detail.into(),
                footer: FOOTER_SELECT.into(),
            })
        }
        Mode::AppMenu => {
            let list = shell.app_menu();
            Some(MenuModel {
                title: shell.app_menu_title().into(),
                rows: list.items().iter().map(|i| MenuRow::plain(i.label)).collect(),
                cursor: list.cursor(),
                detail: list.focused().detail.into(),
                footer: "[A] Choose  [B] Back".into(),
            })
        }
        Mode::Audio => {
            let list = shell.audio();
            Some(MenuModel {
                title: "Sound devices".into(),
                rows: list.items().iter().map(|i| MenuRow::plain(i.label)).collect(),
                cursor: list.cursor(),
                detail: list.focused().detail.into(),
                footer: "[A] Use  [B] Back".into(),
            })
        }
        Mode::Pinned => {
            let list = shell.pinned();
            Some(MenuModel {
                title: "Pinned windows".into(),
                rows: list.items().iter().map(|i| MenuRow::plain(i.label)).collect(),
                cursor: list.cursor(),
                detail: list.focused().detail.into(),
                footer: "[A] Change  [B] Back".into(),
            })
        }
        Mode::LookAndFeel => {
            let list = shell.look_and_feel();
            Some(MenuModel {
                title: "Look and feel".into(),
                rows: list.items().iter().map(|i| MenuRow::plain(i.label)).collect(),
                cursor: list.cursor(),
                detail: list.focused().detail.into(),
                footer: FOOTER_SELECT.into(),
            })
        }
        Mode::Ai => {
            let ai = shell.ai();
            Some(MenuModel {
                title: ai.title(),
                rows: ai
                    .rows()
                    .into_iter()
                    .map(|r| {
                        // A group's name is drawn in capitals, as the window list's are.
                        if r.heading {
                            MenuRow::heading(r.label.to_uppercase())
                        } else {
                            MenuRow { label: r.label, trailing: r.value, heading: false }
                        }
                    })
                    .collect(),
                cursor: ai.cursor(),
                detail: ai.detail(),
                footer: ai.footer().into(),
            })
        }
        Mode::Environment => {
            let picker = shell.environments();
            Some(MenuModel {
                title: "Environment".into(),
                rows: picker
                    .rows()
                    .iter()
                    .map(|r| {
                        if r.in_use {
                            // Spelt out rather than marked with a glyph. A tick has to survive
                            // being read through optics at an angle, and a font that has no
                            // tick in it substitutes a box.
                            MenuRow::with(r.label, "In use")
                        } else {
                            MenuRow::plain(r.label)
                        }
                    })
                    .collect(),
                cursor: picker.cursor(),
                detail: "Pick what surrounds you. The last row opens a folder to add your own \
                         panoramic image."
                    .into(),
                footer: FOOTER_SELECT.into(),
            })
        }
        Mode::Files => {
            let files = shell.files();
            let rows = files.rows();
            let empty = rows.len() == 1;
            Some(MenuModel {
                title: "Add an image".into(),
                rows: rows
                    .iter()
                    .map(|r| {
                        if r.name == PARENT_LABEL {
                            // "Go up" rather than the two dots the shell uses internally. The
                            // dots are a convention from a command line nobody is looking at.
                            MenuRow::plain("Go up")
                        } else if r.is_directory {
                            MenuRow::with(&r.name, "Folder")
                        } else {
                            MenuRow::plain(&r.name)
                        }
                    })
                    .collect(),
                cursor: files.cursor(),
                // Where you are, not what the row does. In a browser the question is always
                // "which folder is this", and the answer does not fit in a row.
                detail: if empty {
                    // Otherwise an empty folder looks exactly like a folder that failed to open.
                    format!("{}\nNo folders or images here.", files.directory())
                } else {
                    files.directory().into()
                },
                footer: "[A] Open  [B] Back".into(),
            })
        }
        Mode::Launcher if shell.launcher().is_empty() => {
            // Inside a remote computer's tab, "nothing here" has a different reason: the
            // computer is off, or has not been given anything to offer yet.
            let launcher = shell.launcher();
            let host = match launcher.level() {
                spatiand_shell::Level::Host(i) => launcher.hosts().get(*i),
                _ => None,
            };
            Some(match host {
                Some(h) if !h.online => MenuModel {
                    title: h.label.clone(),
                    rows: Vec::new(),
                    cursor: 0,
                    detail: format!(
                        "{} is offline. Its apps are still running there if they were; they \
                         come back here when it does.",
                        h.label
                    ),
                    footer: "[B] Back".into(),
                },
                Some(h) => MenuModel {
                    title: h.label.clone(),
                    rows: Vec::new(),
                    cursor: 0,
                    detail: format!(
                        "{} offers no applications yet. Add some in its catalogue.",
                        h.label
                    ),
                    footer: "[B] Back".into(),
                },
                None if !launcher.query().is_empty() => MenuModel {
                    title: format!("Search: {}", launcher.query()),
                    rows: Vec::new(),
                    cursor: 0,
                    detail: "Nothing here is called that.".into(),
                    footer: "[B] Clear what was typed".into(),
                },
                None => MenuModel {
                    title: "No applications".into(),
                    rows: Vec::new(),
                    cursor: 0,
                    detail: "Nothing was found in the system's application folders.".into(),
                    footer: "[B] Back".into(),
                },
            })
        }
        Mode::Hosts => Some(hosts(shell.hosts())),
        Mode::Bluetooth => Some(bluetooth(shell.bluetooth())),
        Mode::Switcher => {
            let switcher = shell.switcher();
            Some(MenuModel {
                title: "Windows".into(),
                rows: switcher
                    .entries()
                    .iter()
                    .map(|w| {
                        // Spelt out rather than marked, for the same reason as the
                        // environment list: a glyph read through optics at an angle is a
                        // box as often as it is a tick.
                        if w.heading {
                            MenuRow::heading(w.title.to_uppercase())
                        } else if w.hidden {
                            MenuRow::with(w.title.clone(), "Hidden")
                        } else if w.pinned {
                            MenuRow::with(w.title.clone(), "Pinned to your view")
                        } else if w.current {
                            MenuRow::with(w.title.clone(), "In front of you")
                        } else if w.room {
                            MenuRow::with(w.title.clone(), "Fills your view")
                        } else {
                            MenuRow::plain(w.title.clone())
                        }
                    })
                    .collect(),
                cursor: switcher.cursor(),
                detail: "Choosing a window brings it to the centre of your view, hidden or not. \
                         X hides one or shows it again; left pins one to the corner of your \
                         view and right lets it go; Y closes one, and holding Y forces an \
                         application that will not close to quit."
                    .into(),
                footer: "[A] Select  [X] Hide  [Y] Close  [Left] Pin  [B] Back".into(),
            })
        }
        Mode::Launcher => None,
        // The editor is drawn from its own view; see `from_editor`.
        Mode::Controller => None,
    }
}

/// The remote computers page: the list, the address or the code being typed, or the pairing.
fn hosts(hosts: &spatiand_shell::Hosts) -> MenuModel {
    use spatiand_shell::hosts::{Page, ADD_LABEL};
    match hosts.page() {
        Page::List => {
            let mut rows: Vec<MenuRow> = hosts
                .rows()
                .iter()
                .enumerate()
                .map(|(i, r)| {
                    if hosts.is_armed(i) {
                        MenuRow::with(&r.label, "A again to forget")
                    } else {
                        MenuRow::with(&r.label, r.status.label())
                    }
                })
                .collect();
            rows.push(MenuRow::plain(ADD_LABEL));
            let cursor = hosts.cursor();
            let detail = match hosts.rows().get(cursor) {
                Some(r) if hosts.is_armed(cursor) => format!(
                    "Press A again to forget {}. Its apps keep running there; this headset \
                     just stops showing them, until it is paired again.",
                    r.label
                ),
                Some(r) => format!(
                    "{} at {}. Its apps are in the launcher, in their own tab. Press A twice \
                     to forget it.",
                    r.label, r.address
                ),
                None => "Pair with a computer running spatiand-host, so its apps can be \
                         opened here as windows."
                    .into(),
            };
            MenuModel {
                title: "Remote computers".into(),
                rows,
                cursor,
                detail,
                footer: FOOTER_SELECT.into(),
            }
        }
        Page::Address => MenuModel {
            title: "Add a computer".into(),
            // The text being typed, as the one row, with a bar where the next letter goes.
            rows: vec![MenuRow::plain(format!("{}\u{2502}", hosts.typed()))],
            cursor: 0,
            detail: "On the computer, run  spatiand-host --pair  first: it shows a code. Then \
                     type its name or address here — its hostname, or something like \
                     192.168.1.20 — and press Enter."
                .into(),
            footer: "[Enter] Next  [B] Back".into(),
        },
        Page::Code => MenuModel {
            title: format!("Pairing with {}", hosts.typed()),
            // The digits typed so far, in the two groups the computer shows them in, with a bar
            // where the next one goes.
            rows: vec![MenuRow::plain(format!("{}\u{2502}", code_shown(hosts.code())))],
            cursor: 0,
            detail: "Type the six-digit code the computer shows. Only a device that has it is \
                     paired, and it is never sent: the two prove to each other that they know it."
                .into(),
            footer: "[Enter] Pair  [B] Back".into(),
        },
        Page::Pairing => {
            let pairing = hosts.pairing();
            let footer = match (pairing.finished, pairing.succeeded) {
                (true, true) => "[A] Done",
                (true, false) => "[A] Try again  [B] Back",
                (false, _) => "[B] Cancel",
            };
            MenuModel {
                title: format!("Pairing with {}", pairing.address),
                rows: Vec::new(),
                cursor: 0,
                detail: pairing.status.clone(),
                footer: footer.into(),
            }
        }
    }
}

/// A code as it is being typed: `482 9`, grouped as the computer shows it.
fn code_shown(code: &str) -> String {
    if code.len() > 3 {
        format!("{} {}", &code[..3], &code[3..])
    } else {
        code.to_string()
    }
}

/// The Bluetooth page: paired devices, then adding one, then the switch.
fn bluetooth(bt: &spatiand_shell::Bluetooth) -> MenuModel {
    use spatiand_shell::bluetooth::{Row, ADD_LABEL};
    let view = bt.view();
    let mut rows: Vec<MenuRow> = view
        .devices
        .iter()
        .enumerate()
        .map(|(i, d)| {
            let status = if bt.is_armed(i) {
                "Y again to forget".to_string()
            } else if bt.is_busy(&d.address) {
                if d.connected { "disconnecting" } else { "connecting" }.to_string()
            } else {
                let mut s = if d.connected { "connected" } else { "not connected" }.to_string();
                if let Some(b) = d.battery {
                    s.push_str(&format!(" · {b}%"));
                }
                s
            };
            // Two with one name — the same keyboard paired twice, usually — are told apart by
            // the end of their address.
            let twins = view.devices.iter().filter(|o| o.label == d.label).count() > 1;
            let label = if twins {
                format!("{} ({})", d.label, &d.address[d.address.len().saturating_sub(5)..])
            } else {
                d.label.clone()
            };
            MenuRow::with(label, &status)
        })
        .collect();
    rows.push(MenuRow::plain(ADD_LABEL));
    rows.push(match view.powered {
        Some(true) => MenuRow::with("Bluetooth", "on"),
        Some(false) => MenuRow::with("Bluetooth", "off"),
        None => MenuRow::with("Bluetooth", "no adapter"),
    });
    let cursor = bt.cursor();
    let explain = match bt.row(cursor) {
        Row::Device(i) => {
            let d = &view.devices[i];
            let kind = d.kind_word().map(|k| format!("A {k}, ")).unwrap_or_default();
            if bt.is_armed(i) {
                format!(
                    "Press Y again to forget {}. It will have to be paired again before it \
                     can connect.",
                    d.label
                )
            } else if d.connected {
                format!("{kind}{}. A disconnects it; Y twice forgets it.", d.address)
            } else {
                format!(
                    "{kind}{}. A connects it — wake it first with a key or its button. If it \
                     never connects, it may have paired again under another address: forget it \
                     with Y twice and add it again.",
                    d.address
                )
            }
        }
        Row::Add => "Find a new device and pair it, in a window in front of you. Put the \
                     device in pairing mode first."
            .to_string(),
        Row::Power => match view.powered {
            Some(true) => "Turns Bluetooth off. Every device here disconnects.".into(),
            Some(false) => "Bluetooth is off. A turns it on.".into(),
            None => "No Bluetooth adapter answered.".into(),
        },
    };
    let detail = if view.note.is_empty() {
        explain
    } else {
        format!("{}\n\n{explain}", view.note)
    };
    let footer = match bt.row(cursor) {
        Row::Device(_) => "[A] Connect or disconnect  [Y] Forget  [B] Back",
        _ => FOOTER_SELECT,
    };
    MenuModel {
        title: "Bluetooth".into(),
        rows,
        cursor,
        detail,
        footer: footer.into(),
    }
}

/// The layout editor's page, as a card like every other menu.
pub fn from_editor(view: &spatiand_mapper::editor::View) -> MenuModel {
    MenuModel {
        title: view.title.clone(),
        rows: view
            .rows
            .iter()
            .map(|r| MenuRow {
                label: r.label.clone(),
                trailing: r.value.clone(),
                heading: false,
            })
            .collect(),
        cursor: view.cursor,
        detail: view.detail.clone(),
        footer: view.footer.clone(),
    }
}

/// The pad's buttons a card answers, as keycaps and a verb each: `"[A] Select  [B] Back"`, the
/// form of the apps' foot hints (`look::keys`), drawn as keycaps by the scene. Groups are two
/// spaces apart; a group with no brackets is words only (the editor's "3 of 24").
const FOOTER_SELECT: &str = "[A] Select  [B] Back";

/// A footer split into its groups -- two spaces or more apart -- and each group into its keys
/// (in brackets) and its words: `(group, is a key, text)`, in order.
pub fn footer_parts(footer: &str) -> Vec<(usize, bool, &str)> {
    let mut parts = Vec::new();
    for (g, group) in footer.split("  ").map(str::trim).filter(|g| !g.is_empty()).enumerate() {
        let mut rest = group;
        while !rest.is_empty() {
            match (rest.find('['), rest.find(']')) {
                (Some(0), Some(end)) => {
                    parts.push((g, true, &rest[1..end]));
                    rest = rest[end + 1..].trim_start();
                }
                (Some(at), _) if at > 0 => {
                    parts.push((g, false, rest[..at].trim()));
                    rest = &rest[at..];
                }
                _ => {
                    parts.push((g, false, rest.trim()));
                    rest = "";
                }
            }
        }
    }
    parts
}

#[cfg(test)]
mod tests {
    use super::*;
    use spatiand_shell::grid::Direction;
    use spatiand_shell::hud::HudAction;
    use spatiand_shell::DesktopPanels;
    use spatiand_shell::Intent;

    #[test]
    fn a_footer_is_keys_and_their_verbs_in_groups() {
        assert_eq!(footer_parts(FOOTER_SELECT), vec![(0, true, "A"), (0, false, "Select"), (1, true, "B"), (1, false, "Back")]);
        assert_eq!(
            footer_parts("3 of 24  [Left] [Right] Change  [B] Back"),
            vec![(0, false, "3 of 24"), (1, true, "Left"), (1, true, "Right"), (1, false, "Change"), (2, true, "B"), (2, false, "Back")]
        );
        assert_eq!(footer_parts("[B] Clear what was typed"), vec![(0, true, "B"), (0, false, "Clear what was typed")]);
    }

    /// Walk the settings list to the look and feel card, and in it to the environment row, and
    /// press A.
    ///
    /// By action rather than by position: the row moves whenever the HUD gains an entry, and a
    /// test that counts presses passes for a while and then silently opens something else.
    fn open_environments(shell: &mut Shell) {
        for _ in 0..shell.hud().items().len() {
            if shell.hud().activate() == HudAction::OpenLookAndFeel {
                break;
            }
            shell.handle(Intent::Navigate(Direction::Down));
        }
        assert_eq!(shell.hud().activate(), HudAction::OpenLookAndFeel);
        shell.handle(Intent::Accept);
        for _ in 0..shell.look_and_feel().items().len() {
            if shell.look_and_feel().activate() == HudAction::OpenEnvironments {
                break;
            }
            shell.handle(Intent::Navigate(Direction::Down));
        }
        assert_eq!(shell.look_and_feel().activate(), HudAction::OpenEnvironments);
        shell.handle(Intent::Accept);
    }

    #[test]
    fn the_two_cards_are_one_row_each_in_the_settings_and_a_card_of_their_own() {
        let mut shell = hud_shell();
        let m = model(&shell).unwrap();
        for row in ["Look and feel", "AI"] {
            assert_eq!(m.rows.iter().filter(|r| r.label == row).count(), 1, "{row}");
        }
        for gone in ["Environment", "Pinned windows"] {
            assert!(!m.rows.iter().any(|r| r.label == gone), "{gone} is the look and feel card's");
        }
        assert!(!m.rows.iter().any(|r| r.label.starts_with("Look:") || r.label.starts_with("Steadiness:")));
        // The look and feel card, with its title as the way back.
        let at = shell.hud().items().iter().position(|i| i.action == HudAction::OpenLookAndFeel).unwrap();
        assert!(shell.point(at));
        shell.handle(Intent::Accept);
        let card = model(&shell).unwrap();
        assert_eq!(card.title, format!("{BACK_MARK}Look and feel"));
        let labels: Vec<&str> = card.rows.iter().map(|r| r.label.as_str()).collect();
        assert_eq!(labels, ["Look: as the system", "Steadiness: medium", "Environment", "Pinned windows"]);
        assert_eq!(card.footer, FOOTER_SELECT);
        // The AI card: its groups named in capitals and never chosen, each setting's value set
        // to the right of it.
        shell.handle(Intent::Back);
        let at = shell.hud().items().iter().position(|i| i.action == HudAction::OpenAi).unwrap();
        assert!(shell.point(at));
        shell.handle(Intent::Accept);
        shell.set_ai(spatiand_shell::AiView { hemera: "http://deepmagpie:12434".into(), everyday: "neohorse-1:4b".into(), ..Default::default() });
        let card = model(&shell).unwrap();
        assert_eq!(card.title, format!("{BACK_MARK}AI"));
        let headings: Vec<&str> = card.rows.iter().filter(|r| r.heading).map(|r| r.label.as_str()).collect();
        assert_eq!(headings, ["MODELS", "DICTATION", "TEXT IN PICTURES"]);
        assert_eq!(card.rows[0].trailing.as_deref(), Some("http://deepmagpie:12434"));
        assert!(card.rows.iter().any(|r| r.label == "Everyday model" && r.trailing.as_deref() == Some("neohorse-1:4b")));
        assert!(!card.rows[card.cursor].heading);
        assert_eq!(card.footer, "[A] Change  [B] Back");
        assert_eq!(footer_parts(&card.footer), vec![(0, true, "A"), (0, false, "Change"), (1, true, "B"), (1, false, "Back")]);
    }

    fn hud_shell() -> Shell {
        let mut shell = Shell::new(Vec::new(), DesktopPanels::ALL, true);
        shell.handle(Intent::ToggleHud);
        shell
    }

    #[test]
    fn the_world_has_no_menu() {
        assert!(model(&Shell::new(Vec::new(), DesktopPanels::ALL, true)).is_none());
    }

    #[test]
    fn the_settings_menu_lists_every_row_and_explains_the_focused_one() {
        let shell = hud_shell();
        let m = model(&shell).expect("the hud is open");
        assert_eq!(m.rows.len(), shell.hud().items().len());
        assert_eq!(m.detail, shell.hud().focused().detail);
        assert!(!m.footer.is_empty());
    }

    #[test]
    fn moving_the_cursor_does_not_change_what_has_to_be_rasterised_except_the_detail() {
        // The point of the model: the selection is a shape the renderer draws, not a marker
        // baked into the text, so walking the list must not invalidate the rows.
        let mut shell = hud_shell();
        let before = model(&shell).unwrap();
        shell.handle(Intent::Navigate(Direction::Down));
        let after = model(&shell).unwrap();
        assert_ne!(before.cursor, after.cursor);
        assert_eq!(before.rows, after.rows, "the rows must be reusable");
        assert_eq!(before.title, after.title);
    }

    #[test]
    fn the_environment_in_use_is_marked_and_the_rest_are_not() {
        use spatiand_shell::environment::{EnvironmentChoice, EnvironmentEntry};
        let mut shell = hud_shell();
        open_environments(&mut shell);
        shell.set_environments(
            vec![
                EnvironmentEntry {
                    label: "Blank".into(),
                    choice: EnvironmentChoice::Blank,
                },
                EnvironmentEntry {
                    label: "Studio".into(),
                    choice: EnvironmentChoice::Studio,
                },
            ],
            EnvironmentChoice::Studio,
        );
        let m = model(&shell).expect("the picker is open");
        assert_eq!(m.title, format!("{BACK_MARK}Environment"));
        let marked: Vec<_> = m.rows.iter().filter(|r| r.trailing.is_some()).collect();
        assert_eq!(marked.len(), 1, "exactly one environment is in use");
        assert_eq!(marked[0].label, "Studio");
        // And the browse row is still the last one, unmarked.
        assert!(m.rows.last().unwrap().trailing.is_none());
    }

    #[test]
    fn a_directory_listing_says_which_rows_are_folders_and_which_way_is_up() {
        use spatiand_shell::files::FileEntry;
        let mut shell = hud_shell();
        open_environments(&mut shell);
        // Straight to the browse row, which is always last.
        for _ in 0..20 {
            shell.handle(Intent::Navigate(Direction::Down));
        }
        shell.handle(Intent::Accept);
        shell.show_directory(
            "/home/deck/Pictures".into(),
            vec![
                FileEntry {
                    name: "Panoramas".into(),
                    is_directory: true,
                },
                FileEntry {
                    name: "sunset.jpg".into(),
                    is_directory: false,
                },
            ],
        );
        let m = model(&shell).expect("the browser is open");
        assert_eq!(m.rows[0].label, "Go up");
        assert_eq!(m.rows[1].trailing.as_deref(), Some("Folder"));
        assert_eq!(m.rows[2].trailing, None);
        assert!(m.detail.contains("/home/deck/Pictures"));
    }

    #[test]
    fn an_empty_folder_says_so_rather_than_looking_broken() {
        use spatiand_shell::files::FileEntry;
        let mut shell = hud_shell();
        open_environments(&mut shell);
        for _ in 0..20 {
            shell.handle(Intent::Navigate(Direction::Down));
        }
        shell.handle(Intent::Accept);
        shell.show_directory("/tmp/empty".into(), Vec::<FileEntry>::new());
        let m = model(&shell).unwrap();
        assert_eq!(m.rows.len(), 1, "only the way out");
        assert!(m.detail.contains("No folders or images"));
    }

    #[test]
    fn the_content_key_ignores_the_cursor_and_notices_everything_else() {
        let a = MenuModel {
            title: "Settings".into(),
            rows: vec![MenuRow::plain("One"), MenuRow::plain("Two")],
            cursor: 0,
            detail: "d".into(),
            footer: "f".into(),
        };
        let moved = MenuModel {
            cursor: 1,
            ..a.clone()
        };
        assert!(!a.differs_from(&moved), "a moved cursor rebuilds nothing");
        let renamed = MenuModel {
            rows: vec![MenuRow::plain("One"), MenuRow::with("Two", "In use")],
            ..a.clone()
        };
        assert!(
            a.differs_from(&renamed),
            "a new trailing label must rebuild"
        );
        let explained = MenuModel {
            detail: "different".into(),
            ..a.clone()
        };
        assert!(a.differs_from(&explained));
    }
}
