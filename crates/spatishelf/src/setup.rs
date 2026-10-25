//! Adding a remote: which kind, then what that kind needs, then whatever rclone asks after
//! that -- one question at a time, as `rclone config` asks them, signing in through a browser
//! where the kind wants that.

use serde_json::Value;
use spatiand_app::ui::{look, theme, Button, Field, Input, List, Painter, Rect, Row, Typed};
use spatiand_app::{Gl, Key};

/// One of rclone's kinds of remote.
#[derive(Debug, Clone)]
pub struct Provider {
    pub name: String,
    pub description: String,
    pub options: Vec<Question>,
}

/// Something a kind of remote needs to be told.
#[derive(Debug, Clone, Default)]
pub struct Question {
    pub name: String,
    pub help: String,
    pub default: String,
    pub examples: Vec<(String, String)>,
    pub required: bool,
    pub password: bool,
    pub exclusive: bool,
}

impl Question {
    pub fn from_json(o: &Value) -> Question {
        let text = |k: &str| o.get(k).map(|v| match v {
            Value::String(s) => s.clone(),
            Value::Null => String::new(),
            other => other.to_string(),
        }).unwrap_or_default();
        Question {
            name: text("Name"),
            help: text("Help"),
            default: text("DefaultStr").chars().take(200).collect::<String>().replace("\"", ""),
            examples: o.get("Examples").and_then(|e| e.as_array()).map(|a| a.iter().map(|x| (x.get("Value").map(|v| v.as_str().map(str::to_string).unwrap_or_else(|| v.to_string())).unwrap_or_default(), x.get("Help").and_then(|h| h.as_str()).unwrap_or("").to_string())).collect()).unwrap_or_default(),
            required: o.get("Required").and_then(|r| r.as_bool()).unwrap_or(false),
            password: o.get("IsPassword").and_then(|r| r.as_bool()).unwrap_or(false),
            exclusive: o.get("Exclusive").and_then(|r| r.as_bool()).unwrap_or(false),
        }
    }

    /// The help's first line, which is what it is; the rest is detail.
    pub fn short_help(&self) -> String {
        self.help.lines().next().unwrap_or("").trim().to_string()
    }
}

/// The kinds most people want, first; the rest by name.
const FIRST: &[&str] = &["drive", "dropbox", "onedrive", "iclouddrive", "protondrive", "s3", "sftp", "smb", "webdav", "ftp", "b2", "box", "pcloud", "mega", "google photos", "http"];

pub fn providers(json: &Value) -> Vec<Provider> {
    let mut all: Vec<Provider> = json
        .get("providers")
        .and_then(|p| p.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|p| {
                    let name = p.get("Name")?.as_str()?.to_string();
                    // Kinds that wrap other remotes, or exist only for testing, are not a
                    // place to keep files.
                    if ["memory", "cache", "tardigrade", "uptobox"].contains(&name.as_str()) {
                        return None;
                    }
                    let options = p.get("Options").and_then(|o| o.as_array()).map(|o| o.iter().filter(|q| !q.get("Advanced").and_then(|a| a.as_bool()).unwrap_or(false) && !q.get("Hide").and_then(|h| h.as_i64()).is_some_and(|h| h != 0)).map(Question::from_json).collect()).unwrap_or_default();
                    Some(Provider { name, description: p.get("Description").and_then(|d| d.as_str()).unwrap_or("").to_string(), options })
                })
                .collect()
        })
        .unwrap_or_default();
    all.sort_by_key(|p| (FIRST.iter().position(|f| *f == p.name).unwrap_or(FIRST.len()), p.description.to_lowercase()));
    all
}

pub enum Step {
    /// Waiting for the list of kinds.
    Loading,
    Choose,
    Form,
    /// Something rclone asked, after the form.
    Asked { state: String, question: Question },
    Waiting(String),
}

/// What the screen wants done.
pub enum Want {
    Nothing,
    Cancel,
    /// Make the remote: its name, kind and the answers.
    Create { name: String, kind: String, answers: Vec<(String, String)> },
    /// Answer rclone's question.
    Answer { name: String, kind: String, state: String, answer: String },
}

pub struct Setup {
    pub step: Step,
    providers: Vec<Provider>,
    filter: Field,
    list: List,
    shown: Vec<usize>,
    chosen: Option<usize>,
    name: Field,
    fields: Vec<Field>,
    focus: usize,
    answer: Field,
    choices: List,
    go: Button,
    back: Button,
    pub trouble: Option<String>,
    scroll: f32,
}

impl Setup {
    pub fn new() -> Setup {
        let mut filter = Field::default();
        filter.focused = true;
        filter.hint = "Find a kind of storage: Google Drive, SFTP, S3\u{2026}".into();
        let mut list = List::default();
        list.focused = true;
        Setup { step: Step::Loading, providers: Vec::new(), filter, list, shown: Vec::new(), chosen: None, name: Field::default(), fields: Vec::new(), focus: 0, answer: Field::default(), choices: List::default(), go: Button::default(), back: Button::default(), trouble: None, scroll: 0.0 }
    }

    pub fn loaded(&mut self, providers: Vec<Provider>) {
        self.providers = providers;
        self.step = Step::Choose;
        self.refilter();
    }

    fn refilter(&mut self) {
        let words = self.filter.text.to_lowercase();
        self.shown = self.providers.iter().enumerate().filter(|(_, p)| words.is_empty() || p.description.to_lowercase().contains(&words) || p.name.contains(&words)).map(|(i, _)| i).collect();
        self.list.cursor = 0;
    }

    fn choose(&mut self, index: usize) {
        let provider = &self.providers[index];
        self.chosen = Some(index);
        self.name = Field::default();
        self.name.hint = "A name for it".into();
        // The kind's own first word, as a name a path can carry: "SSH/SFTP" is "SSH".
        let first = provider.description.split([' ', '(', ',', '/']).next().unwrap_or(&provider.name);
        let name: String = first.chars().filter(|c| c.is_alphanumeric() || *c == '-' || *c == '_').collect();
        self.name.set(if name.is_empty() { provider.name.clone() } else { name });
        self.fields = provider
            .options
            .iter()
            .map(|q| {
                let mut f = Field::default();
                f.secret = q.password;
                f.hint = if !q.examples.is_empty() {
                    format!("e.g. {}", q.examples.iter().take(3).map(|(v, _)| v.as_str()).collect::<Vec<_>>().join(", "))
                } else if !q.default.is_empty() {
                    format!("{} if left empty", q.default)
                } else if q.required {
                    "Needed".into()
                } else {
                    "May be left empty".into()
                };
                f
            })
            .collect();
        self.focus = 0;
        self.name.focused = true;
        self.step = Step::Form;
        self.scroll = 0.0;
    }

    pub fn asked(&mut self, state: String, question: Question) {
        self.answer = Field::default();
        self.answer.focused = true;
        self.answer.secret = question.password;
        self.answer.set(question.default.clone());
        self.choices = List::default();
        self.choices.focused = true;
        if let Some(at) = question.examples.iter().position(|(v, _)| *v == question.default) {
            self.choices.cursor = at;
        }
        self.step = Step::Asked { state, question };
    }

    fn kind(&self) -> String {
        self.chosen.map(|i| self.providers[i].name.clone()).unwrap_or_default()
    }

    pub fn remote_name(&self) -> String {
        self.name.text.trim().to_string()
    }

    fn set_focus(&mut self, to: usize) {
        self.focus = to.min(self.fields.len());
        self.name.focused = self.focus == 0;
        for (i, f) in self.fields.iter_mut().enumerate() {
            f.focused = self.focus == i + 1;
        }
    }

    fn submit_form(&mut self) -> Want {
        let name = self.remote_name();
        if name.is_empty() || name.contains(':') || name.contains('/') {
            self.trouble = Some("The name may not be empty or have : or / in it.".into());
            return Want::Nothing;
        }
        let provider = &self.providers[self.chosen.unwrap()];
        let mut answers = Vec::new();
        for (q, f) in provider.options.iter().zip(&self.fields) {
            let text = f.text.trim();
            if text.is_empty() {
                if q.required && q.default.is_empty() {
                    self.trouble = Some(format!("{} is needed.", nice(&q.name)));
                    return Want::Nothing;
                }
                continue;
            }
            answers.push((q.name.clone(), text.to_string()));
        }
        self.trouble = None;
        self.step = Step::Waiting("Setting it up\u{2026}".into());
        Want::Create { name, kind: provider.name.clone(), answers }
    }

    pub fn input(&mut self, input: &Input) -> Want {
        if self.back.input(input) || matches!(input, Input::Key { key: Key::Escape, .. }) {
            return match self.step {
                Step::Form => {
                    self.step = Step::Choose;
                    Want::Nothing
                }
                _ => Want::Cancel,
            };
        }
        match &mut self.step {
            Step::Loading | Step::Waiting(_) => Want::Nothing,
            Step::Choose => {
                if let Some(Typed::Changed) = self.filter.input(input) {
                    self.refilter();
                    return Want::Nothing;
                }
                if let Some(row) = self.list.input(input) {
                    if let Some(&index) = self.shown.get(row) {
                        self.choose(index);
                    }
                }
                Want::Nothing
            }
            Step::Form => {
                if self.go.input(input) {
                    return self.submit_form();
                }
                match input {
                    Input::Key { key: Key::Tab, modifiers, .. } | Input::Key { key: Key::Down, modifiers, .. } if !modifiers.control => {
                        let back = matches!(input, Input::Key { key: Key::Tab, modifiers, .. } if modifiers.shift);
                        let to = if back { self.focus.saturating_sub(1) } else { self.focus + 1 };
                        self.set_focus(to);
                        return Want::Nothing;
                    }
                    Input::Key { key: Key::Up, .. } => {
                        self.set_focus(self.focus.saturating_sub(1));
                        return Want::Nothing;
                    }
                    Input::Press { pressed: true } => {
                        // The field under the pointer takes the keys.
                        let clicked = std::iter::once(&self.name).chain(self.fields.iter()).position(|f| f.contains(POINTER.with(|p| p.get().0), POINTER.with(|p| p.get().1)));
                        if let Some(at) = clicked {
                            self.set_focus(at);
                        }
                    }
                    _ => {}
                }
                let field = if self.focus == 0 { &mut self.name } else { &mut self.fields[self.focus - 1] };
                if let Some(Typed::Entered) = field.input(input) {
                    if self.focus >= self.fields.len() {
                        return self.submit_form();
                    }
                    self.set_focus(self.focus + 1);
                }
                Want::Nothing
            }
            Step::Asked { state, question } => {
                let state = state.clone();
                let exclusive = question.exclusive && !question.examples.is_empty();
                let examples = question.examples.clone();
                let answer = if exclusive {
                    if let Some(row) = self.choices.input(input) {
                        examples.get(row).map(|(v, _)| v.clone())
                    } else if self.go.input(input) {
                        examples.get(self.choices.cursor).map(|(v, _)| v.clone())
                    } else {
                        None
                    }
                } else if self.go.input(input) || matches!(self.answer.input(input), Some(Typed::Entered)) {
                    Some(self.answer.text.trim().to_string())
                } else {
                    if !examples.is_empty() {
                        if let Some(row) = self.choices.input(input) {
                            if let Some((v, _)) = examples.get(row) {
                                self.answer.set(v.clone());
                            }
                        }
                    }
                    None
                };
                match answer {
                    Some(answer) => {
                        self.step = Step::Waiting(if question_is_browser(&examples, &answer) { "Sign in in the browser that has opened. This waits for you.".into() } else { "Setting it up\u{2026}".into() });
                        Want::Answer { name: self.remote_name(), kind: self.kind(), state, answer }
                    }
                    None => Want::Nothing,
                }
            }
        }
    }

    pub fn draw(&mut self, p: &mut Painter, gl: &Gl, area: Rect) {
        let mut all = area;
        let title = match &self.step {
            Step::Form | Step::Asked { .. } | Step::Waiting(_) if self.chosen.is_some() => format!("Add {}", self.providers[self.chosen.unwrap()].description),
            _ => "Add a remote".to_string(),
        };
        let mut head = all.take_top(Button::HEIGHT);
        let back_label = if matches!(self.step, Step::Form) { "\u{2039}  Kinds" } else { "\u{2039}  Cancel" };
        let w = Button::width(p, gl, back_label);
        self.back.draw(p, gl, head.take_left(w), back_label);
        head.take_left(22.0);
        p.label(gl, &title, head, theme::TITLE_EM * 0.8, theme::ink(), 0.0);
        all.take_top(12.0);
        look::rule(p, gl, all.take_top(2.0));
        all.take_top(16.0);
        let foot = all.take_bottom(theme::FOOTER_EM * 1.8);
        if let Some(trouble) = &self.trouble {
            p.label(gl, trouble, foot, theme::FOOTER_EM, look::trouble(), 0.0);
        }
        match &self.step {
            Step::Loading => look::waiting(p, gl, all, std::time::Instant::now() - std::time::Duration::from_millis(300)),
            Step::Waiting(what) => {
                look::waiting(p, gl, Rect { h: all.h * 0.6, ..all }, std::time::Instant::now() - std::time::Duration::from_millis(300));
                look::message(p, gl, Rect { y: all.y + all.h * 0.55, h: 60.0, ..all }, what, theme::ink_detail());
            }
            Step::Choose => {
                self.filter.draw(p, gl, all.take_top(Field::HEIGHT));
                all.take_top(14.0);
                let rows: Vec<Row> = self.shown.iter().map(|&i| Row::with(self.providers[i].description.clone(), self.providers[i].name.clone())).collect();
                self.list.draw(p, gl, all, &rows);
                if self.shown.is_empty() {
                    look::message(p, gl, all, "No kind of storage by that name.", theme::ink_detail());
                }
            }
            Step::Form => {
                let provider = self.providers[self.chosen.unwrap()].clone();
                let go = Rect { x: all.right() - 260.0, w: 260.0, ..all.take_bottom(Button::HEIGHT) };
                all.take_bottom(14.0);
                self.go.draw(p, gl, go, "Set it up");
                p.clip(gl, all);
                // The form scrolls to keep the field being typed in on show.
                let row_h = 104.0;
                let focus_y = self.focus as f32 * row_h;
                if focus_y - self.scroll > all.h - row_h {
                    self.scroll = focus_y - all.h + row_h;
                }
                if focus_y < self.scroll {
                    self.scroll = focus_y;
                }
                let mut y = all.y - self.scroll;
                let width = all.w.min(900.0);
                let label = |p: &mut Painter, gl: &Gl, y: f32, text: &str, help: &str| {
                    p.label(gl, &text.to_uppercase(), Rect::new(all.x, y, 300.0, 26.0), theme::FOOTER_EM * 0.82, theme::ink_hint(), 0.0);
                    p.label(gl, help, Rect::new(all.x + 220.0, y, width - 220.0, 26.0), theme::FOOTER_EM * 0.82, theme::ink_hint(), 0.0);
                };
                label(p, gl, y, "Name", "What it is called here, as in Name:folder/file");
                self.name.draw(p, gl, Rect::new(all.x, y + 30.0, width, Field::HEIGHT));
                y += row_h;
                for (q, f) in provider.options.iter().zip(self.fields.iter_mut()) {
                    label(p, gl, y, &nice(&q.name), &q.short_help());
                    f.draw(p, gl, Rect::new(all.x, y + 30.0, width, Field::HEIGHT));
                    y += row_h;
                }
                p.unclip(gl);
                if provider.options.is_empty() {
                    look::message(p, gl, Rect { y: all.y + 140.0, h: 80.0, ..all }, "Nothing more is needed here: anything else is asked next, one question at a time.", theme::ink_detail());
                }
            }
            Step::Asked { question, .. } => {
                let question = question.clone();
                let go = Rect { x: all.right() - 260.0, w: 260.0, ..all.take_bottom(Button::HEIGHT) };
                all.take_bottom(14.0);
                self.go.draw(p, gl, go, "Continue");
                let (h, _) = p.measure(gl, &question.help, theme::DETAIL_EM, all.w.min(1000.0));
                let _ = h;
                let (_, th) = p.text(gl, &question.help, all.x, all.y, theme::DETAIL_EM, all.w.min(1000.0), theme::ink_detail());
                all.take_top(th + 22.0);
                let exclusive = question.exclusive && !question.examples.is_empty();
                if !exclusive {
                    self.answer.draw(p, gl, all.take_top(Field::HEIGHT));
                    all.take_top(14.0);
                }
                if !question.examples.is_empty() {
                    let rows: Vec<Row> = question.examples.iter().map(|(v, help)| {
                        let first = help.lines().next().unwrap_or("").to_string();
                        if first.is_empty() { Row::new(v.clone()) } else { Row::with(first, v.clone()) }
                    }).collect();
                    self.choices.draw(p, gl, all, &rows);
                }
            }
        }
    }
}

fn question_is_browser(examples: &[(String, String)], answer: &str) -> bool {
    answer == "true" && examples.iter().any(|(_, h)| h.to_lowercase().contains("browser") || h.to_lowercase().contains("auth"))
}

/// rclone's option names as words: `access_key_id` is "Access key id".
pub fn nice(name: &str) -> String {
    let words = name.replace('_', " ");
    let mut chars = words.chars();
    match chars.next() {
        Some(c) => c.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

thread_local! {
    /// Where the pointer is, for the form to know which field was clicked.
    pub static POINTER: std::cell::Cell<(f32, f32)> = const { std::cell::Cell::new((0.0, 0.0)) };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_kinds_most_people_want_come_first() {
        let json = serde_json::json!({"providers": [
            {"Name": "zoho", "Description": "Zoho", "Options": []},
            {"Name": "drive", "Description": "Google Drive", "Options": [{"Name": "client_id", "Help": "OAuth Client Id\nLeave blank normally.", "Advanced": false}, {"Name": "token", "Advanced": true}]},
            {"Name": "memory", "Description": "In memory", "Options": []}
        ]});
        let all = providers(&json);
        assert_eq!(all.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(), ["drive", "zoho"]);
        assert_eq!(all[0].options.len(), 1);
        assert_eq!(all[0].options[0].short_help(), "OAuth Client Id");
        assert_eq!(nice("access_key_id"), "Access key id");
    }
}
