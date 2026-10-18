//! Settings the panel can change, written back to config.toml with its comments kept.

use serde::{Deserialize, Serialize};

use crate::ai;

#[derive(Serialize)]
pub struct Settings {
    provider: String,
    /// (name, model) of every configured provider, sorted by name.
    providers: Vec<(String, String)>,
    default_question: String,
}

#[derive(Deserialize)]
pub struct Changes {
    provider: String,
    model: String,
    default_question: String,
}

pub fn get() -> Result<Settings, String> {
    let config = ai::load_config()?;
    let mut providers: Vec<_> = config
        .providers
        .iter()
        .map(|(name, p)| (name.clone(), p.model.clone()))
        .collect();
    providers.sort();
    Ok(Settings {
        provider: config.provider,
        providers,
        default_question: config
            .ui
            .default_question
            .unwrap_or_else(|| crate::DEFAULT_QUESTION.to_owned()),
    })
}

/// Switch provider, set its model and the default question. An empty or built-in
/// default question removes the setting.
pub fn save(changes: Changes) -> Result<(), String> {
    let path = ai::config_path();
    let text = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
    let mut doc: toml_edit::DocumentMut = text.parse().map_err(|e| format!("{}: {e}", path.display()))?;

    let provider = doc
        .get_mut("providers")
        .and_then(|p| p.get_mut(&changes.provider))
        .ok_or_else(|| format!("provider '{}' not found in [providers]", changes.provider))?;
    let model = changes.model.trim();
    if model.is_empty() {
        return Err("model can't be empty".into());
    }
    provider["model"] = toml_edit::value(model);
    doc["provider"] = toml_edit::value(&changes.provider);

    let question = changes.default_question.trim();
    if question.is_empty() || question == crate::DEFAULT_QUESTION {
        if let Some(ui) = doc.get_mut("ui").and_then(|ui| ui.as_table_like_mut()) {
            ui.remove("default_question");
        }
    } else {
        doc["ui"].or_insert(toml_edit::table())["default_question"] = toml_edit::value(question);
    }
    std::fs::write(&path, doc.to_string()).map_err(|e| e.to_string())
}

/// Copy text to the clipboard: `wl-copy` on Wayland, `xclip` on X11.
pub fn copy(text: &str) -> Result<(), String> {
    use std::io::Write;
    let (program, args): (&str, &[&str]) = if crate::overlay::is_wayland() {
        ("wl-copy", &[])
    } else {
        ("xclip", &["-selection", "clipboard"])
    };
    let mut child = std::process::Command::new(program)
        .args(args)
        .stdin(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("cannot run {program}: {e}"))?;
    child
        .stdin
        .take()
        .ok_or("no stdin")?
        .write_all(text.as_bytes())
        .map_err(|e| e.to_string())?;
    // wl-copy forks to serve the clipboard; this only waits for the parent.
    let status = child.wait().map_err(|e| e.to_string())?;
    if !status.success() {
        return Err(format!("{program} failed"));
    }
    Ok(())
}
