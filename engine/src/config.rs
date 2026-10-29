use std::{fs, path::PathBuf};

use anyhow::{Context, Error, ensure};
use serde::Deserialize;

// Written to disk on first run. Kept as a literal (not serialized from the
// struct) so the comments survive — plain JSON has no comments, which is why
// the file is .jsonc and parsed with json5 below.
const DEFAULT_CONFIG: &str = r#"{
    // where the OpenAI api key comes from:
    //   "sys_var"    -> read the environment variable named in api_key_var_name
    //   "config_var" -> use the api_key field below
    "api_key_method": "sys_var",
    "api_key_var_name": "OPENAI_API_KEY",
    "api_key": "",
    "voice": "onyx", // alloy, ash, ballad, coral, echo, fable, nova, onyx, sage, shimmer
    "model": "gpt-4o-mini-tts", // tts-1, tts-1-hd, gpt-4o-mini-tts
    "shortcut": null, // e.g. "ctrl+space" (the default), "cmd+shift+s" — restart to apply
    "instructions": "Speak in a cheerful and positive tone." // gpt-4o-mini-tts only: tone prompt
}
"#;

#[derive(Deserialize, Debug, Clone)]
pub struct Config {
    api_key_method: ApiKeyMethod,
    api_key_var_name: String,
    #[serde(default)] // missing in the file -> just ""
    api_key: String,
    pub voice: String,
    pub model: String,
    pub shortcut: Option<String>,
    #[serde(default)]
    pub instructions: String,
    // not part of the file; filled in by load() after parsing
    #[serde(skip)]
    pub resolved_api_key: String,
}

#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "snake_case")] // "sys_var" in json <-> SysVar in rust
enum ApiKeyMethod {
    SysVar,
    ConfigVar,
}

pub fn config_path() -> PathBuf {
    // ~/Library/Application Support on macOS, ~/.config on linux
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("rust_tts")
        .join("rust_tts_config.jsonc")
}

pub fn load() -> Result<Config, Error> {
    let path = config_path();

    if !path.exists() {
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        fs::write(&path, DEFAULT_CONFIG)?;
        println!("created default config at {}", path.display());
    }

    let raw = fs::read_to_string(&path)?;
    let mut config: Config = json5::from_str(&raw)
        .with_context(|| format!("invalid config file at {}", path.display()))?;

    config.resolved_api_key = match config.api_key_method {
        ApiKeyMethod::SysVar => std::env::var(&config.api_key_var_name)
            .with_context(|| format!("env var {} not set", config.api_key_var_name))?,
        ApiKeyMethod::ConfigVar => {
            ensure!(
                !config.api_key.is_empty(),
                "api_key_method is \"config_var\" but api_key is empty in {}",
                path.display()
            );
            config.api_key.clone()
        }
    };

    Ok(config)
}
