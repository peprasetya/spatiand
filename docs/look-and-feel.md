# Look and feel: the rules as they are, and where each app departs

The owner's order: "uniform look and feel ... and polished the project". This is not a redesign.
The norm is what the SDK's shared widgets do (`crates/spatiand-app/src/ui`); where they are
silent, what most of the apps do. Part 1 names the norm, Part 2 lists every departure found with
the exact change, Part 3 says how to check an app afterwards. Written 2026-11-18 from the code at
`2eb8a3a`; line numbers are that commit's. Nothing was built or run for it, and no snapshot was
taken: every statement here is from reading the source. What was not read is at the end.

Paths are short: `app/` is `crates/spatiand-app/src/`, `shelf/` is `crates/spatishelf/src/`, and
so on for `quill/`, `pal/`, `key/`, `reel/`, `loupe/`; `shell/` is `crates/spatiand-shell/src/`,
`room/` is `crates/spatiand/src/`, `choose/` is `crates/spatiand-choose/src/`, `panel.rs` is
`crates/spatiand-render/src/panel.rs`. Sizes are logical pixels of a 1920x1080 window; the
glasses show about 48 of them to a degree.

---

## Part 1: the rules

### 1. Type

| Use | Size | Symbol |
|---|---|---|
| An app's name at the head of its first screen | 40 | `theme::TITLE_EM`, drawn by `look::header` |
| A screen's title beside its Back button | 36 | `theme::TITLE_EM * 0.9` |
| A pane's or dialog's title | 32 | `theme::TITLE_EM * 0.8` |
| A row's label, a field's text | 36 | `theme::ROW_EM` |
| A button's or a tab's label | 31 | `theme::ROW_EM * 0.86` (`Button::draw`, `Tabs::draw`) |
| Prose, a question's text, a row's trailing word | 29 | `theme::ROW_EM * 0.8` (`Confirm::draw`, `List::draw`) |
| A tile's caption | 26 | `theme::ROW_EM * 0.72` (`widgets.rs` `CAPTION`) |
| Detail beside a title, a value in a pane | 25 | `theme::DETAIL_EM` |
| The foot line: hints, what went wrong, what was done | 23 | `theme::FOOTER_EM` |
| A tile's second line, a keycap's words | 21 | `theme::FOOTER_EM * 0.92` |
| A small upper-case label over a value, a column's head | 19 | `theme::FOOTER_EM * 0.82` |

- Sizes are always a `theme::*_EM` constant times a factor, never a bare number. No app
  hard-codes a bare text size today (the only bare ones are a film's subtitles, a part of the
  picture's height, `reel/play.rs:769`, and a tile's "PDF", `shelf/look.rs:59`; both scale with
  what they are drawn on and stay).
- **Nothing is set smaller than `theme::FOOTER_EM * 0.82` (19).** Large one-off figures (a
  pairing code, a one-time code) are `TITLE_EM * 1.05` to `* 2.4` in `look::TEAL`.
- Small labels over a value are written in sentence case in the source and drawn with
  `.to_uppercase()` (`look::shelf`, `loupe/view.rs:1114`).

### 2. Colour, light and dark

Every colour is a function of `ui::theme`, which answers for dark or light:

| What | Symbol | Dark | Light |
|---|---|---|---|
| Window ground | `theme::ground()` | 0.043 0.05 0.072 | 0.955 0.961 0.973 |
| Raised surface (field, button) | `theme::raised()` | white 6% | black 5% |
| Border / rim | `theme::rim()` | 0.55 0.70 1.0 at 16% | 0.18 0.32 0.65 at 20% |
| Hover | `theme::hover()`; a button `theme::wash(0.11)` | white 5% | black 4% |
| Selection, with the keys | `theme::selected()` | 0.42 0.68 1.0 at 20% | 0.16 0.44 0.95 at 16% |
| Selection, without the keys | `theme::selected_away()` | white 8% | black 7% |
| Accent (caret, focus line, progress, ring) | `theme::mark()` | 0.45 0.72 1.0 | 0.10 0.40 0.88 |
| Text: strongest, row, detail, hint, trailing | `theme::ink()`, `ink_row()`, `ink_detail()`, `ink_hint()`, `ink_trailing()` | | |
| What went wrong | `look::trouble()` | 1.0 0.62 0.55 | 0.78 0.18 0.12 |
| The mark's three | `look::TEAL`, `look::BLUE`, `look::VIOLET` | same in both | |
| Separator, scroll track, scroll thumb | `theme::separator()`, `scroll_track()`, `scroll_thumb()` | | |

- **There is no destructive colour.** A destructive button looks like any other; what protects
  is the question (rule 12). `look::trouble()` is for words that say what went wrong, and for a
  stopped tab's dot, nothing else.
- **Following the room's mode.** `App::wait` reads the platform's look each second and calls
  `theme::set_dark` (`app/lib.rs:293`, `:718`). All six apps and `spatiand-choose` clear to
  `theme::ground()` and draw through `theme::*`, so all follow. Exceptions that are right:
  SpatiReel's player draws inside `theme::always_dark` (`reel/play.rs:619`: over a film);
  SpatiQuill's terminal cells are its own dark palette (`quill/tab.rs:29`, `:47`); the room's own
  cards, launcher, notices and keyboard are always dark (`room/scene.rs:328-343`,
  `room/keyboard_face.rs:32-55`) and the Look setting says so (`shell/hud.rs:217`: "for
  Spatiand's own apps and the pages in SpatiMesh").
- A literal `[r, g, b, a]` in an app is right only for: a tile's own tint
  (`Tile::tint`), white ink on such a tint, a film's subtitles, a page's paper, and a `uv`
  rectangle. Every other literal is a departure (Part 2).

### 3. Corners, borders, shadow

| What | Value | Symbol |
|---|---|---|
| Row, button, field, tile | 14 | `theme::ROW_RADIUS` |
| Tab in `ui::Tabs`; a round icon button | half its height | `Tabs::draw`, `Button::draw_icon` |
| A pane, a dialog | 24 | `Confirm::draw` |
| A menu, a pill | 20 | Shelf, Quill |
| A side pane, a floating bar | 22 | Shelf, Loupe |
| The room's cards | 34 | `theme::CARD_RADIUS` |

- No widget has a drawn border. An edge is `look::glass` (a soft shadow of 26, a gradient, a 1.5
  highlight along the top). The focus mark of a field is a 3 high line of `theme::mark()` along
  its foot. A tile with the keys stands in a 3 wide ring of `theme::mark()` with a glow.
- A pane over the window: the window dimmed with black at 35% (`Confirm::draw`), the pane filled
  with `theme::ground()` and then `look::glass(.., 24.0, 1.0)`.

### 4. Spacing

- Window margin `theme::PAD` (42). Row gap `theme::ROW_GAP` (4). Tile gap `theme::TILE_GAP`
  (22). Inset of a row's text `theme::ROW_INSET` (24). Inside a pane 28. Between buttons in a
  row 12. Under a screen's head: 12, the rule (2, `look::rule`), 16 to 24.
- `Rect::take_top` / `take_left` do the layout; there is no other grid.

### 5. Minimum hit size

- Every SDK control is `theme::ROW_HEIGHT` (60) high: `Button::HEIGHT`, `Field::HEIGHT`,
  `Tabs::HEIGHT`, a `List` row. An icon-only button is 60 round.
- **Nothing that can be pressed has a hit rectangle under 44 by 44** (a little under a degree),
  whatever size it is drawn: the tab bars' "+" (44), SpatiPal's bin (44). A smaller drawing gets
  a larger hit rectangle (`.inset(-n)`).

### 6. Tab bars

- A line of views of one thing (SpatiKey's All / Logins / Passkeys / Codes): `ui::Tabs`.
- A bar of closable tabs (SpatiShelf, SpatiQuill): 66 high (`BAR`); each tab's face 50 high,
  radius 14; the active one a gradient `theme::wash(0.11)` to `wash(0.05)`, a blue glow, and
  `look::rule` along its foot; a dot of 8 at the left (teal: active, hint grey: idle, amber
  `[1.0, 0.85, 0.4]`: busy or rung, `look::trouble()`: stopped, violet: a remote); the label at
  `ROW_EM * 0.72`; a close cross on the active and the hovered tab; a "+" of 44 after the last.
  Both apps draw this themselves, alike to the pixel: SDK consolidation 1.

### 7. Toolbars and drawn icons

- Icons are `ui::Icon`, drawn by `ui::draw_icon`: a 24 box, stroke 1.9, round caps and joins,
  no fill (`app/ui/icons.rs:161`). In a button `ROW_EM * 0.9` (32) or 60% of its height; before
  a row's label `ROW_EM * 0.95`.
- A toolbar is a row of `Button::draw_icon`, 60 high, 8 to 12 apart: icon alone (round) for
  Back / Forward / Up / Play and the like, icon and word where the word is needed.
- **No emoji or pictograph characters in strings**: the Beam Pro draws colour emoji blank. A
  symbol is an `Icon`.

### 8. Buttons

- One kind: `ui::Button`. Ground `theme::raised()`; hover `theme::wash(0.11)`; focused (has the
  keys) or pressed `theme::selected()`; ink `ink_row()`, or `ink()` when hovered or focused.
  Acts on release over itself; Enter or Space when focused.
- **Primary** is the focused one: no other marking. **Destructive**: no marking (rule 12).
  **Disabled does not exist**: a button that cannot act is not drawn (`reel/screens.rs:873`,
  `shelf/main.rs:1931`).
- A button drawn lit only to show a state (SpatiPal's microphone) sets `focused` to draw and
  clears it after (`pal/ui.rs:202-210`).

### 9. Text fields

`ui::Field` is the only text field.

- Caret: 2 wide, `theme::mark()`, 56% of the field's height, not blinking. Selection: a wash
  behind the text. Placeholder: `Field::hint` in `ink_hint()`. Focus: the 3 high line at the
  foot. `Field::secret` draws dots and refuses copy and cut.
- Keys in the field (Command on the Mac, Control elsewhere): A all, C copy, X cut, V paste;
  Shift with Left / Right / Home / End chooses; Control-U clears (not Command-U). Double click a
  word, triple click all, drag chooses, Shift-click extends.
- The app decides, and all do it this way: **Enter** takes what was typed (`Typed::Entered`).
  **Escape** drops it and closes the field or, for a field always on show, empties it
  (`key/open.rs:139`). **Tab** takes it and goes to the next field (Shift-Tab back), round at
  the ends (`key/ui.rs:440`, `reel/screens.rs:960`, `shelf/main.rs:1452`). **A press elsewhere
  takes what was typed, and the press then does what it does** (`shelf/main.rs:1470`,
  `pal/ui.rs:426`): text is never lost to a click.

### 10. Lists and tiles

- `ui::List`: rows 60 high, 4 apart, radius 14; the cursor's row washed `selected()` with the
  keys, `selected_away()` without; hover `hover()`; a heading row is `FOOTER_EM` in `ink_hint()`
  and is never the cursor; trailing word in `ink_trailing()` at `ROW_EM * 0.8`.
- `ui::Grid` / `ui::Strip` / `ui::Tile`: the cursor's tile in a ring; captions under.
- Up / Down / PageUp / PageDown / Home / End move; Enter chooses; a press chooses on release
  over the same row.
- **Several chosen at once** exists only in SpatiShelf (the SDK has none) and is the model:
  click chooses one, Control/Command-click adds, Shift-click and Shift-arrows extend, Space
  marks the cursor's, Control/Command-A all, Escape none, a drag on empty ground is a band.
  Chosen: `selected()` (list) or a ring of `mark()` (tile); the cursor alone:
  `selected_away()` / `wash(0.30)`.

### 11. Scrollbars and scrolling

- A track `theme::SCROLL_WIDTH` (5) wide at the right edge, `scroll_track()`, with a thumb of
  `scroll_thumb()`, ends rounded; never under 20 long (`SCROLL_WIDTH * 4`). It shows only when
  there is more than fits and is not dragged.
- A wheel or two fingers scroll what is under the pointer; a list by whole rows, a grid by
  pixels; the cursor comes with the view. No momentum of the app's own (the carousel in
  SpatiLoupe is its own thing). Arrow keys bring the cursor back on show.

### 12. Menus, questions, dialogs

- **Context menu**: right click, or a press held 800 ms without moving (`shelf/hand.rs:21`: a
  head pointer has no right button). Placed at the pointer, kept 12 inside the window; a shadow,
  `theme::ground()`, `look::glass(.., 20.0, 1.0)`, a `ui::List` inset 10. Rows: an icon, a verb,
  the key that does the same as the trailing word. Escape or a press outside closes it and does
  nothing else; arrows and Enter work it.
- **A question** is `ui::Confirm::new(text, yes, no)`: the text says what will happen and what
  is lost; **the no is on the left, the yes on the right**; the yes is labelled with the verb
  ("Delete", "Quit", "Forget"), **the no is "Cancel"**; Escape is no; Return answers with the
  lit button; arrows and Tab go between the two; Space is not an answer.
- **The safe answer is lit to begin with when the yes destroys or ends something** (delete,
  empty, forget, remove, quit or close with work running). The SDK lights the yes always;
  SpatiKey's own copy lights the no: see Decisions 1 and SDK consolidation 3.
- A pane that only shows something closes with **Done** at its foot right (Escape and Enter do
  the same). A screen reached from another has **Back** (or the place's name: "Library",
  "Folder", "Home") at its head left, and Escape goes back.

### 13. Empty states, errors, progress

- **Empty**: one sentence in the middle of the place, `look::message(.., theme::ink_detail())`,
  ending in a full stop: "This folder is empty.", "Nothing here to watch."
- **Waiting**: `look::waiting` (three dots) in the middle; with words, the words end in an
  ellipsis: "Loading…".
- **What went wrong**: at the foot, `look::foot(.., Some(trouble))` in `look::trouble()`,
  wrapped, for **8 seconds** (`shelf/main.rs:2238`, `quill/main.rs:1133`, the room's
  `notices::GLANCE`). **What was done**: the same line in `ink_hint()` for **5 seconds**
  (`shelf/main.rs:2244`). A place that cannot be shown at all says why in its middle,
  `look::message(.., look::trouble())`.
- Wording: a whole sentence, sentence case, a full stop; what happened and, after a colon, why
  or what to do: "SpatiQuill would not start: {e}". Never a bare error code.
- **Progress of a long job**: a bar 8 high at the foot right, `scroll_track()` filled with
  `look::TEAL`, and "{what}  {done} of {total}" beside it (`shelf/main.rs:2224-2234`).

### 14. Cards and notices from the room

- The room's cards are drawn by the room, always dark, at `panel.rs`'s sizes (the same numbers
  as `ui::theme`). A notice stays 8 seconds (`room/notices.rs:28`). Its first line names who
  says it ("Dictation", "SpatiKey"), the rest follows after a line break.
- In a card a destructive row is pressed twice ("A again to forget", `room/menu.rs:314`):
  cards have no question pane. In an app's window it is a question (rule 12).
- A card's footer names pad buttons: "A choose   B back" (`room/menu.rs:134`). See Decisions 4.

### 15. Keyboard

"Cmd" below is Command on the Mac and Control elsewhere (`modifiers.control || modifiers.logo`).
Letter shortcuts are read by the key's place: `Input::Key { key }` is already through
`chord_char` when Control or the logo key is held, and `Input::Key { letter }` is for unmodified
letters. Every app does this today; none reads a raw layout letter for a shortcut.

| Key | Shelf | Quill | Pal | Key | Reel | Loupe |
|---|---|---|---|---|---|---|
| Cmd-C / X / V | files | copy / - / paste (see `quill/keys.rs:42`) | field | field | field | field |
| Cmd-A | all files | - (menu: Select all) | field | field | field | field |
| Cmd-Z | - | - | - | - | - | - |
| Cmd-T / Cmd-W | new tab / close tab | new tab / close tab | - | - | - | - |
| Cmd-Q | quit (`Event::Quit`, the SDK's) in all six | | | | | |
| Cmd-F | find in folder | - | - | - (field always on show) | - | find in a PDF (also `/`) |
| Cmd-N | Shift: new folder | new window | - | new login | - | - |
| + / - / 0 | - | with Cmd: text size | - | - | - | plain: zoom, reset |
| Escape | closes menu, dialog, field; else clears the choice | closes menu / question | drops an edit | back; empties the search | back / Home; closes the player | closes a field or pane; else back to the folder |
| Enter | open | (the program's) | send | open / next | open; play or pause | - |
| Backspace | up a folder | (the program's) | field | field | up a folder; closes the player | up a folder |
| Tab | Cmd-Tab: next tab | Ctrl-Tab: next tab | field | next kind / next field | next field | - |
| Delete | to the Trash; Shift: for good | (the program's) | Cmd-Delete: delete the chat | - | forget a server | - |
| Others | Cmd-D duplicate, R refresh, H hidden, 1 / 2 view, L path, F2 rename, Space mark | Cmd-1..9 tab, Cmd-M move tab, Shift-PgUp/PgDn history | - | Cmd-L lock | M menu, N next, arrows seek | I about, R turn, 3 eyes, X swap, P page, O contents, N next found |

- Nothing binds Cmd-Z: there is no undo anywhere. That is the norm, not a gap to fill here.
- **Key repeat**: the SDK repeats held keys itself (`app/lib.rs:704`). `Input::of` drops a
  repeated Escape (`app/ui/mod.rs:185`). A key that toggles, confirms or destroys must not act
  on repeat (`reel/main.rs:624`, `key/main.rs:966`); arrows, Backspace and typing repeat.

### 16. Pointer and pad

- A pad's stick and D-pad arrive as arrow keys, A as Enter, B as Escape: every screen works
  from the keys alone (`app/ui/widgets.rs:6`).
- **One press opens** in a `List`, `Grid`, `Strip` (on release). **Two presses open** only
  where one press chooses for something else: SpatiShelf's files. Two presses are within
  **450 ms** and 6 of each other (`widgets.rs:775`).
- A press becomes a drag after the pointer has gone 7 to 8 (`shelf/hand.rs:19`,
  `loupe/swipe.rs:25`); see Decisions 3.
- Right click: the context menu, where there is one (SpatiShelf, SpatiQuill).
- Wheel: scrolls; in SpatiLoupe zooms a picture; in SpatiReel wakes the controls.

### 17. Controls that hide themselves

SpatiReel's player and SpatiLoupe's picture and object views (a PDF's controls stay):

- They stay **3 seconds** after the wearer last did something (`reel/play.rs:92`
  `CONTROLS_SHOWN`), then go; SpatiLoupe fades them over half a second.
- **What wakes them**: a press, a key, a scroll, and the room saying the hands stirred
  (`Event::Hands`). Pointer motion alone does not, once the room has ever said `Hands`: the
  pointer sweeps with the head.
- **A press where a hidden control is only brings the controls back; it presses nothing**
  (`reel/play.rs:1008-1011`, `loupe/view.rs:1319`).
- They are not hidden while a menu, a pane or a field of theirs is open.

### 18. Wording

- **Sentence case** everywhere: "Next episode", "Open in a new tab". Proper names keep their
  capitals (SpatiQuill, Touch ID, the Trash).
- **Verbs**: **Delete** destroys the thing for good. **Move to the Trash** puts it where it can
  be put back. **Forget** drops something this device remembers about somewhere else, whose own
  data is untouched: a remote, a server, a paired device, a Bluetooth device, an "always open
  with". **Remove** takes a member out of something shared (a device out of the vault's
  holders). **Empty** is the Trash's. **Cancel** is the no of every question and form; "Not
  now", "Keep it", "No" are not used. **Done** closes a pane that only shows; **Back** leaves a
  screen; "Close" is only a tab's or a window's. **Open** opens a thing; **Show** changes how
  things are shown ("Show as a list", "Show hidden files"). **Find** looks for something where
  you are ("Find in this folder"); "Search" is not used. **Add** makes a new entry or
  connection; "New" names the thing made ("New folder", "New login").
- **Ellipsis** (`\u{2026}`, never three dots): after a menu row that asks something before it
  acts ("Open with…", "Delete for good…"), and after words for something going on ("Loading…",
  "Emptying the Trash…"). Not on buttons.
- **Full stop**: on every whole sentence, also in the foot line. Not on labels, titles, hints.
- **Too long**: a label is cut at the end by its rectangle (`Painter::label` clips); a trail of
  folders keeps its last five and starts with "…" (`shelf/main.rs:1951`). Nothing is cut in the
  middle today.
- **Dates**: "Today 14:47", "4 Oct", "4 Oct 2025" (`shelf/look.rs:93` `when`): 24 hours, day
  before month, the month as three letters.
- **Sizes**: "812 bytes", "1.2 MB", "34 GB": thousands, one decimal under ten
  (`shelf/look.rs:66` `size`).
- **Durations**: "1:15", "1:02:03"; a position and a length "1:15 / 9:58"; a length only judged
  "0:06 / about 0:58"; a length not known is left out (`reel/play.rs:206-229`). A span in words:
  "1 minute", "5 minutes" (`key/ui.rs:403`).
- **Counts**: "1 item", "3 items".
- **Chords** in menus and hints: "Ctrl+C", "Shift+Delete" elsewhere and "⌘C", "⇧⌘N" on the Mac
  (`shelf/hand.rs:206` `keys`). **Keys** in a foot hint: `"[Enter] Open  [Esc] Back"` drawn as
  keycaps by `look::keys`; a capitalised verb after the keys, two spaces between groups; the
  names are Enter, Esc, Tab, Space, Backspace, Delete, PgUp, PgDn.
- **Platform names**: this machine is "This Mac" / "This device" (Android) / "This computer"
  (`reel/source/local.rs:16-23`). A sensor is named by the holder's kind: `key/ui.rs:63`
  `sensor()` ("Touch ID", "your fingerprint") and `sensor_named()` mid-sentence. Where a
  password is to be kept: `key/ui.rs:81` `keeper()` ("Apple Passwords", "your password
  manager"). The file manager: "Finder" on the Mac, "the file manager" elsewhere.
- **The glasses are "the glasses"**, never a maker's name; the room renames their sound devices
  so (`room/system.rs:1020`). No user-visible string in the apps or the room's UI names a brand
  of glasses today.

### 19. `describe`

Every window answers `Event::Describe` with `App::describe(window, &Description { .. })`:
`kind` (one of `terminal`, `web`, `files`, `video`, `viewer`, `chat`, `text`), `app` (its
name), `title` (what is shown), `text` (the field being typed in if one is, else what is
shown), and where they apply `vocabulary`, `directory`, `commands`.

| App | kind | Gives |
|---|---|---|
| SpatiShelf | `files` | app, title, directory, names (`shelf/main.rs:1594`) |
| SpatiQuill | `terminal` | app, title, the screen, directory, commands (`quill/main.rs:867-882`) |
| SpatiPal | `chat` | app, title, text (`pal/main.rs:717`) |
| SpatiReel | `video` | app, title, text (`reel/main.rs:563`) |
| SpatiLoupe | `viewer` | app, title, text (`loupe/main.rs:453`) |
| SpatiKey | none | title only, on purpose: nothing of a vault is read out (`key/main.rs:955`) |

---

## Part 2: the departures

Size: S a line or two, M up to a screenful, L more. "[SDK n]" means the row goes away, or
shrinks to a call, once SDK consolidation n is done: do those first.

### SDK consolidation (do first)

| # | Where | Now | Change | Size |
|---|---|---|---|---|
| 1 | `shelf/main.rs:1979-2026`, `quill/main.rs:1366-1453` | Each draws the same closable tab bar by hand; widths differ (Shelf 150 to 320, Quill 110 to 330 and scrolling) | Add `ui::TabBar` to `app/ui/widgets.rs` from SpatiQuill's (it has `bar_tabs` and its tests): `draw(p, gl, bar, &[TabLook { label, dot: Color }], active, focused)`, `input` answering `Chose(i)`, `Close(i)`, `New`; widths 110 to 330; close and "+" hit rectangles 44; `TabBar::HEIGHT = 66.0`. Both apps use it; drag-to-tear stays SpatiQuill's | L |
| 2 | `shelf/main.rs:2276-2294`, `:2422-2429`, `:1356-1370`; `quill/main.rs:217-225`, `:1074-1124`, `:1230-1237`; `reel/play.rs:848-856`, `:991-1004` | Three context menus built from a `List` on a hand-drawn pane; widths 620, 420, 640; SpatiReel's has a rim and a literal ground instead of glass | Add `ui::Menu { at, list }`: `rect(rows, size)` (as wide as its widest row, 320 to 620, 12 inside the window, no taller than it), `draw` (shadow, `theme::ground()`, `look::glass(.., 20.0, 1.0)`, list inset 10), `input` answering `Chose(i)` / `Closed` (Escape, a press outside). All three use it | M |
| 3 | `app/ui/widgets.rs:1297-1299`, `key/ui.rs:454-517` | The SDK's `Confirm` lights the yes; SpatiKey has its own copy that lights the no, centred, radius 34, a scrim of ground at 72% | Add `Confirm::careful(text, yes, no)` (as `new`, `on_yes: false`). Delete SpatiKey's copy; it uses `ui::Confirm::careful` (its `draw(p, gl)` calls become `draw(p, gl, p.bounds())`) | M |
| 4 | `app/ui/widgets.rs:142-149`, `:453-459`; `shelf/main.rs:2208-2214`; `quill/main.rs:1205-1209` | Four copies of the scrollbar; the apps' thumbs are at least 30, the SDK's 20 | Add `look::scrollbar(p, gl, track: Rect, shown: f32, total: f32, offset: f32)` with the SDK's numbers; all four call it | S |
| 5 | `shelf/look.rs:66-115`, `loupe/folder.rs:123-137`, `key/ui.rs:413-423`, `pal/time.rs:61-69` | `size` twice (identical); a date four ways: "Today 14:47" / "4 Oct" / "4 Oct 2025" (Shelf), "29 Oct 2026" always (Key), "14:47" / "4/10" (Pal) | Move Shelf's `size`, `local` and `when` (with its test) to `app/ui/look.rs` as `look::size`, `look::when`; the four call them | S |
| 6 | `key/ui.rs:248-259`, `reel/screens.rs:611-650`, `:826-838`, `loupe/folder.rs:395-414` | A screen's head (a Back button, the title at `TITLE_EM * 0.9`, a quieter word beside, the rule) built four times; the gaps under it are 12/2/16, 12/2/22, 14/2/24 | Move SpatiKey's `top` to `look::top(p, gl, all, back, back_label, back_icon, title, beside) -> Rect` with gaps 12 / 2 / 18 (as `look::header`); the four call it | M |
| 7 | `app/ui/widgets.rs:775-776` | `DOUBLE_CLICK` (450 ms) and `DOUBLE_CLICK_SLOP` (6) are private; three apps wrote their own (400, 400, 450) | Make them `pub`, re-export from `ui`; add `pub const DRAG_AFTER: f32 = 8.0` (Decisions 3) | S |
| 8 | `app/ui/widgets.rs:348`, `:1247`; `shelf/main.rs:1991`, `:2138`, `:2000`, `:2296`, `:2314`, `:2343`; `shelf/hand.rs:980-999`; `quill/main.rs:1221`, `:1425`, `:1436` | The blue glow `[0.30, 0.62, 1.0, a]`, the scrim `[0, 0, 0, 0.35]` and the busy amber `[1.0, 0.85, 0.4, 1.0]` are literals in each place | Add `theme::glow(alpha) -> Color`, `look::scrim(p, gl, all)` and `pub const AMBER: Color` in `look.rs`; replace the literals | S |
| 9 | `shelf/hand.rs:206-208` | `keys(letter)` ("⌘C" / "Ctrl+C") is SpatiShelf's alone | Move to `look::chord(keys: &str) -> String`; SpatiShelf and SpatiQuill use it | S |
| 10 | `reel/play.rs:92-150` (`Clock`), `loupe/view.rs:30`, `:658-674` | Two clocks for controls that hide; 3.0 s and 3.2 s | Move SpatiReel's `Clock` and its tests to `ui::AutoHide` with `pub const CONTROLS_SHOWN = 3 s`; SpatiLoupe uses its `wake` / `stir` / `on` and keeps its own fade | M |
| 11 | `shelf/main.rs:2047-2054`, `quill/main.rs:1195-1203` | "This tab stopped" pane twice: 680x200 and 720x220, different line heights | Add `look::stopped(p, gl, area, why, what_now)` with SpatiQuill's numbers; both call it | S |
| 12 | `reel/source/local.rs:16-23` | "This Mac" / "This device" / "This computer" is SpatiReel's alone | Move to `look::this_device() -> &'static str` | S |
| 13 | `app/ui/widgets.rs:351`, `:353`, `:976`; `app/ui/look.rs:185-187` | Inside the SDK: a tile's cursor and hover rings are white at 28% and 16% (not seen on the light ground); a field's selection is a literal blue; a keycap is literal dark | `theme::wash(0.28)`, `theme::wash(0.16)`; the selection `{ let mut c = theme::selected(); c[3] = 0.42; c }`; the keycap's rim `theme::wash(0.16)`, face `theme::raised()` over `theme::ground()` | S |
| 14 | `app/ui/mod.rs:185` | `Input::of` drops a repeated Escape only; a held Enter answers a question that came up under it | Drop `repeat: true` for `Key::Enter` and `Key::Space` too. (SpatiQuill reads raw `Event::Key` for the terminal, `quill/main.rs:663`, and is not touched; check `Field` users want no repeated Enter: none does) | S |

### SpatiShelf

| # | Where | Now | Rule | Change | Size |
|---|---|---|---|---|---|
| 1 | `main.rs:1979-2026` | Own tab bar; close cross hit 38 | 6, 5 | `ui::TabBar` [SDK 1] | S after |
| 2 | `main.rs:2276-2294`, `:2422-2429`, `:1356-1370` | Own menu pane, always 620 wide | 12 | `ui::Menu` [SDK 2] | S after |
| 3 | `main.rs:2208-2214` | Own scrollbar, thumb at least 30 | 11 | `look::scrollbar` [SDK 4] | S |
| 4 | `main.rs:1053` | Deleting for good asks with "Yes" / "No", the yes lit | 12 | `Confirm::careful(text, "Delete", "Cancel")` | S |
| 5 | `main.rs:1175` | Forgetting a remote: "Yes" / "No", the yes lit | 12 | `Confirm::careful(.., "Forget", "Cancel")` | S |
| 6 | `main.rs:1188` | Emptying the Trash: "Yes" / "No", the yes lit | 12 | `Confirm::careful(.., "Empty", "Cancel")` | S |
| 7 | `main.rs:715`, `:731` | Closing a tab, quitting, with work going: the yes lit | 12 | `Confirm::careful` (labels stay) | S |
| 8 | `main.rs:2302`, `:2317` | Pane titles at `TITLE_EM * 0.75` | 1 | `theme::TITLE_EM * 0.8` | S |
| 9 | `main.rs:2155` | A tile's second line at `FOOTER_EM * 0.8` (18) | 1 | `theme::FOOTER_EM * 0.82` | S |
| 10 | `main.rs:2083` (`header` 40.0), `:2113-2119` | The list's column heads are 40 high to press | 5 | `(ROW, 44.0)` | S |
| 11 | `main.rs:2409` `DISCLOSURE: f32 = 30.0` | A folder's triangle is 30 wide to press | 5 | `44.0` (the icon stays 22; `list_layout` moves the name by the 14 more) | S |
| 12 | `main.rs:989` | "Cut 1 item — paste where it is to go" has no full stop | 18 | End the format string with `.` | S |
| 13 | `hand.rs:283` | Double click is a literal 450 ms | 16 | `ui::DOUBLE_CLICK` [SDK 7] | S |
| 14 | `hand.rs:19` | `DRAG_AFTER` 7.0 | 16 | `ui::DRAG_AFTER` [SDK 7] | S |
| 15 | `hand.rs:972`, `:974`, `:981`, `:995` | Drop-target washes are literal blues (the dark look's, in the light look too) | 2 | `:972` `theme::selected()`; `:974` `theme::mark()`; `:981`, `:995` `theme::selected()` | S |
| 16 | `hand.rs:980`, `:983`, `:994`, `:999`; `main.rs:1991`, `:2138`; `main.rs:2000`; `main.rs:2296`, `:2314`, `:2343` | Literal glow, amber and scrim | 2 | `theme::glow(a)`, `look::AMBER`, `look::scrim` [SDK 8] | S |
| 17 | `look.rs:66-115`, `hand.rs:206-208` | Own `size`, `when`, `keys` | 18 | `look::size`, `look::when`, `look::chord` [SDK 5, 9] | S |
| 18 | `main.rs:2047-2054` | Own "This tab stopped" pane | 13 | `look::stopped` [SDK 11] | S |
| 19 | `main.rs:2433` | A trashed thing of unknown origin reads "from where, not known" | 18 | Word it `"not known"` | S |

### SpatiQuill

| # | Where | Now | Rule | Change | Size |
|---|---|---|---|---|---|
| 1 | `main.rs:1366-1453` | Own tab bar; close cross hit 38 | 6, 5 | `ui::TabBar` [SDK 1] | S after |
| 2 | `main.rs:217-225`, `:1074-1124`, `:1230-1237` | Own menu pane, 420 wide | 12 | `ui::Menu` [SDK 2] | S after |
| 3 | `main.rs:1063`, `:1065` | The menu says "Ctrl+C", "Ctrl+V" on the Mac too | 18 | `look::chord("C")`, `look::chord("V")` [SDK 9] | S |
| 4 | `main.rs:774`, `:1081` | The menu opens on a right click only: no way to it with a head pointer | 12 | A press held 800 ms within `ui::DRAG_AFTER` on the text opens it at the pointer, as `shelf/hand.rs:21`; the press then chooses nothing | M |
| 5 | `main.rs:848` | Double click is a literal 400 ms | 16 | `ui::DOUBLE_CLICK` [SDK 7] | S |
| 6 | `main.rs:1188` | The caret and the selection are literal colours | 2 | `theme::always_dark(theme::mark)` and `[look::BLUE[0], look::BLUE[1], look::BLUE[2], 0.40]` | S |
| 7 | `main.rs:1205-1209` | Own scrollbar | 11 | `look::scrollbar` [SDK 4] | S |
| 8 | `main.rs:1211` | "{n} lines up    Shift-End goes to the foot": a chord written with a hyphen, in a sentence | 18 | `format!("{} lines up", n)` and, after it in the pill, `look::keys(.., "[Shift+End] Foot", ..)` | S |
| 9 | `main.rs:1195-1203` | Own "This tab stopped" pane | 13 | `look::stopped` [SDK 11] | S |
| 10 | `main.rs:384` | Closing or quitting with something running: the yes lit | 12 | `Confirm::careful` (labels stay) | S |
| 11 | `main.rs:1221`, `:1425`, `:1436` | Literal glow and amber | 2 | `theme::glow(a)`, `look::AMBER` [SDK 8] | S |
| 12 | `main.rs:1240` | What went wrong is capitalised as it is drawn, because it is made in lower case | 13 | Capitalise where each is made (the `failed = Some((..))` sites) and end each with a full stop; drop the capitalising here | S |
| 13 | whole window | In the light look the bar and margins are light and the terminal is its dark palette | 2 | Decisions 2 | S |

### SpatiPal

| # | Where | Now | Rule | Change | Size |
|---|---|---|---|---|---|
| 1 | `ui.rs:11`, `:227`, `:241-248`, `:389-407` | A chat is deleted by pressing its bin twice within 4 s (the second press's label "Delete"); Cmd-Delete the same | 12 | One press (or Cmd-Delete) asks `Confirm::careful("Delete this chat? It cannot be had back.", "Delete", "Cancel")`; drop `ARMED_FOR` and `armed` | M |
| 2 | `time.rs:61-69` | A chat's time is "14:47" or "4/10" | 18 | `look::when` [SDK 5] | S |
| 3 | `main.rs:147` | What went wrong stays 12 s | 13 | `Duration::from_secs(8)` | S |
| 4 | `main.rs:238` | "the room would not give its microphone": lower case, no full stop | 13, 18 | `"The room would not give its microphone."`; give the same treatment to what reaches `say_trouble` at `main.rs:414`, `:439`, `:543`, `:546`, `:685` (capitalise the first letter and end with a full stop inside `say_trouble`, `main.rs:264`, if not already so) | S |
| 5 | `ui.rs:165` | Window margin `theme::PAD * 0.6` | 4 | `theme::PAD` | S |
| 6 | `ui.rs:270-282` | The conversation scrolls with no scrollbar | 11 | `look::scrollbar(p, gl, Rect { x: all.right() - theme::SCROLL_WIDTH, w: theme::SCROLL_WIDTH, ..all }, all.h, tall, tall - all.h - self.scroll)` when `tall > all.h` [SDK 4] | S |
| 7 | `ui.rs:287-291` | Settings is a screen left by "Done" at the foot right | 12 | `look::top(.., &mut self.back, "Back", Icon::Back, "Settings", "")` at its head [SDK 6]; Escape with no edit open goes back | M |
| 8 | `ui.rs:211` (the "+" button) | A new chat has no key | 15 | Cmd-N starts one (`Input::Key { key: Key::Char('n'), .. }` with Control or logo) | S |
| 9 | `ui.rs:293` | The edit field's hint is a sentence about keys: "… — Enter or a click elsewhere keeps it, Escape drops it" | 18 | Hint `label(line)` only; under the list, `look::keys(.., "[Enter] Keep  [Esc] Drop", 0.0, 1.0)` | S |

### SpatiKey

| # | Where | Now | Rule | Change | Size |
|---|---|---|---|---|---|
| 1 | `ui.rs:454-517` | Own `Confirm` | 12 | `ui::Confirm::careful` [SDK 3] | S after |
| 2 | `open.rs:442`, `:1249`, `:1263` | The no is "Keep it" | 18 | `"Cancel"` | S |
| 3 | `open.rs:659-661` | "Load it" / "Not now" | 18 | `"Load it"` / `"Cancel"` | S |
| 4 | `open.rs:835` | "Leave" / "Keep editing" | 18 | `"Leave"` / `"Cancel"` | S |
| 5 | `main.rs:932-934`, `:940` | Quitting with a code unsaved or changes unsaved: SDK `Confirm::new`, the yes lit | 12 | `Confirm::careful` | S |
| 6 | `main.rs:379`, `:396`, `:398`, `:423`, `:428`, `:433`, `:444`, `:457` ("Forgetting"), `:465` | Words for something going on have no ellipsis: "Opening", "Saving", "Looking for Macs on this network" | 18 | End each with `\u{2026}` | S |
| 7 | `open.rs:56` | The field's hint is "Search" | 18 | `"Find in SpatiKey"` | S |
| 8 | `main.rs:955` | `describe` gives a title and no app | 19 | `Description { app: TITLE.into(), title: TITLE.into(), ..Default::default() }` (kind and text stay empty) | S |
| 9 | `ui.rs:413-423` | A date is always "29 Oct 2026" | 18 | `look::when` [SDK 5] | S |
| 10 | `ui.rs:248-259` | Own `top` | 4 | `look::top` [SDK 6] | S |
| 11 | `ui.rs:232` | Window margin `theme::PAD * 0.8` | 4 | `theme::PAD` | S |
| 12 | `main.rs:53` | What was done stays 6 s | 13 | `Duration::from_secs(5)` | S |
| 13 | `locked.rs:562` | "This device" on every platform | 18 | `look::this_device()` [SDK 12] | S |
| 14 | `locked.rs:566`, `:585`, `:594`, `:600`, `:618`, `:623`, `:784`; `main.rs:365`; `open.rs:1153`; `ui.rs:372`, `:376`, `:382`, `:384` | A device that asks another says "your Mac" and "Touch ID on {mac}", though since `b35ed2c` the holder that answers may be a phone | 18 | Name the holder by its kind: "Touch ID on {name}" / "your fingerprint on {name}" through `ui::sensor(kind)`; "your Mac" becomes "a device that opens SpatiKey" in prose and the holder's name on buttons ("Pair with {name}") | M |
| 15 | `locked.rs:90` | Prose names "The Steam Deck and the Beam Pro" | 18 | `"Devices with no sensor of their own ask one of them each time, …"` | S |

### SpatiReel

| # | Where | Now | Rule | Change | Size |
|---|---|---|---|---|---|
| 1 | `screens.rs:317-333`, `:465-467`, `:621-626`, `:698` | A server is taken away by pressing Delete, or its button, twice within 4 s ("Press again to remove") | 12 | One press asks `Confirm::careful(format!("Forget {name}? Its films are not touched; SpatiReel signs out of it."), "Forget", "Cancel")`; drop `removing` | M |
| 2 | `screens.rs:297`, `:622` | "Remove a server", "Remove" | 18 | `"[Enter] Open  [Delete] Forget a server"`, `"Forget"` | S |
| 3 | `play.rs:841-842` | "Next Episode" | 18 | `"Next episode"` (both lines) | S |
| 4 | `play.rs:848-856`, `:991-1004` | Own menu card: a rim and a literal ground `[0.043, 0.05, 0.072, 0.97]` | 12, 2 | `ui::Menu` [SDK 2] | S after |
| 5 | `screens.rs:611-650`, `:826-838` | Own screen heads | 4 | `look::top` [SDK 6] | S |
| 6 | `play.rs:92-150` | Own `Clock` | 17 | `ui::AutoHide` [SDK 10] | S |
| 7 | `source/local.rs:16-23` | Own "This Mac" words | 18 | `look::this_device()` [SDK 12] | S |
| 8 | `screens.rs:654` | In a local folder the foot shows the path and no keys | 18 | `"[Enter] Open  [Backspace] Up  [Esc] Home"` there too; the path goes beside the title as `beside` | S |
| 9 | `play.rs:807` | The shade behind the controls is a literal `[0.02, 0.025, 0.04, 0.78]` | 2 | `{ let mut g = theme::ground(); g[3] = 0.78; g }` | S |

### SpatiLoupe

| # | Where | Now | Rule | Change | Size |
|---|---|---|---|---|---|
| 1 | `view.rs:30` | The controls stay 3.2 s | 17 | `ui::CONTROLS_SHOWN` (3 s) [SDK 10] | S |
| 2 | `view.rs:1367` | Double click is a literal 400 ms | 16 | `ui::DOUBLE_CLICK` [SDK 7] | S |
| 3 | `swipe.rs:25` | `DECIDE_AFTER` 8.0 | 16 | `ui::DRAG_AFTER` [SDK 7] | S |
| 4 | `folder.rs:123-137` | Own `size_words` | 18 | `look::size` [SDK 5] | S |
| 5 | `folder.rs:395-414` | Own screen head | 4 | `look::top` [SDK 6] | S |
| 6 | `view.rs:1791` | What a search found is washed in literal teal and blue | 2 | `[look::TEAL[0], look::TEAL[1], look::TEAL[2], 0.45]`, `[look::BLUE[0], look::BLUE[1], look::BLUE[2], 0.25]` | S |
| 7 | `view.rs:1707` | The SDK's default tile tint copied as a literal | 2 | Make it `pub const TILE_TINT` in `app/ui/widgets.rs:361` and use it | S |
| 8 | `view.rs:1114-1117` | What went wrong is centred, and stays until the next file | 13 | Left, as `look::foot`; cleared 8 s after it was set | S |
| 9 | `view.rs:1049-1093` | The top bar, the strip and the hint are placed 28 and 24 to 26 from the window's edge | 4 | Leave: they float over a picture. Listed so that a later job does not "fix" it | - |

### The room's own UI

| # | Where | Now | Rule | Change | Size |
|---|---|---|---|---|---|
| 1 | `shell/environment.rs:15` | "Add an image..." with three dots | 18 | `"Add an image\u{2026}"` | S |
| 2 | `shell/hud.rs:167` | "Looking for sound devices" | 18 | `"Looking for sound devices\u{2026}"` | S |
| 3 | `room/startup.rs:65-74` | "Starting the graphics", "Loading the room", … with no ellipsis | 18 | End each with `\u{2026}` | S |
| 4 | `shell/hud.rs:474` | "Press A to use this one" in a row's detail (the footer already says "A use") | 18 | `"Choose to use this one"`, as the other details ("Choose to change") | S |
| 5 | `room/dictate.rs:204` | "Listening — press 🎤 to stop": a colour emoji, blank on the Beam Pro | 7 | `"Dictation\nListening \u{2014} press the microphone key to stop"` | S |
| 6 | `room/spatikey.rs:372` | "Caps Lock is on: turn it off and try again" has no full stop | 18 | Add `.` | S |
| 7 | `shell/launcher.rs:214` | This machine is "This computer" on every platform | 18 | "This Mac" on macOS, "This device" on Android, as `reel/source/local.rs:16-23` (the shell does not link the SDK: three `cfg!` arms) | S |
| 8 | `choose/main.rs:400-406` | The chooser draws its two buttons by hand; its yes is filled `theme::selected()` / `theme::mark()`, the only "primary" look anywhere | 8 | Two `ui::Button`s; the yes `focused = true` | M |
| 9 | `choose/main.rs:388` | Its title at `TITLE_EM * 0.7` | 1 | `theme::TITLE_EM * 0.8` | S |
| 10 | `choose/main.rs:309`, `:328`, `:363` | "Type the file's name first", "Choose a file first", "Nothing is at {path}" have no full stop | 18 | Add `.` | S |
| 11 | `choose/main.rs` (no `describe` call) | The chooser does not answer `Event::Describe` | 19 | On `Event::Describe`: `Description { kind: "files".into(), app: "Spatiand".into(), title: self.title(), text: field text, directory, vocabulary: the names shown }` | S |
| 12 | `room/menu.rs:134-499` | Footers "A choose   B back": lower-case verbs, no keycaps | 18 | Decisions 4 | M |

### Decisions for the owner

1. **A question that destroys starts on Cancel.** The SDK's `Confirm` lights the yes (Return
   deletes); SpatiKey's own lights the no. Proposed: `Confirm::careful` for delete, empty,
   forget, remove, and quitting or closing with work running, because a Return typed on from
   what came before then loses nothing; every other question keeps the yes lit.
2. **SpatiQuill in the light look stays dark, bar and all** (its `draw` inside
   `theme::always_dark`), because a terminal's colours are made for a dark ground and a light
   frame round a dark screen is neither look. The other way is a light palette for the cells.
3. **One distance at which a press becomes a drag: 8** (`ui::DRAG_AFTER`; Shelf has 7, Loupe 8).
   SpatiReel's 12 for "a tap on the picture" stays as it is, because a head pointer drifts more
   during a press than a hand does and that 12 was tuned on the glasses.
4. **The room's card footers become keycaps with a capitalised verb, pad letters kept**
   ("[A] Choose  [B] Back"), because the apps' hints are read that way and one form is learned
   once. The other way is to leave the room's cards as they are and call it the room's own form.
5. **A tap on the picture puts SpatiReel's controls away at once; SpatiLoupe has no such tap**
   (a single press there is half of a double click, which zooms). Proposed: leave SpatiLoupe
   without it; only the 3 seconds and what wakes them are shared.

---

## Part 3: how to check an app

Run from the repository's root; `APP` is one of `spatishelf spatiquill spatipal spatikey
spatireel spatiloupe`. Each grep should print nothing but the lines named as right in Part 1
rule 2 or left on purpose in Part 2.

1. **Literal colours**: `grep -nE '\[[0-9.]+, ?[0-9.]+, ?[0-9.]+, ?[0-9.]+\]' crates/$APP/src/*.rs`
   Allowed: `uv` rectangles (`[0.0, 0.0, 1.0, 1.0]`), a `Tile::tint`, white ink on a tint, a
   film's subtitles, a page's paper, test code.
2. **Bare text sizes**: `grep -nE '(label|text|text_left)\(gl,' crates/$APP/src/*.rs | grep -v '_EM'`
   Every size left must be a named constant that is itself `theme::*_EM * factor`.
3. **Too small**: `grep -nE 'FOOTER_EM \* 0\.[0-7]|FOOTER_EM \* 0\.80?\b' crates/$APP/src/*.rs` prints nothing.
4. **Own widgets**: `grep -nE 'fn menu_rect|struct Confirm|fn draw_bar|fn paint_bar|scroll_thumb\(\)|fn size_words|fn when\(|from_millis\((400|450)\)' crates/$APP/src/*.rs`
   prints nothing once the SDK consolidation is done.
5. **Questions**: `grep -n 'Confirm::' crates/$APP/src/*.rs`. Each is `careful` if its yes is
   Delete, Empty, Forget, Remove, Quit, Close or Leave; no label is "Yes", "No", "Keep it",
   "Not now".
6. **Words**: `grep -nE '"[^"]*(\.\.\.|Search|Remove |Not now|Keep it|Next Episode)[^"]*"' crates/$APP/src/*.rs`
   prints nothing (SpatiKey's "Remove {holder}" is right). Words for something going on end in
   `\u{2026}`: `grep -nE '"(Loading|Opening|Saving|Looking|Making|Waiting|Typing|Fetching|Setting)[^"]*[a-z}]"' crates/$APP/src/*.rs` prints nothing.
7. **Emoji**: `grep -nP '[\x{1F300}-\x{1FAFF}]|\\u\{1F[0-9A-Fa-f]{3}\}' crates/$APP/src/*.rs` prints nothing.
8. **Shortcuts by place**: `grep -n "Key::Char('" crates/$APP/src/*.rs`. Each match is on an
   `Input::Key` (already through `chord_char`) with Control or logo held, or uses `letter`;
   none matches on a raw `Event::Key`'s `key` (SpatiQuill's go through `keys::chord_char`).
9. **Glasses by name**: `grep -rnEi '"[^"]*(xreal|nreal|viture|rokid)[^"]*"' crates/$APP/src` prints nothing.
10. **`describe`**: `grep -n 'Description {' crates/$APP/src/*.rs` shows `app:` set, and
    `kind:` set in all but SpatiKey.
11. **Tests**: `cargo test -p spatiand-app -p $APP` (deepmagpie; three /proc and timing tests
    fail on macOS by nature).

**A snapshot** (the headless test room on deepmagpie, scratch settings folders only,
`SPATIAND_HMD=null SPATIAND_INPUT=off`), once in the dark look and once in the light, should
show:

- the first screen: margin 42 all round; the title's rule in teal, blue, violet; the foot line
  at 23; nothing under 19;
- in the light look: no white-on-light or dark pane left behind (the tab bar's active tab, the
  menu, a question, a tile's hover ring, the keycaps in the foot);
- a context menu (Shelf, Quill): at the pointer, radius 20, as wide as its longest row; on the
  Mac its chords read "⌘C";
- a destructive question: "Cancel" left and lit, the verb right; Return closes it and nothing
  is lost; Escape the same;
- a field with text in it, then a click elsewhere: the text is kept;
- SpatiReel playing and SpatiLoupe on a picture, left alone: the controls are gone a little
  after 3 seconds; a press where "Library" / "Folder" was brings them back and does not leave.

---

## What was not read

- **Read in full**: `app/ui/look.rs`, `app/ui/mod.rs`, `app/ui/widgets.rs` (not its field
  tests), `app/describe.rs`.
- **Read in part, the rest searched** (every string literal beginning with a capital, every
  colour literal, every text size not a `theme::*_EM`, every `Key::` and `letter`, every
  duration and top-level constant were listed by search for each file and read; the code round
  them was read only where quoted above): all of `shelf/` except as below, `quill/main.rs`,
  `quill/keys.rs`, `pal/ui.rs`, `pal/main.rs`, `pal/time.rs`, all of `key/`, all of `reel/`,
  `loupe/view.rs`, `loupe/folder.rs`, `loupe/main.rs`, `loupe/stereo.rs`, `choose/main.rs`,
  `shell/*.rs`, `room/menu.rs`, `room/notices.rs`, `room/startup.rs`, `room/install.rs`,
  `room/dictate.rs`, `room/calib.rs`, `room/spatikey.rs`, `panel.rs`.
- **Strings that begin in lower case were not listed by the search** and so were seen only
  where the code round them was read: lower-case status words and error texts made deep in
  `shelf/tab.rs`, `shelf/trash.rs`, `reel/source/*`, `pal/brain.rs`, `pal/ears.rs`,
  `loupe/pdf.rs`, `quill/main.rs` (its `failed` sites) may hold more departures from rule 13's
  wording. Quill row 12 and Pal row 4 say how to treat them.
- **Not read at all**: `shelf/ds_store.rs`, `shelf/proto.rs`, `shelf/watch.rs`,
  `shelf/openwith_*.rs` beyond their strings, `shelf/tree.rs`, `shelf/setup.rs` beyond its
  strings and keys; `quill/grid.rs`, `quill/tab.rs`, `quill/proc.rs`, `quill/wire.rs` beyond
  their colours; `pal/brain.rs`, `pal/ears.rs`, `pal/voice.rs`, `pal/tools.rs`, `pal/store.rs`,
  `pal/warm.rs` beyond their strings (what the assistant says aloud and its prompts are out of
  this list); `key/demo.rs`, `key/host.rs`, `key/ask.rs`, `key/generate.rs`; `loupe/model.rs`,
  `loupe/pdf.rs`, `loupe/picture.rs`, `loupe/place.rs`, `loupe/swipe.rs` beyond their
  constants; `app/ui/paint.rs`, `app/ui/icons.rs` beyond their signatures and the stroke;
  `app/lib.rs`, `app/keys.rs`, `app/wire.rs` beyond the lines quoted.
- **The room**: `room/scene.rs` (4686 lines: how the launcher, cards, notices and the keyboard
  are drawn) was searched for its colour constants only; `room/keyboard_face.rs`,
  `shell/keyboard.rs`, `shell/launcher.rs`, `shell/grid.rs`, `shell/switcher.rs`,
  `room/notifications.rs`, `room/controls.rs`, `room/shutdown.rs`, `room/attention.rs`,
  `room/waiting.rs`, `room/assist.rs`, `room/pip.rs`, `room/status.rs` were searched for strings
  and colours and not read. The room's table is therefore its wording and the chooser; its
  drawing (sizes, radii, the keyboard's keys, the launcher's tiles) is not audited here.
- **SpatiMesh** (Chromium) and the Android and Mac shells' own strings (Java, Swift) were out
  of the brief and not looked at.
- **No snapshot was taken.** Three things above want one before they are changed: Quill row 13
  (what the light look does to SpatiQuill today), SDK 13 (the tile rings and keycaps in the
  light look), and Shelf rows 10 and 11 (that the larger hit rectangles do not crowd the list).
