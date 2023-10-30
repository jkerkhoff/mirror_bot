use anyhow::{bail, Result};
use chrono::{DateTime, Duration, Utc};
use log::{debug, info};
use reqwest::blocking::{Client, Response};
use reqwest::StatusCode;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::settings::{KalshiQuestionRequirements, Settings};
use crate::types::{BinaryResolution, Question, QuestionSource};

fn list_questions(
    client: &Client,
    params: &KalshiListQuestionsParams,
) -> Result<KalshiEventListResponse, KalshiError> {
    debug!(
        "kalshi::list_questions called (page {})",
        params.page_number.unwrap_or(1)
    );
    let resp = client
        .get("https://trading-api.kalshi.com/v1/events/")
        .query(&params)
        .send()?;
    parse_response(resp)
}

pub fn get_question(
    client: &Client,
    input_ticker: &str,
    _config: &Settings,
) -> Result<KalshiMarket, KalshiError> {
    // As input validation, ensure only alphanumeric and "-" and "." are used
    if !input_ticker
        .chars()
        .all(|c| c.is_alphanumeric() || c == '-' || c == '.')
    {
        return Err(KalshiError::IllegalTickerCharacters(
            input_ticker.to_string(),
        ));
    }
    // The Kalshi api requires the ticker to be uppercase, like it's given in
    // the JSON. Their URLs use lowercase by default, so user input is likely
    // to need the uppercase conversion.
    let uppercase_ticker = input_ticker.to_uppercase();
    let resp = client
        .get(format!(
            "https://trading-api.kalshi.com/v1/events/{}/",
            uppercase_ticker
        ))
        .send()?;
    let resp: KalshiEventResponse = parse_response(resp)?;
    return (&resp.event).try_into();
}

pub fn get_mirror_candidates(client: &Client, config: &Settings) -> Result<Vec<KalshiMarket>> {
    info!("Fetching mirror candidates from Kalshi");
    let requirements = &config.kalshi.auto_filter;
    let mut params = KalshiListQuestionsParams {
        single_event_per_series: Some(requirements.single_event_per_series),
        page_size: Some(200),
        page_number: Some(1),
        ..Default::default()
    };
    if requirements.require_open {
        params.status = Some("open".to_string()); // TODO: use enum?
    }
    let mut events = Vec::new();
    loop {
        let resp = list_questions(client, &params)?;
        // single_event_per_series, and perhaps other filtering parameters, are
        // applied after the server limits to page_size, such that fewer events
        // than page_size may be returned. Strictly speaking, checking for len()
        // == 0 is not sufficient to know there are no more events on later
        // pages, but it's a good enough heuristic.
        if resp.events.len() == 0 {
            break;
        }
        events.extend(resp.events.into_iter());
        *params.page_number.as_mut().unwrap() += 1;
    }
    info!("{} events listed via Kalshi API", events.len());
    let markets = events
        .into_iter()
        .map(|event| (&event).try_into())
        .filter_map(Result::ok)
        .filter(|q| check_market_requirements(q, requirements).is_ok())
        .collect::<Vec<KalshiMarket>>();

    Ok(markets)
}

pub fn check_market_requirements(
    market: &KalshiMarket,
    requirements: &KalshiQuestionRequirements,
) -> Result<(), KalshiCheckFailure> {
    // config requirements
    if requirements.require_open && !market.is_active() {
        return Err(KalshiCheckFailure::NotActive);
    }
    if requirements.exclude_resolved && market.is_resolved() {
        return Err(KalshiCheckFailure::Resolved);
    }
    // Min liquidity
    if market.liquidity < requirements.min_liquidity {
        return Err(KalshiCheckFailure::NotEnoughLiquidity {
            liquidity: market.liquidity,
            threshold: requirements.min_liquidity,
        });
    }
    // Min volume
    if market.volume < requirements.min_volume {
        return Err(KalshiCheckFailure::NotEnoughVolume {
            volume: market.volume,
            threshold: requirements.min_volume,
        });
    }
    // Min recent volume
    if market.recent_volume < requirements.min_recent_volume {
        return Err(KalshiCheckFailure::NotEnoughRecentVolume {
            recent_volume: market.recent_volume,
            threshold: requirements.min_recent_volume,
        });
    }
    // Min open interest
    if market.open_interest < requirements.min_open_interest {
        return Err(KalshiCheckFailure::NotEnoughOpenInterest {
            open_interest: market.open_interest,
            threshold: requirements.min_open_interest,
        });
    }
    // min dollar volume
    if market.dollar_volume < requirements.min_dollar_volume {
        return Err(KalshiCheckFailure::NotEnoughDollarVolume {
            dollar_volume: market.dollar_volume,
            threshold: requirements.min_dollar_volume,
        });
    }
    // min dollar recent volume
    if market.dollar_recent_volume < requirements.min_dollar_recent_volume {
        return Err(KalshiCheckFailure::NotEnoughDollarRecentVolume {
            dollar_recent_volume: market.dollar_recent_volume,
            threshold: requirements.min_dollar_recent_volume,
        });
    }
    // min dollar open interest
    if market.dollar_open_interest < requirements.min_dollar_open_interest {
        return Err(KalshiCheckFailure::NotEnoughDollarOpenInterest {
            dollar_open_interest: market.dollar_open_interest,
            threshold: requirements.min_dollar_open_interest,
        });
    }

    if market.time_to_resolution() < Duration::days(requirements.min_days_to_resolution) {
        return Err(KalshiCheckFailure::ResolvesTooSoon {
            days_remaining: market.time_to_resolution().num_days(),
            threshold: requirements.min_days_to_resolution,
        });
    }
    if market.time_to_resolution() > Duration::days(requirements.max_days_to_resolution) {
        return Err(KalshiCheckFailure::ResolvesTooLate {
            days_remaining: market.time_to_resolution().num_days(),
            threshold: requirements.max_days_to_resolution,
        });
    }
    if market.age() > Duration::days(requirements.max_age_days) {
        return Err(KalshiCheckFailure::TooOld {
            age_days: market.age().num_days(),
            threshold: requirements.max_age_days,
        });
    }
    if (100 - market.yes_ask) as f64 > requirements.max_confidence * 100.0
        || market.yes_bid as f64 > requirements.max_confidence * 100.0
    {
        return Err(KalshiCheckFailure::TooExtreme {
            yes_ask: market.yes_ask,
            yes_bid: market.yes_bid,
            threshold: requirements.max_confidence,
        });
    }
    if requirements.exclude_ids.contains(market.id()) {
        return Err(KalshiCheckFailure::Banned);
    }

    Ok(())
}

/// helper function for parsing both success and error responses
fn parse_response<T: DeserializeOwned>(resp: Response) -> Result<T, KalshiError> {
    if resp.status().is_success() {
        let body = resp
            .text()
            .map_err(|_| KalshiError::UnexpectedResponseType)?;
        match serde_json::from_str(&body) {
            Ok(r) => Ok(r),
            Err(e) => {
                print!("Response: {}", body);
                println!("Error parsing response from Kalshi: {}", e);
                Err(KalshiError::UnexpectedResponseType)
            }
        }
    } else {
        let status = resp.status();
        let error_resp: KalshiErrorResponse = resp
            .json()
            .map_err(|_| KalshiError::UnexpectedErrorType(status))?;
        Err(KalshiError::ErrorResponse(status, error_resp))
    }
}

impl KalshiMarket {
    pub fn id(&self) -> &str {
        &self.ticker_name
    }

    pub fn age(&self) -> Duration {
        Utc::now() - self.open_date
    }

    pub fn is_resolved(&self) -> bool {
        self.status == Status::Finalized
    }

    pub fn is_active(&self) -> bool {
        self.status == Status::Active
    }

    pub fn time_to_resolution(&self) -> Duration {
        self.expiration_date - Utc::now()
    }

    pub fn full_url(&self) -> String {
        // TODO: grab base from config (consistent with manifold)?
        format!(
            "https://kalshi.com/markets/{}#{}",
            self.series_ticker,
            self.id()
        )
    }

    pub fn title(&self) -> String {
        self.title.clone()
    }

    pub fn get_criteria_and_sources(&self) -> String {
        format!(
            "{}{}",
            self.format_underlying_rulebook_variables(),
            self.get_resolution_sources_markdown()
        )
    }

    pub fn format_underlying_rulebook_variables(&self) -> String {
        let mut return_string = self.underlying.clone();
        // For each variable in market.rulebook_variables, substitute ||variable|| with the value of the variable
        let rulebook = self.rulebook_variables.clone();
        // For each key in rulebook
        for key in rulebook.as_object().unwrap().keys() {
            // Remove the surrounding "" marks
            let mut replacement_value = rulebook[key].to_string();
            replacement_value = replacement_value[1..replacement_value.len() - 1].to_string();

            // We've seen instances with and without spaces
            return_string = return_string.replace(&format!("||{}||", key), &replacement_value);
            return_string = return_string.replace(&format!("|| {} ||", key), &replacement_value);
        }
        return return_string;
    }

    pub fn get_resolution_sources_markdown(&self) -> String {
        let sources = self
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

    pub fn get_binary_resolution(&self) -> Result<Option<BinaryResolution>> {
        if self.is_resolved() {
            match self.result {
                Some(KalshiResult::Yes) => Ok(Some(BinaryResolution::Yes)),
                Some(KalshiResult::No) => Ok(Some(BinaryResolution::No)),
                Some(KalshiResult::StillOpen) => {
                    bail!("Kalshi market is resolved but has no result")
                }
                None => bail!("Kalshi market is resolved but with an unexpected result"),
            }
        } else {
            Ok(None)
        }
    }
}

impl TryInto<KalshiMarket> for &Event {
    type Error = KalshiError;

    fn try_into(self) -> Result<KalshiMarket, KalshiError> {
        // We're only supporting single market Kalshi events at this time, and
        // assuming the try_into can only fail in this way
        if self.markets.len() != 1 {
            return Err(KalshiError::OnlySingleMarketsSupported(self.markets.len()));
        }
        let mut market = self.markets[0].clone();
        market.series_ticker = self.series_ticker.clone();
        market.underlying = self.underlying.clone();
        market.settlement_sources = self.settlement_sources.clone();
        return Ok(market);
    }
}

impl TryInto<Question> for &KalshiMarket {
    type Error = anyhow::Error;

    fn try_into(self) -> Result<Question> {
        Ok(Question {
            source: QuestionSource::Kalshi,
            source_url: self.full_url(),
            source_id: self.id().to_string(),
            question: self.title.clone(),
            criteria: Some(self.get_criteria_and_sources()),
            end_date: self.expiration_date,
        })
    }
}

#[derive(Deserialize, Debug, Clone)]
pub struct KalshiEventResponse {
    pub event: Event,
}

#[derive(Deserialize, Debug, Clone)]
pub struct KalshiEventListResponse {
    pub events: Vec<Event>,
}

#[derive(Deserialize, Debug, Clone)]
pub struct Event {
    pub series_ticker: String,
    pub ticker: String,
    pub markets: Vec<KalshiMarket>,
    pub settlement_sources: Vec<SettlementSource>,
    pub underlying: String,
}

#[derive(Deserialize, Debug, Clone)]
pub struct SettlementSource {
    pub name: String,
    pub url: String,
}

#[derive(Deserialize, Debug, Clone)]
pub struct KalshiMarket {
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
    #[serde(skip)]
    pub series_ticker: String,
    #[serde(skip)]
    pub underlying: String,
    #[serde(skip)]
    pub settlement_sources: Vec<SettlementSource>,
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
    Yes,
    No,
    #[serde(rename = "")]
    StillOpen,
}

#[derive(Serialize, Debug, Default)]
pub struct KalshiListQuestionsParams {
    pub status: Option<String>,
    pub single_event_per_series: Option<bool>,
    pub page_size: Option<i64>,
    pub page_number: Option<i64>,
}

#[derive(Error, Debug)]
pub enum KalshiCheckFailure {
    #[error("question is not active")]
    NotActive,
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
    #[error(
        "question has {dollar_recent_volume} dollar_recent_volume, and the minimum is {threshold}"
    )]
    NotEnoughDollarRecentVolume {
        dollar_recent_volume: i64,
        threshold: i64,
    },
    #[error(
        "question has {dollar_open_interest} dollar_open_interest, and the minimum is {threshold}"
    )]
    NotEnoughDollarOpenInterest {
        dollar_open_interest: i64,
        threshold: i64,
    },
    #[error("question resolves in {days_remaining} days, and the minimum is {threshold}")]
    ResolvesTooSoon { days_remaining: i64, threshold: i64 },
    #[error("question resolves in {days_remaining} days, and the maximum is {threshold}")]
    ResolvesTooLate { days_remaining: i64, threshold: i64 },
    #[error("question opened {age_days} days ago, and the maximum is {threshold}")]
    TooOld { age_days: i64, threshold: i64 },
    #[error("The orderbook has bids at {yes_bid}, asks at {yes_ask}, and the maximum confidence is {threshold}")]
    TooExtreme {
        yes_bid: i64,
        yes_ask: i64,
        threshold: f64,
    },
    #[error("question has already resolved")]
    Resolved,
    #[error("question is banned in config")]
    Banned,
}

#[derive(Error, Debug)]
pub enum KalshiError {
    #[error("failed to parse error response from Kalshi (status code: {})", .0)]
    UnexpectedErrorType(StatusCode),
    #[error("failed to parse success response from Kalshi")]
    UnexpectedResponseType,
    // TODO: split out concrete errors
    #[error("error response ({}) from Kalshi: {}", .0, .1.message)]
    ErrorResponse(StatusCode, KalshiErrorResponse),
    #[error("Only events with exactly one market are currently supported ({} found)", .0)]
    OnlySingleMarketsSupported(usize),
    #[error(transparent)]
    ReqwestError(#[from] reqwest::Error),
    #[error("Only alphanumeric, \"-\", and \".\" are allowed in ticker names (\"{}\" given)", .0)]
    IllegalTickerCharacters(String),
    // #[error(transparent)]
    // Other(#[from] anyhow::Error),
}

#[derive(Debug)]
pub struct KalshiErrorResponse {
    code: KalshiErrorCode,
    message: String,
    service: String,
}

#[derive(Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum KalshiErrorCode {
    NotFound,
    Unknown,
}

impl Default for KalshiErrorCode {
    fn default() -> Self {
        Self::Unknown
    }
}

impl<'de> Deserialize<'de> for KalshiErrorResponse {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct Outer {
            error: Inner,
        }
        #[derive(Deserialize)]
        struct Inner {
            // TODO: parse unknown error code
            // see https://github.com/serde-rs/serde/issues/912
            #[serde(default)]
            code: KalshiErrorCode,
            message: String,
            service: String,
        }
        let tmp = Outer::deserialize(deserializer)?.error;
        Ok(KalshiErrorResponse {
            code: tmp.code,
            message: tmp.message,
            service: tmp.service,
        })
    }
}
