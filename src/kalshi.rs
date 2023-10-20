use anyhow::{Context, Result};
use chrono::{DateTime, Duration, Utc};
use log::{debug, info};
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::settings::{KalshiQuestionRequirements, Settings};
use crate::types::{Question, QuestionSource};

fn list_questions(
    client: &Client,
    params: KalshiListQuestionsParams,
) -> Result<KalshiQuestionsResponse> {
    debug!("kalshi::list_questions called"); // (params: {:?})", params);
    Ok(client.get("https://trading-api.kalshi.com/v1/events/")
    .query(&params)
    .send()?
    .json()?)
}

pub fn get_question(client: &Client, id: &str, _config: &Settings) -> Result<KalshiQuestion> {
    // The Kalshi api requires the id (ticker) to be uppercase. Their frontend
    // uses lowercase by default, but redirects given uppercase. Use
    // uppercase to be safe.
    let id = id.to_uppercase();
    debug!("get_question called (id: {})", id);
    let mut question = client.get(format!("https://trading-api.kalshi.com/v1/events/{}/", id))
        .send()?
        .json::<KalshiQuestion>()?;
    question.id = id.to_string();
    Ok(question)
}

pub fn get_mirror_candidates(client: &Client, config: &Settings) -> Result<Vec<KalshiQuestion>> {
    info!("Fetching mirror candidates from Kalshi");
    let requirements = &config.kalshi.auto_filter;
    let mut params = KalshiListQuestionsParams {
        single_event_per_series: Some(true),
        ..Default::default()
    };
    if requirements.require_open {
        params.status = Some("open".to_string()); // TODO: use enum?
    }
    let questions = list_questions(client, params)
        .with_context(|| "failed to fetch questions from kalshi")?
        .events
        .into_iter()
        .map(|event| KalshiQuestion {
            id: event.ticker.clone(),
            event,
        })
        .filter(|q| check_event_requirements(q, requirements).is_ok())
        .collect();
    Ok(questions)
}

pub fn check_event_requirements(
    question: &KalshiQuestion,
    requirements: &KalshiQuestionRequirements,
) -> Result<(), KalshiCheckFailure> {
    if ! question.has_matching_market() {
        return Err(KalshiCheckFailure::NoMatchingMarket);
    }
    // config requirements
    if requirements.exclude_series && question.is_series() {
        return Err(KalshiCheckFailure::Series);
    }
    if requirements.require_open && !question.is_active() {
        return Err(KalshiCheckFailure::NotActive);
    }
    if requirements.exclude_resolved && question.is_resolved() {
        return Err(KalshiCheckFailure::Resolved);
    }
    // Min liquidity
    if question.get_market().liquidity < requirements.min_liquidity {
        return Err(KalshiCheckFailure::NotEnoughLiquidity {
            liquidity: question.get_market().liquidity,
            threshold: requirements.min_liquidity,
        });
    }
    // Min volume
    if question.get_market().volume < requirements.min_volume {
        return Err(KalshiCheckFailure::NotEnoughVolume {
            volume: question.get_market().volume,
            threshold: requirements.min_volume,
        });
    }
    // Min recent volume
    if question.get_market().recent_volume < requirements.min_recent_volume {
        return Err(KalshiCheckFailure::NotEnoughRecentVolume {
            recent_volume: question.get_market().recent_volume,
            threshold: requirements.min_recent_volume,
        });
    }
    // Min open interest
    if question.get_market().open_interest < requirements.min_open_interest {
        return Err(KalshiCheckFailure::NotEnoughOpenInterest {
            open_interest: question.get_market().open_interest,
            threshold: requirements.min_open_interest,
        });
    }
    // min dollar volume
    if question.get_market().dollar_volume < requirements.min_dollar_volume {
        return Err(KalshiCheckFailure::NotEnoughDollarVolume {
            dollar_volume: question.get_market().dollar_volume,
            threshold: requirements.min_dollar_volume,
        });
    }
    // min dollar recent volume
    if question.get_market().dollar_recent_volume < requirements.min_dollar_recent_volume {
        return Err(KalshiCheckFailure::NotEnoughDollarRecentVolume {
            dollar_recent_volume: question.get_market().dollar_recent_volume,
            threshold: requirements.min_dollar_recent_volume,
        });
    }
    // min dollar open interest
    if question.get_market().dollar_open_interest < requirements.min_dollar_open_interest {
        return Err(KalshiCheckFailure::NotEnoughDollarOpenInterest {
            dollar_open_interest: question.get_market().dollar_open_interest,
            threshold: requirements.min_dollar_open_interest,
        });
    }

    if question.time_to_resolution() < Duration::days(requirements.min_days_to_resolution) {
        return Err(KalshiCheckFailure::ResolvesTooSoon {
            days_remaining: question.time_to_resolution().num_days(),
            threshold: requirements.min_days_to_resolution,
        });
    }
    if question.time_to_resolution() > Duration::days(requirements.max_days_to_resolution) {
        return Err(KalshiCheckFailure::ResolvesTooLate {
            days_remaining: question.time_to_resolution().num_days(),
            threshold: requirements.max_days_to_resolution,
        });
    }
    if question.age() > Duration::days(requirements.max_age_days) {
        return Err(KalshiCheckFailure::TooOld {
            age_days: question.age().num_days(),
            threshold: requirements.max_age_days,
        });
    }
    if (100 - question.get_market().yes_ask) > requirements.max_confidence * 100
    || question.get_market().yes_bid > requirements.max_confidence * 100 {
        return Err(KalshiCheckFailure::TooExtreme {
            yes_ask: question.get_market().yes_ask,
            yes_bid: question.get_market().yes_bid,
            threshold: requirements.max_confidence,
        });
    }
    if requirements.exclude_ids.contains(&question.id) {
        return Err(KalshiCheckFailure::Banned);
    }

    Ok(())
}

impl KalshiQuestion {
    pub fn age(&self) -> Duration {
        Utc::now() - self.get_market().open_date
    }

    pub fn has_matching_market(&self) -> bool {
        self.event
            .markets
            .iter()
            .find(|market| market.ticker_name == self.id)
            .is_some()
    }

    pub fn get_market(&self) -> &Market {
        self.event
            .markets
            .iter()
            .find(|market| market.ticker_name == self.id)
            .with_context(|| format!("Could not find market in series {} with ticker_name {}", self.id, self.id))
            .unwrap()
    }

    pub fn is_resolved(&self) -> bool {
        self.get_market().status == Status::Finalized
    }

    pub fn is_active(&self) -> bool {
        self.get_market().status == Status::Active
    }

    pub fn is_series(&self) -> bool {
        self.event.markets.len() > 1
    }

    pub fn time_to_resolution(&self) -> Duration {
        self.get_market().expiration_date - Utc::now()
    }

    pub fn full_url(&self) -> String {
        // TODO: grab base from config (consistent with manifold)?
        format!("https://kalshi.com/markets/{}", self.event.series_ticker)
    }

    pub fn title(&self) -> String {
        self.get_market().title.clone()
    }

    pub fn get_criteria_and_sources(&self) -> String {
        format!("{}{}", self.format_underlying_rulebook_variables(), self.get_resolution_sources_markdown())
    }

    pub fn format_underlying_rulebook_variables(&self) -> String {
        let mut return_string = self.event.underlying.clone();
        // For each variable in market.rulebook_variables, substitute ||variable|| with the value of the variable
        let rulebook = self.get_market().rulebook_variables.clone();
        // For each key in rulebook
        for key in rulebook.as_object().unwrap().keys() {
            // Remove the surrounding "" marks
            let mut replacement_value = rulebook[key].to_string();
            replacement_value = replacement_value[1..replacement_value.len()-1].to_string();

            // We've seen instances with and without spaces
            return_string = return_string.replace(&format!("||{}||", key), &replacement_value);
            return_string = return_string.replace(&format!("|| {} ||", key), &replacement_value);
        }
        return return_string

    }

    pub fn get_resolution_sources_markdown(&self) -> String {
        let sources = self
            .event
            .settlement_sources
            .iter()
            .map(|source| format!("<{}>", source.url.clone()))
            .collect::<Vec<String>>();

        // Return "" if there are none
        if sources.is_empty() {
            return "".to_string();
        }
        format!("\n\n\n**Resolution sources**\n\n{}", sources.join(", "))
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
            criteria: Some(self.get_criteria_and_sources()),
            end_date: self.get_market().expiration_date,
        })
    }
}

#[derive(Deserialize, Debug, Clone)]
pub struct KalshiQuestionsResponse {
    pub events: Vec<Event>,
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
    pub ticker: String,
    pub markets: Vec<Market>,
    pub settlement_sources: Vec<SettlementSource>,
    pub underlying: String,
}

#[derive(Deserialize, Debug, Clone)]
pub struct SettlementSource {
    pub name: String,
    pub url: String,
}

#[derive(Deserialize, Debug, Clone)]
pub struct Market {
    pub title: String,
    pub ticker_name: String,
    pub status: Status,
    pub open_date: DateTime<Utc>,
    pub result: Option<KalshiResult>,
    pub yes_bid: i64,
    pub yes_ask: i64,
    pub expiration_date: DateTime<Utc>, // Unsure if we should use close_date, which is earlier
    pub volume: i64,
    pub recent_volume: i64,
    pub open_interest: i64,
    pub dollar_volume: i64,
    pub dollar_recent_volume: i64,
    pub dollar_open_interest: i64,
    pub liquidity: i64,
    pub rulebook_variables: serde_json::Value,
}


#[derive(Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    //todo all options
    Active,
    Closed,
    Finalized, // In GET parameters, use status=settled instead, even though "settled" never shows up in the json response
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

#[derive(Serialize, Debug, Default)]
pub struct KalshiListQuestionsParams {
    pub status: Option<String>,
    pub single_event_per_series: Option<bool>,
}

#[derive(Error, Debug)]
pub enum KalshiCheckFailure {
    #[error("question does not have matching market id")]
    NoMatchingMarket,
    #[error("question is not active")]
    NotActive,
    #[error("question is part of a group")]
    Series,
    #[error("question has {volume} volume, and the minimum is {threshold}")]
    NotEnoughVolume { volume: i64, threshold: i64 },
    #[error("question has {recent_volume} recent_volume, and the minimum is {threshold}")]
    NotEnoughRecentVolume { recent_volume: i64, threshold: i64 },
    #[error("question has {open_interest} open_interest, and the minimum is {threshold}")]
    NotEnoughOpenInterest { open_interest: i64, threshold: i64 },
    #[error("question has {liquidity} liquidity, and the minimum is {threshold}")]
    NotEnoughLiquidity { liquidity: i64, threshold: i64 },
    #[error("question has {dollar_volume} dollar_volume, and the minimum is {threshold}")]
    NotEnoughDollarVolume { dollar_volume: i64, threshold: i64 },
    #[error("question has {dollar_recent_volume} dollar_recent_volume, and the minimum is {threshold}")]
    NotEnoughDollarRecentVolume { dollar_recent_volume: i64, threshold: i64 },
    #[error("question has {dollar_open_interest} dollar_open_interest, and the minimum is {threshold}")]
    NotEnoughDollarOpenInterest { dollar_open_interest: i64, threshold: i64 },
    #[error("question resolves in {days_remaining} days, and the minimum is {threshold}")]
    ResolvesTooSoon { days_remaining: i64, threshold: i64 },
    #[error("question resolves in {days_remaining} days, and the maximum is {threshold}")]
    ResolvesTooLate { days_remaining: i64, threshold: i64 },
    #[error("question opened {age_days} days ago, and the maximum is {threshold}")]
    TooOld { age_days: i64, threshold: i64 },
    #[error("The orderbook has bids at {yes_bid}, asks at {yes_ask}, and the maximum confidence is {threshold}")]
    TooExtreme { yes_bid: i64, yes_ask: i64, threshold: i64 },
    #[error("question has already resolved")]
    Resolved,
    #[error("question is banned in config")]
    Banned,
}