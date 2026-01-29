use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceKeys {
    pub api_key: String,
    pub api_secret: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Account {
    pub service: String,
    pub username: String,
    pub password_md5: String,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub services: HashMap<String, ServiceKeys>,
    #[serde(default)]
    pub accounts: Vec<Account>,
}

pub fn default_config_path() -> PathBuf {
    let fallback = PathBuf::from(".config/cobblestone/config.json");
    dirs::home_dir().map_or(fallback, |home| {
        home.join(".config/cobblestone/config.json")
    })
}

pub fn load_config(path: &Path) -> Result<Config> {
    if !path.exists() {
        return Ok(Config::default());
    }
    let raw = fs::read_to_string(path)
        .with_context(|| format!("Failed reading config at {}", path.display()))?;
    let config = serde_json::from_str(&raw)
        .with_context(|| format!("Failed parsing config at {}", path.display()))?;
    Ok(config)
}

pub fn save_config(config: &Config, path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("Failed creating config directory {}", parent.display()))?;
    }
    let serialized =
        serde_json::to_string_pretty(config).context("Failed serializing config to JSON")?;
    fs::write(path, format!("{serialized}\n"))
        .with_context(|| format!("Failed writing config at {}", path.display()))?;
    Ok(())
}

pub fn set_service_keys(config: &mut Config, service: &str, api_key: &str, api_secret: &str) {
    config.services.insert(
        service.to_string(),
        ServiceKeys {
            api_key: api_key.to_string(),
            api_secret: api_secret.to_string(),
        },
    );
}

pub fn add_account(config: &mut Config, service: &str, username: &str, password: &str) {
    let password_md5 = format!("{:x}", md5::compute(password));
    for account in &mut config.accounts {
        if account.service == service && account.username == username {
            account.password_md5 = password_md5;
            return;
        }
    }
    config.accounts.push(Account {
        service: service.to_string(),
        username: username.to_string(),
        password_md5,
    });
}

pub fn remove_account(config: &mut Config, service: &str, username: &str) -> bool {
    let original_len = config.accounts.len();
    config
        .accounts
        .retain(|account| !(account.service == service && account.username == username));
    config.accounts.len() != original_len
}

pub fn iter_accounts<'a>(
    config: &'a Config,
    service: Option<&str>,
) -> impl Iterator<Item = &'a Account> {
    config
        .accounts
        .iter()
        .filter(move |account| service.is_none_or(|svc| svc == account.service))
}

pub fn get_service_keys<'a>(config: &'a Config, service: &str) -> Option<&'a ServiceKeys> {
    config.services.get(service)
}
