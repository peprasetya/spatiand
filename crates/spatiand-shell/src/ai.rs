//! The AI card: what more than one app uses, in one place in the settings -- hemera's address,
//! the models, dictation, and text in pictures (the owner, 2026-11-19: "model selection *need to
//! put in general for model selection, as it may impact other app too").
//!
//! The shell keeps no settings. The compositor hands the card what they are ([`AiView`]) and what
//! hemera offers ([`Offered`]); the card says what the wearer changed ([`AiAction`]), and is handed
//! the settings again once they are kept -- in the file every paired machine shares
//! (`crates/spatiand/src/ai.rs`).
//!
//! A list with a page under some of its rows: a model or a language is chosen from a list of its
//! own, the wearer's words are a list to forget from and add to, and hemera's address and a new
//! word are typed, as a computer's address is on the remote computers page.

use crate::grid::Direction;

/// The settings, as the card shows them.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AiView {
    pub hemera: String,
    pub everyday: String,
    pub larger: String,
    pub cleanup: String,
    /// Empty: hemera's choice.
    pub reading: String,
    /// Dictation's languages, as Whisper names them (`en`).
    pub languages: Vec<String>,
    pub words: Vec<String>,
    pub read_by_itself: bool,
    pub first_language: String,
    pub second_language: String,
}

/// What hemera offers, as last asked.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Offered {
    /// Asked, and not answered yet.
    pub asking: bool,
    /// Its models that write text, by name.
    pub models: Vec<String>,
    /// Its models that read text in pictures.
    pub readers: Vec<String>,
    /// The one it reads with when none is named.
    pub reader_default: Option<String>,
    /// Why it could not be asked, for the wearer: a whole sentence.
    pub trouble: Option<String>,
}

/// One of the models the card chooses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Model {
    Everyday,
    Larger,
    Cleanup,
    Reading,
}

impl Model {
    pub fn label(self) -> &'static str {
        match self {
            Model::Everyday => "Everyday model",
            Model::Larger => "Larger model",
            Model::Cleanup => "Terminal clean-up model",
            Model::Reading => "Text reading model",
        }
    }

    /// What it does, for the card's paragraph.
    fn does(self) -> &'static str {
        match self {
            Model::Everyday => "The quick model every request goes to first: SpatiPal's answers, and any app's quick questions.",
            Model::Larger => "For what needs thought: the everyday model hands such a request over to it.",
            Model::Cleanup => "Turns what is said into a terminal into the command to type, before it is shown to you.",
            Model::Reading => "Reads the words in pictures. hemera's choice is the one it reads with when none is named.",
        }
    }
}

/// One of the two languages of text in pictures.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Language {
    First,
    Second,
}

impl Language {
    pub fn label(self) -> &'static str {
        match self {
            Language::First => "First language",
            Language::Second => "Second language",
        }
    }
}

/// What the wearer changed: for the compositor to keep.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AiAction {
    Hemera(String),
    /// A model chosen. For [`Model::Reading`], empty is hemera's choice.
    Model(Model, String),
    /// The languages dictation hears.
    Languages(Vec<String>),
    AddWord(String),
    ForgetWord(String),
    ReadByItself(bool),
    /// The first and the second language of text in pictures: choosing for one the language the
    /// other had swaps them, so that the two are never the same.
    TextLanguages(String, String),
}

/// The languages a picture's text may be translated between, by their two-letter names and as
/// they are called in English. Whisper names dictation's languages the same way.
pub const LANGUAGES: &[(&str, &str)] = &[
    ("en", "English"),
    ("id", "Indonesian"),
    ("ms", "Malay"),
    ("jv", "Javanese"),
    ("su", "Sundanese"),
    ("zh", "Chinese"),
    ("ja", "Japanese"),
    ("ko", "Korean"),
    ("th", "Thai"),
    ("vi", "Vietnamese"),
    ("tl", "Tagalog"),
    ("hi", "Hindi"),
    ("ta", "Tamil"),
    ("te", "Telugu"),
    ("ar", "Arabic"),
    ("tr", "Turkish"),
    ("ru", "Russian"),
    ("el", "Greek"),
    ("es", "Spanish"),
    ("fr", "French"),
    ("de", "German"),
    ("nl", "Dutch"),
    ("pt", "Portuguese"),
    ("it", "Italian"),
];

/// A language's name in English, or its code where it has none here.
pub fn language_name(code: &str) -> String {
    LANGUAGES.iter().find(|(c, _)| *c == code).map(|(_, n)| n.to_string()).unwrap_or_else(|| code.to_string())
}

/// Dictation's languages as they are stepped through, as SpatiPal's settings did: the owner's
/// two together, each alone, and none (Whisper decides).
const SPOKEN: [&[&str]; 4] = [&["en", "id"], &["en"], &["id"], &[]];

/// What every row about text in pictures says under the list.
const TEXT_NOTE: &str = "Text in the first language is translated into the second; anything else into the first. Nothing \
                         reads text in pictures yet: these are kept for when it does.";

/// The row that adds a word, after the words.
pub const ADD_WORD: &str = "Add a word";

/// Where the card is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Page {
    #[default]
    List,
    /// Choosing a model, from hemera's.
    Model(Model),
    /// Choosing a language of text in pictures.
    Language(Language),
    /// The wearer's own words: each one, to forget, and adding another.
    Words,
    /// Typing hemera's address.
    Hemera,
    /// Typing a word to add.
    NewWord,
}

/// A row of the list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Item {
    Heading(&'static str),
    Hemera,
    Model(Model),
    Languages,
    Words,
    ReadByItself,
    Language(Language),
}

/// The list, in its order: the server first, as everything is on it; then what is chosen there.
const LIST: &[Item] = &[
    Item::Hemera,
    Item::Heading("Models"),
    Item::Model(Model::Everyday),
    Item::Model(Model::Larger),
    Item::Model(Model::Cleanup),
    Item::Model(Model::Reading),
    Item::Heading("Dictation"),
    Item::Languages,
    Item::Words,
    Item::Heading("Text in pictures"),
    Item::ReadByItself,
    Item::Language(Language::First),
    Item::Language(Language::Second),
];

/// A row, as drawn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AiRow {
    pub label: String,
    /// Set to the right, dimmer: what it is now.
    pub value: Option<String>,
    /// The name of the group the rows under it belong to: never chosen.
    pub heading: bool,
}

impl AiRow {
    fn with(label: impl Into<String>, value: impl Into<String>) -> AiRow {
        AiRow { label: label.into(), value: Some(value.into()), heading: false }
    }

    fn plain(label: impl Into<String>) -> AiRow {
        AiRow { label: label.into(), value: None, heading: false }
    }
}

/// The card's state.
#[derive(Debug, Clone, Default)]
pub struct Ai {
    view: AiView,
    offered: Offered,
    page: Page,
    cursor: usize,
    /// The list's cursor while one of its pages is open, to come back to.
    list_cursor: usize,
    /// The words page's, while a word is typed.
    words_cursor: usize,
    typed: String,
    /// The word chosen once, to be chosen again to forget: a card has no question to ask.
    armed: Option<usize>,
    /// What went wrong keeping the last change, until the next is kept.
    note: Option<String>,
}

impl Ai {
    /// Opened from the settings: at the list, where it was left.
    pub fn open(&mut self) {
        if self.page != Page::List {
            self.cursor = self.list_cursor;
        }
        self.page = Page::List;
        self.armed = None;
        self.typed.clear();
        self.off_heading(true);
    }

    /// The settings as they are kept. The cursor stays on its row, or the last there is.
    pub fn set_view(&mut self, view: AiView) {
        self.view = view;
        self.armed = None;
        self.cursor = self.cursor.min(self.rows().len().saturating_sub(1));
        self.off_heading(true);
    }

    /// What hemera offers, as it answers. A model's page open meanwhile keeps its cursor on the
    /// model it was on, wherever hemera's list puts it.
    pub fn set_offered(&mut self, offered: Offered) {
        let on = match self.page {
            Page::Model(m) => self.choices(m).get(self.cursor).cloned(),
            _ => None,
        };
        self.offered = offered;
        if let (Page::Model(m), Some(name)) = (self.page, on) {
            if let Some(at) = self.choices(m).iter().position(|c| *c == name) {
                self.cursor = at;
            }
        }
        self.cursor = self.cursor.min(self.rows().len().saturating_sub(1));
    }

    /// What went wrong keeping a change, said on the card until the next is kept; `None` once one is.
    pub fn set_note(&mut self, note: Option<String>) {
        self.note = note;
    }

    pub fn view(&self) -> &AiView {
        &self.view
    }

    pub fn offered(&self) -> &Offered {
        &self.offered
    }

    pub fn page(&self) -> Page {
        self.page
    }

    pub fn cursor(&self) -> usize {
        self.cursor
    }

    /// Whether the card is asking for text: hemera's address, or a word.
    pub fn typing(&self) -> bool {
        matches!(self.page, Page::Hemera | Page::NewWord)
    }

    pub fn typed(&self) -> &str {
        &self.typed
    }

    /// The models a model's page offers, the one in use among them: hemera's, with the one in use
    /// first if hemera has not said it (not answered yet, or not offering it any more).
    fn choices(&self, model: Model) -> Vec<String> {
        let now = self.model(model).to_string();
        let mut choices = match model {
            Model::Reading => {
                let mut c = vec![String::new()];
                c.extend(self.offered.readers.iter().cloned());
                c
            }
            _ => self.offered.models.clone(),
        };
        if !choices.contains(&now) {
            choices.insert(0, now);
        }
        choices
    }

    fn model(&self, model: Model) -> &str {
        match model {
            Model::Everyday => &self.view.everyday,
            Model::Larger => &self.view.larger,
            Model::Cleanup => &self.view.cleanup,
            Model::Reading => &self.view.reading,
        }
    }

    fn language(&self, language: Language) -> &str {
        match language {
            Language::First => &self.view.first_language,
            Language::Second => &self.view.second_language,
        }
    }

    /// A model's name as a row shows it: hemera's choice for none.
    fn model_name(name: &str) -> String {
        if name.is_empty() {
            "hemera's choice".into()
        } else {
            name.to_string()
        }
    }

    /// Dictation's languages, in words.
    fn spoken(languages: &[String]) -> String {
        match languages.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
            ["en", "id"] => "English and Indonesian, mixed".into(),
            [] => "Any (Whisper decides)".into(),
            [one] => language_name(one),
            several => several.iter().map(|c| language_name(c)).collect::<Vec<_>>().join(", "),
        }
    }

    fn words_value(&self) -> String {
        match self.view.words.len() {
            0 => "None".into(),
            1 => "1 word".into(),
            n => format!("{n} words"),
        }
    }

    /// The rows of the page on show.
    pub fn rows(&self) -> Vec<AiRow> {
        match self.page {
            Page::List => LIST
                .iter()
                .map(|item| match *item {
                    Item::Heading(name) => AiRow { label: name.into(), value: None, heading: true },
                    Item::Hemera => AiRow::with("hemera's address", self.view.hemera.clone()),
                    Item::Model(m) => AiRow::with(m.label(), Self::model_name(self.model(m))),
                    Item::Languages => AiRow::with("Languages spoken", Self::spoken(&self.view.languages)),
                    Item::Words => AiRow::with("Your own words", self.words_value()),
                    Item::ReadByItself => AiRow::with("Read text in pictures by itself", if self.view.read_by_itself { "On" } else { "Off" }),
                    Item::Language(l) => AiRow::with(l.label(), language_name(self.language(l))),
                })
                .collect(),
            Page::Model(m) => {
                let now = self.model(m);
                self.choices(m)
                    .iter()
                    .map(|name| {
                        let label = Self::model_name(name);
                        if name == now {
                            AiRow::with(label, "In use")
                        } else {
                            AiRow::plain(label)
                        }
                    })
                    .collect()
            }
            Page::Language(l) => {
                let now = self.language(l);
                LANGUAGES.iter().map(|(code, name)| if *code == now { AiRow::with(*name, "In use") } else { AiRow::plain(*name) }).collect()
            }
            Page::Words => {
                let mut rows: Vec<AiRow> = self
                    .view
                    .words
                    .iter()
                    .enumerate()
                    .map(|(i, w)| if self.armed == Some(i) { AiRow::with(w.clone(), "A again to forget") } else { AiRow::plain(w.clone()) })
                    .collect();
                rows.push(AiRow::plain(ADD_WORD));
                rows
            }
            // What is being typed, with a bar where the next letter goes.
            Page::Hemera | Page::NewWord => vec![AiRow::plain(format!("{}\u{2502}", self.typed))],
        }
    }

    pub fn title(&self) -> String {
        match self.page {
            Page::List => "AI".into(),
            Page::Model(m) => m.label().into(),
            Page::Language(l) => l.label().into(),
            Page::Words => "Your own words".into(),
            Page::Hemera => "hemera's address".into(),
            Page::NewWord => ADD_WORD.into(),
        }
    }

    pub fn footer(&self) -> &'static str {
        match self.page {
            Page::List => "[A] Change  [B] Back",
            Page::Model(_) | Page::Language(_) => "[A] Choose  [B] Back",
            Page::Words => "[A] Select  [B] Back",
            Page::Hemera | Page::NewWord => "[Enter] Keep  [B] Back",
        }
    }

    /// The paragraph under the rows: what the row the cursor is on does, or where the card is.
    pub fn detail(&self) -> String {
        let detail = match self.page {
            Page::List => match LIST.get(self.cursor) {
                Some(Item::Hemera) => "Where hemera answers: the server every model and voice is on. Dictation, SpatiPal and any app that \
                                       asks a model go there, on every device. Choose to change it."
                    .to_string(),
                Some(Item::Model(m)) => match &self.offered.trouble {
                    Some(why) => format!("{} {why}", m.does()),
                    None => format!("{} Choose from hemera's models.", m.does()),
                },
                Some(Item::Languages) => "The languages you speak, for dictation and SpatiPal's ears. With one, Whisper is told it; with \
                                          several, it hears which, and a sentence that mixes them stays mixed. Choose to change."
                    .to_string(),
                Some(Item::Words) => {
                    let words = if self.view.words.is_empty() { "none yet".to_string() } else { self.view.words.join(", ") };
                    format!("Names and words of your own, written as you spell them when dictation hears them nearly: {words}. Choose to add one or forget one.")
                }
                Some(Item::ReadByItself) => {
                    let what = if self.view.read_by_itself {
                        "On: a picture's words are read as soon as it is shown, ready to be chosen."
                    } else {
                        "Off: a picture's words are read only when you ask."
                    };
                    format!("{what} {TEXT_NOTE}")
                }
                Some(Item::Language(_)) => format!("{TEXT_NOTE} Choose to change."),
                Some(Item::Heading(_)) | None => String::new(),
            },
            Page::Model(m) => {
                let state = if let Some(why) = &self.offered.trouble {
                    why.clone()
                } else if self.offered.asking {
                    "Asking hemera for its models\u{2026}".to_string()
                } else {
                    "Chosen here, it is used from the next request on, on every device.".to_string()
                };
                // Which one hemera's choice is, where hemera has said.
                let what = match (m, &self.offered.reader_default) {
                    (Model::Reading, Some(name)) => format!("{}, {name} now.", m.does().trim_end_matches('.')),
                    _ => m.does().to_string(),
                };
                format!("{what} {state}")
            }
            Page::Language(_) => TEXT_NOTE.to_string(),
            Page::Words => match self.view.words.get(self.cursor) {
                Some(w) if self.armed == Some(self.cursor) => format!("Press A again to forget {w}."),
                Some(w) => format!("{w}. Press A twice to forget it."),
                None => "Add a name or a word of your own, so that dictation writes it as you spell it.".to_string(),
            },
            Page::Hemera => "hemera's address, as http://name:port. Type it and press Enter; B leaves it as it was.".to_string(),
            Page::NewWord => "Type the word as it is to be written, and press Enter.".to_string(),
        };
        match &self.note {
            Some(note) => format!("{note}\n\n{detail}"),
            None => detail,
        }
    }

    fn is_heading(&self, index: usize) -> bool {
        self.page == Page::List && matches!(LIST.get(index), Some(Item::Heading(_)))
    }

    /// The cursor is never on a heading: moved on from one, the way it was going.
    fn off_heading(&mut self, forward: bool) {
        let n = self.rows().len();
        while self.is_heading(self.cursor) {
            if forward && self.cursor + 1 < n {
                self.cursor += 1;
            } else if !forward && self.cursor > 0 {
                self.cursor -= 1;
            } else {
                return;
            }
        }
    }

    /// Up and down, stopping at the ends as every list of the room's does.
    pub fn step(&mut self, direction: Direction) -> bool {
        let before = self.cursor;
        let n = self.rows().len();
        match direction {
            Direction::Up if self.cursor > 0 => {
                self.cursor -= 1;
                self.off_heading(false);
                // A heading at the very top: back where it was.
                if self.is_heading(self.cursor) {
                    self.cursor = before;
                }
            }
            Direction::Down if self.cursor + 1 < n => {
                self.cursor += 1;
                self.off_heading(true);
            }
            _ => {}
        }
        if self.cursor != before {
            self.armed = None;
        }
        self.cursor != before
    }

    /// The pointer is over a row: the cursor goes there. Not onto a heading.
    pub fn select(&mut self, index: usize) -> bool {
        if index >= self.rows().len() || index == self.cursor || self.is_heading(index) {
            return false;
        }
        self.cursor = index;
        self.armed = None;
        true
    }

    /// Back to the list, its cursor where it was.
    fn to_list(&mut self) {
        self.page = Page::List;
        self.cursor = self.list_cursor;
        self.armed = None;
    }

    /// A page under the list, the cursor at `cursor`.
    fn to_page(&mut self, page: Page, cursor: usize) {
        if self.page == Page::List {
            self.list_cursor = self.cursor;
        }
        self.page = page;
        self.cursor = cursor;
        self.armed = None;
    }

    /// A or a press: what the row the cursor is on does.
    pub fn activate(&mut self) -> Option<AiAction> {
        match self.page {
            Page::List => match LIST.get(self.cursor).copied()? {
                Item::Heading(_) => None,
                Item::Hemera => {
                    self.typed = self.view.hemera.clone();
                    self.to_page(Page::Hemera, 0);
                    None
                }
                Item::Model(m) => {
                    let now = self.model(m).to_string();
                    let at = self.choices(m).iter().position(|c| *c == now).unwrap_or(0);
                    self.to_page(Page::Model(m), at);
                    None
                }
                Item::Languages => {
                    let now: Vec<&str> = self.view.languages.iter().map(String::as_str).collect();
                    let at = SPOKEN.iter().position(|p| *p == now.as_slice()).map(|i| (i + 1) % SPOKEN.len()).unwrap_or(0);
                    Some(AiAction::Languages(SPOKEN[at].iter().map(|l| l.to_string()).collect()))
                }
                Item::Words => {
                    self.to_page(Page::Words, 0);
                    None
                }
                Item::ReadByItself => Some(AiAction::ReadByItself(!self.view.read_by_itself)),
                Item::Language(l) => {
                    let now = self.language(l).to_string();
                    let at = LANGUAGES.iter().position(|(c, _)| *c == now).unwrap_or(0);
                    self.to_page(Page::Language(l), at);
                    None
                }
            },
            Page::Model(m) => {
                let chosen = self.choices(m).get(self.cursor).cloned()?;
                let changed = chosen != self.model(m);
                self.to_list();
                changed.then_some(AiAction::Model(m, chosen))
            }
            Page::Language(l) => {
                let chosen = LANGUAGES.get(self.cursor)?.0.to_string();
                let (first, second) = (self.view.first_language.clone(), self.view.second_language.clone());
                let pair = match l {
                    Language::First if chosen == second => (chosen, first),
                    Language::First => (chosen, second),
                    Language::Second if chosen == first => (second, chosen),
                    Language::Second => (first, chosen),
                };
                self.to_list();
                (pair != (self.view.first_language.clone(), self.view.second_language.clone())).then_some(AiAction::TextLanguages(pair.0, pair.1))
            }
            Page::Words => match self.view.words.get(self.cursor).cloned() {
                Some(word) if self.armed == Some(self.cursor) => {
                    self.armed = None;
                    Some(AiAction::ForgetWord(word))
                }
                Some(_) => {
                    self.armed = Some(self.cursor);
                    None
                }
                None => {
                    self.words_cursor = self.cursor;
                    self.typed.clear();
                    self.page = Page::NewWord;
                    self.cursor = 0;
                    None
                }
            },
            Page::Hemera | Page::NewWord => self.submit(),
        }
    }

    /// B: up a page. `false` at the list, which leaves the card.
    pub fn back(&mut self) -> bool {
        match self.page {
            Page::List => false,
            Page::NewWord => {
                self.page = Page::Words;
                self.cursor = self.words_cursor;
                self.typed.clear();
                true
            }
            _ => {
                self.typed.clear();
                self.to_list();
                true
            }
        }
    }

    pub fn type_text(&mut self, text: &str) {
        if self.typing() {
            // One line: what a keyboard's Enter or a dictated line's end would put in is not kept.
            self.typed.extend(text.chars().filter(|c| !c.is_control()));
        }
    }

    pub fn backspace(&mut self) {
        if self.typing() {
            self.typed.pop();
        }
    }

    /// Enter: what was typed is kept, if there is anything.
    pub fn submit(&mut self) -> Option<AiAction> {
        let text = self.typed.trim().to_string();
        if text.is_empty() {
            return None;
        }
        match self.page {
            Page::Hemera => {
                // A name and a port is enough: hemera speaks plain HTTP.
                let address = if text.contains("://") { text } else { format!("http://{text}") };
                let address = address.trim_end_matches('/').to_string();
                self.typed.clear();
                self.to_list();
                (address != self.view.hemera).then_some(AiAction::Hemera(address))
            }
            Page::NewWord => {
                self.typed.clear();
                self.page = Page::Words;
                // On the word, where it is or where it is going to be: at the end.
                let had = self.view.words.iter().position(|w| w.eq_ignore_ascii_case(&text));
                self.cursor = had.unwrap_or(self.view.words.len());
                had.is_none().then_some(AiAction::AddWord(text))
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn card() -> Ai {
        let mut ai = Ai::default();
        ai.set_view(AiView {
            hemera: "http://deepmagpie:12434".into(),
            everyday: "neohorse-1:4b".into(),
            larger: "Qwen3.8:27b-q3".into(),
            cleanup: "neohorse-1:4b".into(),
            reading: String::new(),
            languages: vec!["en".into(), "id".into()],
            words: vec!["Spatiand".into(), "hemera".into()],
            read_by_itself: true,
            first_language: "en".into(),
            second_language: "id".into(),
        });
        ai.set_offered(Offered {
            models: vec!["neohorse-1:4b".into(), "Qwen3.8:27b-q3".into(), "ornith-1.5:9b".into()],
            readers: vec!["pp-ocrv6".into()],
            reader_default: Some("pp-ocrv6".into()),
            ..Default::default()
        });
        ai.open();
        ai
    }

    /// The cursor on the list's row labelled `label`.
    fn to_row(ai: &mut Ai, label: &str) {
        let at = ai.rows().iter().position(|r| r.label == label).unwrap_or_else(|| panic!("no row {label}"));
        while ai.cursor() > at && ai.step(Direction::Up) {}
        while ai.cursor() < at && ai.step(Direction::Down) {}
        assert_eq!(ai.cursor(), at, "{label}");
    }

    #[test]
    fn the_list_says_what_each_is_now_and_its_headings_are_never_chosen() {
        let mut ai = card();
        let rows = ai.rows();
        let shown: Vec<(&str, Option<&str>)> = rows.iter().filter(|r| !r.heading).map(|r| (r.label.as_str(), r.value.as_deref())).collect();
        assert_eq!(
            shown,
            vec![
                ("hemera's address", Some("http://deepmagpie:12434")),
                ("Everyday model", Some("neohorse-1:4b")),
                ("Larger model", Some("Qwen3.8:27b-q3")),
                ("Terminal clean-up model", Some("neohorse-1:4b")),
                ("Text reading model", Some("hemera's choice")),
                ("Languages spoken", Some("English and Indonesian, mixed")),
                ("Your own words", Some("2 words")),
                ("Read text in pictures by itself", Some("On")),
                ("First language", Some("English")),
                ("Second language", Some("Indonesian")),
            ]
        );
        let headings: Vec<&str> = rows.iter().filter(|r| r.heading).map(|r| r.label.as_str()).collect();
        assert_eq!(headings, ["Models", "Dictation", "Text in pictures"]);
        // Walked from top to bottom, the cursor steps over every heading and stops at the ends.
        assert_eq!(ai.cursor(), 0);
        assert!(!ai.step(Direction::Up));
        let mut seen = vec![ai.cursor()];
        while ai.step(Direction::Down) {
            seen.push(ai.cursor());
        }
        assert!(seen.iter().all(|i| !rows[*i].heading));
        assert_eq!(seen.len(), 10);
        // Pointed at, a heading is not taken.
        let heading = rows.iter().position(|r| r.heading).unwrap();
        assert!(!ai.select(heading));
    }

    #[test]
    fn a_model_is_chosen_from_hemera_s_and_the_card_goes_back_to_its_list() {
        let mut ai = card();
        to_row(&mut ai, "Larger model");
        let row = ai.cursor();
        assert_eq!(ai.activate(), None);
        assert_eq!(ai.page(), Page::Model(Model::Larger));
        // On the one in use, marked so.
        assert_eq!(ai.rows()[ai.cursor()], AiRow::with("Qwen3.8:27b-q3", "In use"));
        ai.step(Direction::Down);
        assert_eq!(ai.activate(), Some(AiAction::Model(Model::Larger, "ornith-1.5:9b".into())));
        assert_eq!((ai.page(), ai.cursor()), (Page::List, row));
        // The one in use chosen again changes nothing.
        ai.activate();
        assert_eq!(ai.activate(), None);
        assert_eq!(ai.page(), Page::List);
    }

    #[test]
    fn text_reading_offers_hemera_s_choice_first_and_names_it() {
        let mut ai = card();
        to_row(&mut ai, "Text reading model");
        ai.activate();
        let rows: Vec<String> = ai.rows().iter().map(|r| r.label.clone()).collect();
        assert_eq!(rows, ["hemera's choice", "pp-ocrv6"]);
        assert!(ai.detail().contains("when none is named, pp-ocrv6 now."));
        assert_eq!(ai.detail().matches("hemera's choice").count(), 1);
        ai.step(Direction::Down);
        assert_eq!(ai.activate(), Some(AiAction::Model(Model::Reading, "pp-ocrv6".into())));
        // And back to hemera's choice: no model named.
        let mut view = ai.view().clone();
        view.reading = "pp-ocrv6".into();
        ai.set_view(view);
        ai.activate();
        ai.step(Direction::Up);
        assert_eq!(ai.activate(), Some(AiAction::Model(Model::Reading, String::new())));
    }

    #[test]
    fn a_model_hemera_has_not_named_is_still_shown_while_it_is_asked() {
        let mut ai = card();
        ai.set_offered(Offered { asking: true, ..Default::default() });
        to_row(&mut ai, "Everyday model");
        ai.activate();
        assert_eq!(ai.rows(), vec![AiRow::with("neohorse-1:4b", "In use")]);
        assert!(ai.detail().ends_with("Asking hemera for its models\u{2026}"));
        ai.set_offered(Offered { trouble: Some("hemera did not answer: refused.".into()), ..Default::default() });
        assert!(ai.detail().ends_with("hemera did not answer: refused."));
        // hemera answers while the page is open: the cursor stays on the model it was on.
        ai.set_offered(Offered { models: vec!["gemma4:e4b".into(), "neohorse-1:4b".into()], ..Default::default() });
        assert_eq!(ai.cursor(), 1);
        assert_eq!(ai.rows()[1], AiRow::with("neohorse-1:4b", "In use"));
    }

    #[test]
    fn languages_spoken_step_as_spatipal_s_did() {
        let mut ai = card();
        to_row(&mut ai, "Languages spoken");
        let mut view = ai.view().clone();
        let mut seen = Vec::new();
        for _ in 0..4 {
            let Some(AiAction::Languages(next)) = ai.activate() else { panic!("languages step") };
            view.languages = next.clone();
            ai.set_view(view.clone());
            seen.push(ai.rows()[ai.cursor()].value.clone().unwrap());
        }
        assert_eq!(seen, ["English", "Indonesian", "Any (Whisper decides)", "English and Indonesian, mixed"]);
    }

    #[test]
    fn a_word_is_forgotten_by_pressing_twice_and_another_typed_in() {
        let mut ai = card();
        to_row(&mut ai, "Your own words");
        ai.activate();
        assert_eq!(ai.page(), Page::Words);
        let labels: Vec<String> = ai.rows().iter().map(|r| r.label.clone()).collect();
        assert_eq!(labels, ["Spatiand", "hemera", ADD_WORD]);
        // Once arms it; moving away lets it go; twice forgets it.
        assert_eq!(ai.activate(), None);
        assert_eq!(ai.rows()[0].value.as_deref(), Some("A again to forget"));
        ai.step(Direction::Down);
        ai.step(Direction::Up);
        assert_eq!(ai.rows()[0].value, None);
        ai.activate();
        assert_eq!(ai.activate(), Some(AiAction::ForgetWord("Spatiand".into())));
        // Adding: typed, Enter; a word already there is not added twice.
        to_row(&mut ai, ADD_WORD);
        ai.activate();
        assert!(ai.typing());
        ai.type_text("Pixel");
        assert_eq!(ai.rows()[0].label, "Pixel\u{2502}");
        assert_eq!(ai.submit(), Some(AiAction::AddWord("Pixel".into())));
        assert_eq!((ai.page(), ai.cursor()), (Page::Words, 2));
        to_row(&mut ai, ADD_WORD);
        ai.activate();
        ai.type_text("HEMERA");
        assert_eq!(ai.submit(), None);
        assert_eq!(ai.cursor(), 1);
        // B from typing goes back to the words, from the words to the list, and then out.
        to_row(&mut ai, ADD_WORD);
        ai.activate();
        assert!(ai.back());
        assert_eq!(ai.page(), Page::Words);
        assert!(ai.back());
        assert_eq!(ai.page(), Page::List);
        assert!(!ai.back());
    }

    #[test]
    fn hemera_s_address_is_typed_over_what_it_was_and_given_its_http() {
        let mut ai = card();
        ai.activate();
        assert_eq!(ai.page(), Page::Hemera);
        assert_eq!(ai.typed(), "http://deepmagpie:12434");
        for _ in 0.."http://deepmagpie:12434".len() {
            ai.backspace();
        }
        ai.type_text("hemera.local:12434\n");
        assert_eq!(ai.typed(), "hemera.local:12434");
        assert_eq!(ai.submit(), Some(AiAction::Hemera("http://hemera.local:12434".into())));
        assert_eq!(ai.page(), Page::List);
        // B leaves it as it was.
        ai.activate();
        ai.type_text("x");
        assert!(ai.back());
        assert_eq!(ai.typed(), "");
        assert_eq!(ai.page(), Page::List);
        // Nothing typed is not an address.
        ai.activate();
        while !ai.typed().is_empty() {
            ai.backspace();
        }
        assert_eq!(ai.submit(), None);
        assert!(ai.typing());
    }

    #[test]
    fn the_two_languages_of_text_in_pictures_are_never_the_same() {
        let mut ai = card();
        to_row(&mut ai, "First language");
        ai.activate();
        assert_eq!(ai.rows()[ai.cursor()], AiRow::with("English", "In use"));
        // Indonesian, the second, chosen for the first: the two swap.
        ai.step(Direction::Down);
        assert_eq!(ai.activate(), Some(AiAction::TextLanguages("id".into(), "en".into())));
        to_row(&mut ai, "Second language");
        ai.activate();
        let japanese = LANGUAGES.iter().position(|(c, _)| *c == "ja").unwrap();
        assert!(ai.select(japanese));
        assert_eq!(ai.activate(), Some(AiAction::TextLanguages("en".into(), "ja".into())));
        // What they do is said under each of the three.
        to_row(&mut ai, "Read text in pictures by itself");
        assert!(ai.detail().contains("Nothing reads text in pictures yet"));
        assert_eq!(ai.activate(), Some(AiAction::ReadByItself(false)));
    }

    #[test]
    fn opened_again_the_card_is_at_its_list() {
        let mut ai = card();
        to_row(&mut ai, "Second language");
        let row = ai.cursor();
        ai.activate();
        ai.open();
        assert_eq!((ai.page(), ai.cursor()), (Page::List, row));
    }

    #[test]
    fn a_change_not_kept_is_said_until_one_is() {
        let mut ai = card();
        ai.set_note(Some("The settings could not be kept: the disk is full.".into()));
        assert!(ai.detail().starts_with("The settings could not be kept"));
        ai.set_note(None);
        assert!(ai.detail().starts_with("Where hemera answers"));
    }
}
