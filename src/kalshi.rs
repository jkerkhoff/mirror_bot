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
    let mut question = client.get(format!("https://trading-api.kalshi.com/v1/events/{}/", id))
        .send()?
        .json::<KalshiQuestion>()?;
    question.id = id.to_string();
    Ok(question)
}

impl KalshiQuestion {
    pub fn get_market(&self) -> &Market {
        self.event
            .markets
            .iter()
            .find(|market| market.ticker_name == self.id)
            .unwrap()
    }

    pub fn is_resolved(&self) -> bool {
        self.get_market().status == Status::Finalized
    }

    pub fn full_url(&self) -> String {
        // TODO: grab base from config (consistent with manifold)?
        // Don't think there even is a public dev instance though.
        format!("https://kalshi.com/markets/{}", self.event.series_ticker)
    }
}

impl TryInto<Question> for &KalshiQuestion {
    type Error = anyhow::Error;

    fn try_into(self) -> Result<Question> {
        Ok(Question {
            source: QuestionSource::Kalshi,
            source_url: self.full_url(),
            source_id: self.id.clone(),
            question: self.get_market().title.clone(),
            criteria: Some(self.event.underlying.clone()),
            end_date: self.get_market().expiration_date,
        })
    }
}


#[derive(Deserialize, Debug, Clone)]
pub struct KalshiQuestion {
    pub event: Event,
    #[serde(skip)]
    pub id: String,
}

#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "snake_case")]
pub struct Event {
    pub series_ticker: String,
    pub markets: Vec<Market>,
    pub underlying: String,
}

#[derive(Deserialize, Debug, Clone)]
pub struct Market {
    pub ticker_name: String,
    pub status: Status,
    pub result: Option<KalshiResult>,
    pub yes_bid: i64,
    pub yes_ask: i64,
    /// The expiration date in format "2024-01-31T15:00:00Z"
    pub expiration_date: DateTime<Utc>,
    pub close_date: DateTime<Utc>,
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