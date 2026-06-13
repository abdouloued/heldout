use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

pub const CONFIG_FILE: &str = "tattle.yaml";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Config {
    #[serde(default = "default_version")]
    pub version: u8,
    #[serde(default)]
    pub replay: ReplayConfig,
    #[serde(default)]
    pub judge: JudgeConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReplayConfig {
    #[serde(default)]
    pub commands: Vec<String>,
    #[serde(default = "default_replay_timeout")]
    pub timeout_secs: u64,
}

impl Default for ReplayConfig {
    fn default() -> Self {
        Self {
            commands: vec![],
            timeout_secs: default_replay_timeout(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct JudgeConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_judge_provider")]
    pub provider: String,
    #[serde(default = "default_judge_model")]
    pub model: String,
    #[serde(default = "default_judge_endpoint")]
    pub endpoint: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            version: default_version(),
            replay: ReplayConfig::default(),
            judge: JudgeConfig::default(),
        }
    }
}

impl Default for JudgeConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            provider: default_judge_provider(),
            model: default_judge_model(),
            endpoint: default_judge_endpoint(),
        }
    }
}

pub fn init(root: &Path) -> Result<PathBuf> {
    let path = root.join(CONFIG_FILE);
    if !path.exists() {
        let config = Config::default();
        let yaml = serde_yaml::to_string(&config).context("serialize default config")?;
        fs::write(&path, yaml).with_context(|| format!("write {}", path.display()))?;
    }
    Ok(path)
}

pub fn load(root: &Path) -> Result<Config> {
    let path = root.join(CONFIG_FILE);
    if !path.exists() {
        return Ok(Config::default());
    }
    let text = fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
    serde_yaml::from_str(&text).with_context(|| format!("parse {}", path.display()))
}

fn default_version() -> u8 {
    1
}

fn default_judge_provider() -> String {
    "none".to_string()
}

fn default_judge_model() -> String {
    "qwen3.5:2b".to_string()
}

fn default_judge_endpoint() -> String {
    "http://localhost:11434".to_string()
}

fn default_replay_timeout() -> u64 {
    120
}
