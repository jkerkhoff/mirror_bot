use anyhow::{anyhow, Context, Result};
use chrono::{DateTime, Duration, Utc};
use log::{debug, info, warn};
use reqwest::{
    blocking::{Client, RequestBuilder},
    header::AUTHORIZATION,
};
use serde::{Deserialize, Serialize};
use serde_json::value::Value as JsonValue;
use thiserror::Error;

use crate::settings::Settings;
// use crate::settings::{KalshiQuestionRequirements, Settings};
use crate::types::{BinaryResolution, Question, QuestionSource};

pub fn get_question(client: &Client, id: &str, config: &Settings) -> Result<KalshiQuestion> {
    debug!("get_question called (id: {})", id);
    Ok(client.get(format!("https://trading-api.kalshi.com/v1/events/{}/", id))
    .send()?
    .json()?)
}

impl KalshiQuestion {
    pub fn is_resolved(&self) -> bool {
        // todo filter for market that has the ticker_name we gave as an argument
        self.event.markets[0].status == Status::Finalized
    }
}

#[derive(Deserialize, Debug, Clone)]
pub struct KalshiQuestion {
    pub event: Event,
}

#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "snake_case")]
pub struct Event {
    pub markets: Vec<Market>,
}

#[derive(Deserialize, Debug, Clone)]
pub struct Market {
    pub status: Status,
    pub result: Option<KalshiResult>,
    pub yes_bid: i64,
    pub yes_ask: i64,
    /// The expiration date in format "2024-01-31T15:00:00Z"
    pub expiration_date: String,
    pub close_date: String,
    pub title: String,
}


#[derive(Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    //todo all options
    Active,
    Closed,
    Finalized,
}

#[derive(Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum KalshiResult {
    //todo all options
    Yes,
    No,
    #[serde(rename = "")]
    None,
}