//! Which applications open a file on Android: only the APK's own Java may ask the
//! PackageManager, and SpatiShelf is a native program inside it, so the room is asked over its
//! protocol (`spatiand_xr_v1.ask_system`, answered by `android/.../Shell.java`), and the app
//! chosen opens in the room as any of the phone's apps does there, handed the file as a
//! `content:` address of Spatiand's own with leave to read it.

use std::path::Path;
use std::time::Duration;

use serde_json::json;

use super::App;

/// The MIME type a file's ending says, as Android's `MimeTypeMap` has the common ones.
pub fn type_of(path: &Path, folder: bool) -> String {
    if folder {
        return "inode/directory".into();
    }
    let ending = path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    let kind = match ending.as_str() {
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "heic" | "heif" => "image/heif",
        "avif" => "image/avif",
        "bmp" => "image/bmp",
        "svg" => "image/svg+xml",
        "dng" => "image/x-adobe-dng",
        "mp4" | "m4v" => "video/mp4",
        "mkv" => "video/x-matroska",
        "webm" => "video/webm",
        "mov" => "video/quicktime",
        "avi" => "video/x-msvideo",
        "ts" | "m2ts" => "video/mp2t",
        "3gp" => "video/3gpp",
        "mp3" => "audio/mpeg",
        "m4a" | "aac" => "audio/mp4",
        "flac" => "audio/flac",
        "ogg" | "oga" => "audio/ogg",
        "opus" => "audio/opus",
        "wav" => "audio/x-wav",
        "pdf" => "application/pdf",
        "txt" | "md" | "log" | "csv" => "text/plain",
        "html" | "htm" => "text/html",
        "json" => "application/json",
        "zip" => "application/zip",
        "apk" => "application/vnd.android.package-archive",
        "epub" => "application/epub+zip",
        "doc" => "application/msword",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "pptx" => "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        "odt" => "application/vnd.oasis.opendocument.text",
        _ => "application/octet-stream",
    };
    kind.into()
}

/// The PNG files the room drew the apps' icons into, by app.
fn icons() -> &'static std::sync::Mutex<std::collections::HashMap<String, String>> {
    static ICONS: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<String, String>>> = std::sync::OnceLock::new();
    ICONS.get_or_init(Default::default)
}

fn ask(room: &mut spatiand_app::App, question: serde_json::Value) -> Result<serde_json::Value, String> {
    let answer = room.ask_system(&question.to_string(), Duration::from_secs(5))?;
    let answer: serde_json::Value = serde_json::from_str(&answer).map_err(|e| e.to_string())?;
    match answer.get("error").and_then(|e| e.as_str()) {
        Some(e) => Err(e.to_string()),
        None => Ok(answer),
    }
}

fn apps_of(answer: &serde_json::Value) -> Vec<App> {
    let mut known = icons().lock().unwrap();
    answer
        .get("apps")
        .and_then(|a| a.as_array())
        .into_iter()
        .flatten()
        .filter_map(|a| {
            let id = a.get("id")?.as_str()?.to_string();
            if let Some(icon) = a.get("icon").and_then(|i| i.as_str()) {
                known.insert(id.clone(), icon.to_string());
            }
            Some(App { name: a.get("name").and_then(|n| n.as_str()).unwrap_or(&id).to_string(), system_default: a.get("default").and_then(|d| d.as_bool()).unwrap_or(false), takes_urls: false, id })
        })
        .collect()
}

pub fn apps_for(path: &Path, kind: &str, room: &mut spatiand_app::App) -> Vec<App> {
    match ask(room, json!({"do": "openers", "path": path.to_string_lossy(), "type": kind})) {
        Ok(answer) => apps_of(&answer),
        Err(e) => {
            log::warn!("the phone's apps for {kind}: {e}");
            Vec::new()
        }
    }
}

pub fn all_apps(room: &mut spatiand_app::App) -> Vec<App> {
    ask(room, json!({"do": "all_openers", "type": "*/*"})).map(|a| apps_of(&a)).unwrap_or_default()
}

pub fn launch(app: &App, files: &[String], room: &mut spatiand_app::App) -> Result<(), String> {
    for file in files {
        let kind = type_of(Path::new(file), Path::new(file).is_dir());
        ask(room, json!({"do": "open_with", "id": app.id, "path": file, "type": kind}))?;
    }
    Ok(())
}

pub fn icon(app: &App, size: u32) -> Option<(u32, u32, Vec<u8>)> {
    let file = icons().lock().unwrap().get(&app.id).cloned()?;
    super::picture_file(Path::new(&file), size)
}
