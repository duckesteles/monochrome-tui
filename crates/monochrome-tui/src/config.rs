use anyhow::{Context, Result};
use monochrome_core::model::Quality;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub account: AccountConfig,
    pub catalog: CatalogConfig,
    pub playback: PlaybackConfig,
    pub amazon: AmazonConfig,
    pub deezer: DeezerConfig,
    pub ui: UiConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AccountConfig {
    pub auth_url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct CatalogConfig {
    pub instances: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct PlaybackConfig {
    pub quality: String,
    pub volume: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AmazonConfig {
    pub enabled: bool,
    pub url: String,
    pub bypass_token: String,
    pub api_key: String,
    pub turnstile_site_key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct DeezerConfig {
    pub enabled: bool,
    pub url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct UiConfig {
    pub accent: String,
    pub spacing: String,
}

impl Default for AccountConfig {
    fn default() -> Self {
        Self {
            auth_url: monochrome_api::auth::DEFAULT_AUTH_URL.into(),
        }
    }
}

impl Default for CatalogConfig {
    fn default() -> Self {
        Self {
            instances: monochrome_api::catalog::DEFAULT_INSTANCES
                .iter()
                .map(|(url, _)| (*url).to_string())
                .collect(),
        }
    }
}

impl Default for UiConfig {
    fn default() -> Self {
        Self {
            accent: String::new(),
            spacing: "compact".into(),
        }
    }
}

impl Default for PlaybackConfig {
    fn default() -> Self {
        Self {
            quality: "lossless".into(),
            volume: 0.7,
        }
    }
}

impl Default for AmazonConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            url: monochrome_api::stream::DEFAULT_AMAZON_URL.into(),
            bypass_token: String::new(),
            api_key: String::new(),
            turnstile_site_key: monochrome_api::turnstile::DEFAULT_SITE_KEY.into(),
        }
    }
}

impl Default for DeezerConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            url: monochrome_api::stream::DEFAULT_DEEZER_URL.into(),
        }
    }
}

impl Config {
    pub fn quality(&self) -> Quality {
        match self.playback.quality.to_ascii_lowercase().as_str() {
            "low" => Quality::Low,
            "high" => Quality::High,
            "hi-res" | "hires" | "hi_res_lossless" => Quality::HiRes,
            _ => Quality::Lossless,
        }
    }

    pub fn set_quality(&mut self, quality: Quality) {
        self.playback.quality = quality.label().into();
    }

    pub fn roomy_rows(&self) -> bool {
        self.ui.spacing.eq_ignore_ascii_case("roomy")
    }

    pub fn volume(&self) -> f32 {
        self.playback.volume.clamp(0.0, 1.0)
    }

    pub fn stream_config(&self) -> monochrome_api::StreamConfig {
        monochrome_api::StreamConfig {
            amazon_enabled: self.amazon.enabled,
            amazon_url: self.amazon.url.clone(),
            amazon_bypass_token: non_empty(&self.amazon.bypass_token),
            amazon_api_key: non_empty(&self.amazon.api_key),
            turnstile_site_key: self.amazon.turnstile_site_key.clone(),
            deezer_enabled: self.deezer.enabled,
            deezer_url: self.deezer.url.clone(),
        }
    }

    pub fn instances(&self) -> Vec<monochrome_api::Instance> {
        let known: std::collections::HashMap<&str, f32> =
            monochrome_api::catalog::DEFAULT_INSTANCES
                .iter()
                .map(|(url, version)| (*url, *version))
                .collect();
        self.catalog
            .instances
            .iter()
            .map(|url| {
                let version = known.get(url.as_str()).copied().unwrap_or(2.10);
                monochrome_api::Instance::new(url.clone(), version)
            })
            .collect()
    }

    pub fn load(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let raw = std::fs::read_to_string(path)
            .with_context(|| format!("cannot read {}", path.display()))?;
        toml::from_str(&raw).with_context(|| format!("cannot parse {}", path.display()))
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let body = toml::to_string_pretty(self)?;
        crate::paths::write_private(path, body.as_bytes())
    }
}

fn non_empty(value: &str) -> Option<String> {
    (!value.trim().is_empty()).then(|| value.trim().to_string())
}

#[derive(Debug, Clone)]
pub struct Paths {
    pub config: PathBuf,
    pub snapshot: PathBuf,
    pub log_dir: PathBuf,
}

impl Paths {
    pub fn resolve() -> Result<Self> {
        let config_dir = dirs::config_dir()
            .context("no configuration directory is available")?
            .join("monochrome-tui");
        let state_dir = dirs::state_dir()
            .or_else(dirs::data_local_dir)
            .context("no state directory is available")?
            .join("monochrome-tui");
        Ok(Self {
            config: config_dir.join("config.toml"),
            snapshot: state_dir.join("snapshot.json"),
            log_dir: state_dir.clone(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::Scratch;

    #[test]
    fn the_default_config_points_at_the_official_services() {
        let config = Config::default();
        assert_eq!(config.account.auth_url, "https://auth.monochrome.tf");
        assert!(
            config
                .catalog
                .instances
                .iter()
                .all(|url| url.starts_with("https://"))
        );
        assert!(config.amazon.enabled);
        assert!(config.deezer.enabled);
    }

    #[test]
    fn the_default_look_is_pure_monochrome() {
        assert!(Config::default().ui.accent.is_empty());
    }

    #[test]
    fn rows_are_compact_unless_roomy_is_asked_for() {
        let mut config = Config::default();
        assert!(!config.roomy_rows());
        config.ui.spacing = "roomy".into();
        assert!(config.roomy_rows());
        config.ui.spacing = "ROOMY".into();
        assert!(config.roomy_rows());
        config.ui.spacing = "nonsense".into();
        assert!(!config.roomy_rows());
    }

    #[test]
    fn quality_round_trips_through_the_config() {
        let mut config = Config::default();
        for quality in Quality::ALL {
            config.set_quality(quality);
            assert_eq!(config.quality(), quality);
        }
    }

    #[test]
    fn an_unknown_quality_falls_back_to_lossless() {
        let mut config = Config::default();
        config.playback.quality = "nonsense".into();
        assert_eq!(config.quality(), Quality::Lossless);
    }

    #[test]
    fn volume_is_clamped_when_read() {
        let mut config = Config::default();
        config.playback.volume = 5.0;
        assert_eq!(config.volume(), 1.0);
        config.playback.volume = -2.0;
        assert_eq!(config.volume(), 0.0);
    }

    #[test]
    fn blank_credentials_are_treated_as_absent() {
        let mut config = Config::default();
        config.amazon.bypass_token = "   ".into();
        let stream = config.stream_config();
        assert!(stream.amazon_bypass_token.is_none());
        assert!(stream.amazon_api_key.is_none());
    }

    #[test]
    fn credentials_are_trimmed() {
        let mut config = Config::default();
        config.amazon.api_key = "  key  ".into();
        assert_eq!(
            config.stream_config().amazon_api_key.as_deref(),
            Some("key")
        );
    }

    #[test]
    fn a_missing_config_file_yields_the_defaults() {
        let scratch = Scratch::new("absent");
        let config = Config::load(&scratch.file("config.toml")).expect("defaults");
        assert_eq!(config.playback.quality, "lossless");
    }

    #[test]
    fn a_config_file_round_trips() {
        let scratch = Scratch::new("roundtrip");
        let path = scratch.file("config.toml");
        let mut config = Config::default();
        config.playback.volume = 0.42;
        config.amazon.bypass_token = "abc".into();
        config.save(&path).expect("save");

        let loaded = Config::load(&path).expect("load");
        assert_eq!(loaded.playback.volume, 0.42);
        assert_eq!(loaded.amazon.bypass_token, "abc");
    }

    #[test]
    fn a_partial_config_file_keeps_the_defaults_for_the_rest() {
        let parsed: Config = toml::from_str("[playback]\nvolume = 0.1\n").expect("parse");
        assert_eq!(parsed.playback.volume, 0.1);
        assert_eq!(parsed.playback.quality, "lossless");
        assert_eq!(parsed.account.auth_url, "https://auth.monochrome.tf");
    }

    #[test]
    fn user_supplied_instances_are_used_verbatim() {
        let mut config = Config::default();
        config.catalog.instances = vec!["https://my.instance".into()];
        let instances = config.instances();
        assert_eq!(instances.len(), 1);
        assert_eq!(instances[0].url, "https://my.instance");
    }
}
