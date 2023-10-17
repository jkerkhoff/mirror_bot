use anyhow::{Context, Result};
use config::{Config, Environment, File, FileFormat};
use log::debug;
use serde::Deserialize;
use std::{
    collections::HashSet,
    env::{self, VarError},
};

#[derive(Debug, Deserialize)]
pub struct Database {
    pub path: String,
}

#[derive(Debug, Deserialize)]
pub struct Kalshi {
    pub add_group_ids: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct MarketTemplate {
    pub description_footer: String,
    pub title_retain_end_characters: usize,
    pub max_question_length: usize,
    pub max_description_length: usize,
}

#[derive(Debug, Deserialize)]
pub struct Managrams {
    /// minimum amount that can be sent
    pub min_amount: f64,
    /// amount we want to charge people for mirroring
    pub mirror_cost: f64,
}

#[derive(Debug, Deserialize)]
pub struct Manifold {
    pub url: String,
    pub api_key: String,
    pub user_id: String,
    pub template: MarketTemplate,
    pub managrams: Managrams,
}

#[derive(Debug, Deserialize)]
pub struct MetaculusQuestionRequirements {
    pub require_visible_community_prediction: bool,
    pub require_open: bool,
    pub exclude_resolved: bool,
    pub exclude_grouped: bool,
    pub min_forecasters: i64,
    pub min_votes: i64,
    pub min_days_to_resolution: i64,
    pub max_days_to_resolution: i64,
    /// require question to have had activity in the last n days
    pub max_last_active_days: i64,
    pub max_age_days: i64,
    /// exclude question if community forecast puts a high probability on YES or NO
    pub max_confidence: f64,
    pub exclude_ids: HashSet<i64>,
}

#[derive(Debug, Deserialize)]
pub struct Metaculus {
    pub url: String,
    pub api_key: String,
    pub max_clones_per_day: usize,
    pub fetch_criteria: bool,
    pub auto_filter: MetaculusQuestionRequirements,
    pub request_filter: MetaculusQuestionRequirements,
    pub add_group_ids: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct Settings {
    pub database: Database,
    pub kalshi: Kalshi,
    pub manifold: Manifold,
    pub metaculus: Metaculus,
}

impl Settings {
    fn config_path() -> String {
        match env::var("MB_CONFIG_PATH") {
            Ok(path) => path,
            Err(VarError::NotPresent) => "config.toml".to_string(),
            Err(VarError::NotUnicode(_)) => panic!("MB_CONFIG_PATH should be valid unicode"),
        }
    }

    fn config_override_path() -> Option<String> {
        match env::var("MB_CONFIG_OVERRIDE_PATH") {
            Ok(path) => Some(path),
            Err(VarError::NotPresent) => None,
            Err(VarError::NotUnicode(_)) => {
                panic!("MB_CONFIG_OVERRIDE_PATH should be valid unicode")
            }
        }
    }

    pub fn new() -> Result<Self> {
        let mut cfg =
            Config::builder().add_source(File::new(&Self::config_path(), FileFormat::Toml));
        if let Some(override_path) = Self::config_override_path() {
            debug!("Applying config overrides from {}", override_path);
            cfg = cfg.add_source(File::new(&override_path, FileFormat::Toml));
        }
        cfg.add_source(Environment::with_prefix("MB"))
            .build()
            .with_context(|| "failed to build config")?
            .try_deserialize()
            .with_context(|| "failed to deserialize config")
    }
}
