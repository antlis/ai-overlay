//! Settings the panel can change, written back to config.toml with its comments kept.

use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::ai;

#[derive(Serialize)]
pub struct Settings {
    provider: String,
    /// (name, model) of every configured provider, sorted by name.
    providers: Vec<(String, String)>,
    default_question: String,
    invisible: bool,
    /// Running on Wayland, the only place invisibility can work.
    wayland: bool,
}

#[derive(Deserialize)]
pub struct Changes {
    provider: String,
    model: String,
    default_question: String,
    pub invisible: bool,
}

const MODEL_CACHE_TTL: Duration = Duration::from_secs(86_400); // 1 day

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
        invisible: config.ui.invisible,
        wayland: crate::overlay::is_wayland(),
    })
}

/// Models advertised by `provider` (the part visible in the picker). Pulls them from the
/// provider's `GET {base_url}/models`, caches the result per-provider on disk, and falls
/// back to the cache when the live fetch fails (network down, key missing). `force` skips
/// the cache. The empty list is meaningful: it tells the UI to show a free-text input.
pub fn models_for(provider: &str, force: bool) -> Result<Vec<String>, String> {
    let config = ai::load_config()?;
    let p = config
        .providers
        .get(provider)
        .ok_or_else(|| format!("provider '{provider}' not found"))?;
    let cache = cache_path(provider);
    if !force {
        if let Some(ids) = read_cache(&cache) {
            return Ok(ids);
        }
    }
    let ids = match fetch_models(p) {
        Ok(ids) => {
            if let Some(dir) = cache.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            let payload = serde_json::json!({
                "fetched_at": SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs(),
                "ids": &ids,
            });
            let _ = std::fs::write(&cache, payload.to_string());
            ids
        }
        Err(_) => read_cache(&cache).unwrap_or_default(),
    };
    Ok(ids)
}

fn fetch_models(p: &ai::Provider) -> Result<Vec<String>, String> {
    let url = format!("{}/models", p.base_url.trim_end_matches('/'));
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(5))
        .timeout_read(Duration::from_secs(10))
        .build();
    let mut req = agent.get(&url);
    // OpenAI-compatible endpoints may or may not require auth. Ollama doesn't.
    if let Ok(key) = p.api_key() {
        if !key.is_empty() && key != "ollama" {
            req = req.set("Authorization", &format!("Bearer {key}"));
        }
    }
    let response = req.call().map_err(|e| e.to_string())?;
    let body = response.into_string().map_err(|e| e.to_string())?;
    let v: serde_json::Value = serde_json::from_str(&body).map_err(|e| format!("{url}: {e}"))?;
    let mut ids: Vec<String> = Vec::new();
    // OpenAI-compatible: { "data": [{ "id": "..." }, ...] }
    if let Some(arr) = v.get("data").and_then(|d| d.as_array()) {
        for entry in arr {
            if let Some(id) = entry.get("id").and_then(|i| i.as_str()) {
                ids.push(id.to_owned());
            }
        }
    }
    // Ollama native shape: { "models": [{ "name": "...", ... }] }
    if let Some(arr) = v.get("models").and_then(|d| d.as_array()) {
        for entry in arr {
            if let Some(name) = entry.get("name").and_then(|i| i.as_str()) {
                ids.push(name.to_owned());
            }
        }
    }
    ids.sort();
    ids.dedup();
    Ok(ids)
}

fn cache_path(provider: &str) -> PathBuf {
    let dir = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
        .unwrap_or_else(std::env::temp_dir)
        .join("ai-overlay/models");
    dir.join(format!("{provider}.json"))
}

fn read_cache(path: &PathBuf) -> Option<Vec<String>> {
    let text = std::fs::read_to_string(path).ok()?;
    let v: serde_json::Value = serde_json::from_str(&text).ok()?;
    let fetched_at = v.get("fetched_at")?.as_u64()?;
    let age = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()?
        .saturating_sub(Duration::from_secs(fetched_at));
    if age > MODEL_CACHE_TTL {
        return None;
    }
    let arr = v.get("ids")?.as_array()?;
    let ids: Vec<String> = arr.iter().filter_map(|i| i.as_str().map(String::from)).collect();
    Some(ids)
}

/// Set `[speech] <key> = value` in config.toml, keeping its comments.
pub fn set_speech(key: &str, value: impl Into<toml_edit::Value>) -> Result<(), String> {
    let path = ai::config_path();
    let text = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
    let mut doc: toml_edit::DocumentMut = text.parse().map_err(|e| format!("{}: {e}", path.display()))?;
    doc["speech"].or_insert(toml_edit::table())[key] = toml_edit::value(value);
    std::fs::write(&path, doc.to_string()).map_err(|e| e.to_string())
}

/// Switch provider, set its model, the default question and invisibility. An empty or built-in
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
    if changes.invisible {
        if let Some(ui) = doc.get_mut("ui").and_then(|ui| ui.as_table_like_mut()) {
            ui.remove("invisible");
        }
    } else {
        doc["ui"].or_insert(toml_edit::table())["invisible"] = toml_edit::value(false);
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
