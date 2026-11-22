use std::{fs, path::PathBuf};

use anyhow::{Context, Error, bail, ensure};
use serde::{Deserialize, Serialize};

pub const VOICES: &[&str] = &[
    "alloy", "ash", "ballad", "cedar", "coral", "echo", "fable", "marin", "nova", "onyx", "sage",
    "shimmer", "verse",
];
pub const MODELS: &[&str] = &["gpt-4o-mini-tts", "tts-1", "tts-1-hd"];

pub const ENGINES: &[&str] = &["speechify", "openai"];
/// Speechify's voice ids are many; these are the ones worth picking from.
pub const SPEECHIFY_VOICES: &[&str] = &[
    "benjamin", "oliver", "graham", "steve", "dorian", "cleon", "rob", "hugh_32", "wyatt_32",
    "ron", "alec", "michael", "collin", "phil", "fraser", "lorne", "leonard", "earnest", "hamish",
];
pub const SPEECHIFY_MODELS: &[&str] = &["simba-3.2", "simba-3.5"];

#[derive(Deserialize, Serialize, Debug, Clone, PartialEq)]
pub struct Config {
    api_key_method: ApiKeyMethod,
    api_key_var_name: String,
    /// "speechify" (default) or "openai". Files from before the switch have no
    /// engine and so move to Speechify; their OpenAI voice and model stay as the fallback.
    #[serde(default = "default_engine")]
    pub engine: String,
    #[serde(default = "default_speechify_var")]
    speechify_api_key_var_name: String,
    #[serde(default = "default_speechify_voice")]
    pub speechify_voice: String,
    #[serde(default = "default_speechify_model")]
    pub speechify_model: String,
    #[serde(default)] // missing in the file -> just ""
    api_key: String,
    #[serde(default)]
    api_key_file: String,
    pub voice: String,
    pub model: String,
    pub shortcut: Option<String>,
    #[serde(default)]
    pub instructions: String,
    #[serde(default = "one")]
    pub speed: f32,
    #[serde(default = "one")]
    pub volume: f32,
    // not part of the file; filled in by load() after parsing
    #[serde(skip)]
    pub resolved_api_key: String,
    #[serde(skip)]
    pub resolved_speechify_key: String,
}

fn default_engine() -> String {
    "speechify".into()
}
fn default_speechify_var() -> String {
    "SPEECHIFY_API_KEY".into()
}
fn default_speechify_voice() -> String {
    "benjamin".into()
}
fn default_speechify_model() -> String {
    "simba-3.2".into()
}

fn one() -> f32 {
    1.0
}

#[derive(Deserialize, Serialize, Debug, Clone, Copy, PartialEq)]
#[serde(rename_all = "snake_case")] // "sys_var" in json <-> SysVar in rust
enum ApiKeyMethod {
    SysVar,
    ConfigVar,
    EnvFile,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            api_key_method: ApiKeyMethod::SysVar,
            api_key_var_name: "OPENAI_API_KEY".into(),
            engine: default_engine(),
            speechify_api_key_var_name: default_speechify_var(),
            speechify_voice: default_speechify_voice(),
            speechify_model: default_speechify_model(),
            api_key: String::new(),
            api_key_file: String::new(),
            voice: "onyx".into(),
            model: "gpt-4o-mini-tts".into(),
            shortcut: None,
            instructions: "Speak in a calm, even tone.".into(),
            speed: 1.0,
            volume: 1.0,
            resolved_api_key: String::new(),
            resolved_speechify_key: String::new(),
        }
    }
}

impl Config {
    /// The file as written to disk. Kept as a template rather than plain
    /// serde output so the comments survive every save from the app.
    pub fn render(&self) -> String {
        let s = |v: &str| serde_json::to_string(v).unwrap_or_else(|_| "\"\"".into());
        let method = match self.api_key_method {
            ApiKeyMethod::SysVar => "sys_var",
            ApiKeyMethod::ConfigVar => "config_var",
            ApiKeyMethod::EnvFile => "env_file",
        };
        let shortcut = self.shortcut.as_deref().map_or("null".into(), s);

        format!(
            r#"{{
    // where the OpenAI api key comes from:
    //   "sys_var"    -> the environment variable named in api_key_var_name
    //   "config_var" -> the api_key field below
    //   "env_file"   -> the line api_key_var_name=... in the .env file at api_key_file
    "api_key_method": {method},
    "api_key_var_name": {var},
    // which service reads the text aloud: "speechify" (default) or "openai" (fallback).
    // Speechify's key is the line speechify_api_key_var_name=... in the same key source.
    "engine": {engine},
    "speechify_api_key_var_name": {svar},
    "speechify_voice": {svoice}, // e.g. {svoices}
    "speechify_model": {smodel}, // {smodels}
    "api_key": {key},
    "api_key_file": {file},
    "voice": {voice}, // openai only: {voices}
    "model": {model}, // openai only: {models}
    "shortcut": {shortcut}, // e.g. "ctrl+space" (the default), "cmd+shift+s"; restart to apply
    "instructions": {instructions}, // openai gpt-4o-mini-tts only: tone prompt
    "speed": {speed}, // playback speed, 0.5 to 3, pitch kept
    "volume": {volume} // 0 to 1
}}
"#,
            method = s(method),
            var = s(&self.api_key_var_name),
            engine = s(&self.engine),
            svar = s(&self.speechify_api_key_var_name),
            svoice = s(&self.speechify_voice),
            svoices = SPEECHIFY_VOICES.join(", "),
            smodel = s(&self.speechify_model),
            smodels = SPEECHIFY_MODELS.join(", "),
            key = s(&self.api_key),
            file = s(&self.api_key_file),
            voice = s(&self.voice),
            voices = VOICES.join(", "),
            model = s(&self.model),
            models = MODELS.join(", "),
            instructions = s(&self.instructions),
            speed = self.speed,
            volume = self.volume,
        )
    }

    pub fn save(&self) -> Result<(), Error> {
        let path = config_path();
        // Through a temp file, so a crash mid-write cannot leave half a config.
        let tmp = path.with_extension("jsonc.tmp");
        fs::write(&tmp, self.render())?;
        fs::rename(&tmp, &path)?;
        Ok(())
    }

    fn resolve_api_key(&self, var_name: &str) -> Result<String, Error> {
        match self.api_key_method {
            ApiKeyMethod::SysVar => std::env::var(var_name)
                .with_context(|| format!("env var {var_name} not set")),
            ApiKeyMethod::ConfigVar => {
                ensure!(!self.api_key.is_empty(), "api_key_method is \"config_var\" but api_key is empty");
                Ok(self.api_key.clone())
            }
            ApiKeyMethod::EnvFile => {
                let raw = fs::read_to_string(&self.api_key_file)
                    .with_context(|| format!("cannot read api_key_file {}", self.api_key_file))?;
                match env_value(&raw, var_name) {
                    Some(key) => Ok(key),
                    None => bail!("{} is not in {}", var_name, self.api_key_file),
                }
            }
        }
    }
}

/// One `NAME=value` line out of a .env file, quotes stripped.
fn env_value(raw: &str, name: &str) -> Option<String> {
    raw.lines().find_map(|line| {
        let (key, value) = line.trim().split_once('=')?;
        (key.trim() == name).then(|| value.trim().trim_matches('"').trim_matches('\'').to_string())
    })
}

pub fn config_path() -> PathBuf {
    // ~/Library/Application Support on macOS, ~/.config on linux
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("rust_tts")
        .join("rust_tts_config.jsonc")
}

pub fn parse(raw: &str) -> Result<Config, Error> {
    Ok(json5::from_str(raw)?)
}

pub fn load() -> Result<Config, Error> {
    let path = config_path();

    if !path.exists() {
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        fs::write(&path, Config::default().render())?;
        println!("created default config at {}", path.display());
    }

    let raw = fs::read_to_string(&path)?;
    let mut config =
        parse(&raw).with_context(|| format!("invalid config file at {}", path.display()))?;
    // The engine in use must have its key; the other one's key is a bonus, so a
    // missing OpenAI key never stops Speechify and the other way round.
    let openai = config.resolve_api_key(&config.api_key_var_name);
    let speechify = config.resolve_api_key(&config.speechify_api_key_var_name);
    let active = if config.engine == "openai" { &openai } else { &speechify };
    if let Err(e) = active {
        bail!("{e:#}");
    }
    config.resolved_api_key = openai.unwrap_or_default();
    config.resolved_speechify_key = speechify.unwrap_or_default();

    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_parses_back_to_the_same_config() {
        let config = Config {
            instructions: "Calm, \"slow\".\nNever excited.".into(),
            shortcut: Some("ctrl+space".into()),
            speed: 1.25,
            volume: 0.8,
            ..Config::default()
        };
        assert_eq!(parse(&config.render()).unwrap(), config);
    }

    #[test]
    fn old_files_move_to_speechify_and_keep_their_openai_voice() {
        let raw = r#"{
            "api_key_method": "sys_var", "api_key_var_name": "X", "voice": "onyx",
            "model": "tts-1", "shortcut": null,
        }"#;
        let config = parse(raw).unwrap();
        assert_eq!(config.engine, "speechify");
        assert_eq!(config.speechify_voice, "benjamin");
        assert_eq!(config.speechify_model, "simba-3.2");
        assert_eq!(config.voice, "onyx");
    }

    #[test]
    fn old_files_without_speed_or_volume_still_load() {
        let raw = r#"{
            "api_key_method": "sys_var", "api_key_var_name": "X", "voice": "onyx",
            "model": "tts-1", "shortcut": null, // comment
        }"#;
        let config = parse(raw).unwrap();
        assert_eq!(config.speed, 1.0);
        assert_eq!(config.volume, 1.0);
    }

    #[test]
    fn env_file_lines_are_read_with_quotes_stripped() {
        let raw = "# keys\nOTHER=1\nTTS_KEY=\"sk-abc\"\n";
        assert_eq!(env_value(raw, "TTS_KEY").as_deref(), Some("sk-abc"));
        assert_eq!(env_value(raw, "MISSING"), None);
    }
}
