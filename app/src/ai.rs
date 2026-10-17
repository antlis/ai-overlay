//! LLM client for any OpenAI-compatible chat-completions API
//! (NanoGPT, OpenCode Zen, OpenRouter, Ollama, ...), configured in
//! `$XDG_CONFIG_HOME/invisible-ai/config.toml`.

use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::time::Duration;

use base64::Engine;
use serde::Deserialize;
use serde_json::{json, Value};

const SYSTEM_PROMPT: &str = "You are a concise assistant shown in a small overlay on the user's \
screen. You get a screenshot of their screen and a question. Answer directly and briefly; \
use short Markdown (lists, code blocks) only when it helps.";

#[derive(Deserialize)]
pub struct Config {
    /// Name of the entry in `providers` to use.
    pub provider: String,
    pub providers: HashMap<String, Provider>,
}

#[derive(Deserialize)]
pub struct Provider {
    /// e.g. `https://nano-gpt.com/api/v1`; `/chat/completions` is appended.
    pub base_url: String,
    pub model: String,
    /// Environment variable holding the API key.
    pub api_key_env: Option<String>,
    /// Shell command printing the API key (used if `api_key_env` is unset or empty).
    pub api_key_cmd: Option<String>,
}

pub fn config_path() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".config"))
        .join("invisible-ai/config.toml")
}

pub fn load_config() -> Result<Config, String> {
    let path = config_path();
    let text = std::fs::read_to_string(&path)
        .map_err(|e| format!("cannot read {}: {e} (see config.example.toml)", path.display()))?;
    toml::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))
}

impl Config {
    pub fn active(&self) -> Result<&Provider, String> {
        self.providers
            .get(&self.provider)
            .ok_or_else(|| format!("provider '{}' not found in [providers]", self.provider))
    }
}

impl Provider {
    fn api_key(&self) -> Result<String, String> {
        if let Some(key) = self.api_key_env.as_deref().and_then(|v| std::env::var(v).ok()) {
            if !key.is_empty() {
                return Ok(key);
            }
        }
        let cmd = self
            .api_key_cmd
            .as_deref()
            .ok_or("no API key: set api_key_env or api_key_cmd")?;
        let out = std::process::Command::new("sh")
            .args(["-c", cmd])
            .output()
            .map_err(|e| format!("api_key_cmd: {e}"))?;
        let key = String::from_utf8_lossy(&out.stdout).trim().to_owned();
        if !out.status.success() || key.is_empty() || key == "null" {
            return Err("api_key_cmd printed no key".into());
        }
        Ok(key)
    }

    /// Stream an answer about `screenshot_png`, calling `on_delta` per text chunk.
    pub fn ask(
        &self,
        question: &str,
        screenshot_png: &[u8],
        mut on_delta: impl FnMut(&str),
    ) -> Result<(), String> {
        let image = base64::engine::general_purpose::STANDARD.encode(screenshot_png);
        let body = json!({
            "model": self.model,
            "stream": true,
            "messages": [
                { "role": "system", "content": SYSTEM_PROMPT },
                { "role": "user", "content": [
                    { "type": "text", "text": question },
                    { "type": "image_url", "image_url": { "url": format!("data:image/png;base64,{image}") } },
                ]},
            ],
        });

        let url = format!("{}/chat/completions", self.base_url.trim_end_matches('/'));
        let response = ureq::AgentBuilder::new()
            .timeout_connect(Duration::from_secs(10))
            // Between streamed chunks; reasoning models can think quietly for a while.
            .timeout_read(Duration::from_secs(90))
            .build()
            .post(&url)
            .set("Authorization", &format!("Bearer {}", self.api_key()?))
            .send_json(body)
            .map_err(|e| match e {
                ureq::Error::Status(code, r) => {
                    format!("HTTP {code}: {}", r.into_string().unwrap_or_default())
                }
                e => e.to_string(),
            })?;

        // Server-sent events: `data: {json}` lines, terminated by `data: [DONE]`.
        for line in BufReader::new(response.into_reader()).lines() {
            let line = line.map_err(|e| e.to_string())?;
            let Some(data) = line.strip_prefix("data:").map(str::trim) else {
                continue;
            };
            if data == "[DONE]" {
                break;
            }
            let event: Value = serde_json::from_str(data).map_err(|e| format!("bad event: {e}"))?;
            if let Some(err) = event.get("error") {
                return Err(err.to_string());
            }
            // Reasoning models also stream `reasoning`/`reasoning_content`; only show the answer.
            if let Some(text) = event["choices"][0]["delta"]["content"].as_str() {
                on_delta(text);
            }
        }
        Ok(())
    }
}
