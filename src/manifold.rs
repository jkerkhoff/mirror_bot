use std::fmt::{Debug, Display};

use anyhow::Result;
use chrono::{DateTime, Duration, Utc};
use log::{debug, warn};
use reqwest::{
    blocking::{Client, RequestBuilder, Response},
    header::AUTHORIZATION,
    StatusCode, Url,
};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::value::Value as JsonValue;
use thiserror::Error;

use crate::{
    settings::Settings,
    types::Question,
    types::{BinaryResolution, QuestionSource},
};

// TODO: migrate from anyhow to this where it makes sense
#[derive(Error, Debug)]
pub enum ManifoldError {
    #[error("failed to parse error response from Manifold")]
    UnexpectedErrorType(StatusCode),
    #[error("failed to parse success response from Manifold")]
    UnexpectedResponseType,
    // TODO: split out concrete errors
    #[error("error response ({}) from Manifold: {}", .0, .1.message)]
    ErrorResponse(StatusCode, ManifoldErrorResponse),
    #[error(transparent)]
    ReqwestError(#[from] reqwest::Error),
    // #[error(transparent)]
    // Other(#[from] anyhow::Error),
}

/// Create a new market on Manifold.
/// Currently only supports simple binary markets.
pub fn create_market(
    client: &Client,
    market: CreateMarketArgs,
    config: &Settings,
) -> Result<LiteMarket, ManifoldError> {
    debug!("create_market called with market = {:#?}", market);
    // debug!(
    //     "to be serialized as {}",
    //     serde_json::to_string(&market).map_err(anyhow::Error::from)?
    // );
    let endpoint = get_api_url(config).join("market/").unwrap();
    let resp = add_auth(client.post(endpoint), config)
        .json(&market)
        .send()?;
    parse_response(resp)
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateMarketArgs {
    /// Type of market to create.
    /// Currently only simple binary markets are supported.
    pub outcome_type: ManifoldOutcomeType,
    /// Market title. Max 120 characters.
    pub question: String,
    pub description_markdown: String,
    #[serde(with = "chrono::serde::ts_milliseconds")]
    pub close_time: DateTime<Utc>,
    /// Starting probability as integer percentage (1-99)
    pub initial_prob: u32,
    /// ids of groups/topics to add to market on creation
    pub group_ids: Vec<String>,
}

/// Resolve an existing market.
/// Currently only supports simple binary markets.
pub fn resolve_market(
    client: &Client,
    market_id: &str,
    resolution: ManifoldResolution,
    config: &Settings,
) -> Result<LiteMarket, ManifoldError> {
    debug!(
        "resolve_market called with market_id = {}, resolution = {:?}",
        market_id, resolution
    );
    // debug!(
    //     "to be serialized as {}",
    //     serde_json::to_string(&resolution)?
    // );
    let endpoint = get_api_url(config)
        .join(&format!("market/{}/resolve/", market_id))
        .expect("endpoint URL should be a valid URL");
    let resp = add_auth(client.post(endpoint), config)
        .json(&resolution)
        .send()?;
    parse_response(resp)
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ManifoldResolution {
    pub outcome: ManifoldOutcome,
    /// For Mkt resolution, integer percentage to resolve to (1-99)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub probability_int: Option<u32>,
}

/// Fetch market info by contract id
pub fn get_market(
    client: &Client,
    market_id: &str,
    config: &Settings,
) -> Result<FullMarket, ManifoldError> {
    debug!("get_market called with market_id = {}", market_id);
    let endpoint = get_api_url(config)
        .join(&format!("market/{}/", market_id))
        .expect("endpoint URL should be a valid URL");
    let resp = add_auth(client.get(endpoint), config).send()?;
    parse_response(resp)
}

/// Fetch market info by contract slug
pub fn get_market_by_slug(
    client: &Client,
    slug: &str,
    config: &Settings,
) -> Result<FullMarket, ManifoldError> {
    debug!("get_market_by_slug called with slug = {}", slug);
    let endpoint = get_api_url(config)
        .join(&format!("slug/{}/", slug))
        .expect("endpoint URL should be a valid URL");
    let resp = add_auth(client.get(endpoint), config).send()?;
    parse_response(resp)
}

/// Fetch all markets in a group/topic
pub fn get_group_markets(
    client: &Client,
    group_id: &str,
    config: &Settings,
) -> Result<Vec<LiteMarket>, ManifoldError> {
    debug!("get_group_markets called with group_id = {}", group_id);
    let endpoint = get_api_url(config)
        .join(&format!("group/by-id/{}/markets/", group_id))
        .expect("endpoint URL should be a valid URL");
    let resp = add_auth(client.get(endpoint), config).send()?;
    parse_response(resp)
}

/// Fetch managrams, reverse-chronological, manual pagination
pub fn get_managrams(
    client: &Client,
    args: &GetManagramsArgs,
    config: &Settings,
) -> Result<Vec<Managram>, ManifoldError> {
    debug!("get_managrams called with args = {:?}", args);
    let endpoint = get_api_url(config)
        .join("managrams/")
        .expect("endpoint URL should be a valid URL");
    let resp = add_auth(client.get(endpoint), config).query(args).send()?;
    parse_response(resp)
}

/// Same as [`get_managrams`], but handles pagination
pub fn get_managrams_depaginated(
    client: &Client,
    mut args: GetManagramsArgs,
    config: &Settings,
) -> Result<Vec<Managram>, ManifoldError> {
    debug!("get_managrams_depaginated called with args = {:?}", args);
    let mut managrams = Vec::new();
    loop {
        let mut batch = get_managrams(client, &args, config)?;
        let batch_size = batch.len();
        debug!("get_managrams returned {} items", batch_size);
        managrams.append(&mut batch);
        if batch_size < args.limit.unwrap_or(100) {
            break;
        } else {
            args.before = Some(
                managrams
                    .last()
                    .expect("managrams should never be empty here")
                    .created_time,
            );
        }
    }
    Ok(managrams)
}

#[derive(Debug, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct GetManagramsArgs {
    #[serde(skip_serializing_if = "Option::is_none")] // TODO: check if I need all these skips
    pub to_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from_id: Option<String>,
    /// server side max and default 100
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(with = "chrono::serde::ts_milliseconds_option")]
    pub before: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(with = "chrono::serde::ts_milliseconds_option")]
    pub after: Option<DateTime<Utc>>,
}

/// Send a managram
pub fn send_managram(
    client: &Client,
    config: &Settings,
    args: &SendManagramArgs,
) -> Result<(), ManifoldError> {
    debug!("send_managram called with args={:?}", args);
    let endpoint = get_api_url(config)
        .join("managram/")
        .expect("endpoint URL should be a valid URL");
    let resp = add_auth(client.post(endpoint), config).json(args).send()?;
    let _: JsonValue = parse_response(resp)?;
    Ok(())
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SendManagramArgs {
    /// Amount of mana to send. Min 10.
    pub amount: f64,
    pub to_ids: Vec<String>,
    pub message: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LiteMarket {
    pub id: String,
    pub question: String,
    pub slug: String,
    #[serde(with = "chrono::serde::ts_milliseconds")]
    pub created_time: DateTime<Utc>,
    #[serde(with = "chrono::serde::ts_milliseconds")]
    pub close_time: DateTime<Utc>,
    #[serde(with = "chrono::serde::ts_milliseconds")]
    pub last_updated_time: DateTime<Utc>,
    pub is_resolved: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FullMarket {
    pub id: String,
    pub creator_id: String,
    pub question: String,
    pub slug: String,
    #[serde(with = "chrono::serde::ts_milliseconds")]
    pub created_time: DateTime<Utc>,
    #[serde(with = "chrono::serde::ts_milliseconds")]
    pub close_time: DateTime<Utc>,
    #[serde(with = "chrono::serde::ts_milliseconds")]
    pub last_updated_time: DateTime<Utc>,
    pub is_resolved: bool,
    pub description: JsonValue, // TODO: parse this properly?
    pub text_description: String,
}

impl Into<LiteMarket> for &FullMarket {
    fn into(self) -> LiteMarket {
        LiteMarket {
            id: self.id.clone(),
            question: self.question.clone(),
            slug: self.slug.clone(),
            created_time: self.created_time,
            close_time: self.close_time,
            last_updated_time: self.last_updated_time,
            is_resolved: self.is_resolved,
        }
    }
}

/// Common methods between LiteMarket and FullMarket
pub trait ManifoldMarket {
    fn slug(&self) -> &String;

    fn url(&self, config: &Settings) -> String {
        get_client_url(config)
            .join("market/")
            .unwrap()
            .join(self.slug())
            .expect("market slug should make for a valid url")
            .to_string()
    }
}

impl ManifoldMarket for LiteMarket {
    fn slug(&self) -> &String {
        &self.slug
    }
}

impl ManifoldMarket for FullMarket {
    fn slug(&self) -> &String {
        &self.slug
    }
}

#[derive(Debug)]
pub struct Managram {
    pub id: String,
    /// identifies set of identical managrams sent at once to multiple users
    pub group_id: String,
    pub from_id: String,
    pub to_id: String,
    pub created_time: DateTime<Utc>,
    // currently this is only ever Mana, mainly parsing so code will break loudly if that changes
    pub token: TokenType,
    pub amount: f64,
    pub message: String,
}

impl<'de> Deserialize<'de> for Managram {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Main {
            id: String,
            from_id: String,
            to_id: String,
            #[serde(with = "chrono::serde::ts_milliseconds")]
            pub created_time: DateTime<Utc>,
            token: TokenType,
            amount: f64,
            data: Data,
        }

        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Data {
            group_id: String,
            message: String,
        }

        let tmp = Main::deserialize(deserializer)?;
        Ok(Managram {
            id: tmp.id,
            group_id: tmp.data.group_id,
            from_id: tmp.from_id,
            to_id: tmp.to_id,
            created_time: tmp.created_time,
            token: tmp.token,
            amount: tmp.amount,
            message: tmp.data.message,
        })
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TokenType {
    #[serde(rename = "M$")]
    Mana,
}

impl Display for TokenType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            TokenType::Mana => "Mana",
        })?;
        Ok(())
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum ManifoldOutcome {
    Yes,
    No,
    Mkt,
    Cancel,
}

impl From<BinaryResolution> for ManifoldResolution {
    fn from(value: BinaryResolution) -> Self {
        if let BinaryResolution::Percent(p) = value {
            let probability_int = (p * 100.0).round() as u32;
            ManifoldResolution {
                outcome: ManifoldOutcome::Mkt,
                probability_int: Some(probability_int),
            }
        } else {
            ManifoldResolution {
                outcome: match value {
                    BinaryResolution::Yes => ManifoldOutcome::Yes,
                    BinaryResolution::No => ManifoldOutcome::No,
                    BinaryResolution::Cancel => ManifoldOutcome::Cancel,
                    _ => panic!("unknown outcome type"),
                },
                probability_int: None,
            }
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManifoldErrorResponse {
    message: String,
}

/// Base url for Manifold web client
fn get_client_url(config: &Settings) -> Url {
    Url::parse(&config.manifold.client_url).expect("MANIFOLD_CLIENT_URL should be a valid URL")
}

/// Base url for Manifold api requests
fn get_api_url(config: &Settings) -> Url {
    Url::parse(&config.manifold.api_url).expect("MANIFOLD_API_URL should be a valid URL")
}

fn add_auth(req: RequestBuilder, config: &Settings) -> RequestBuilder {
    req.header(AUTHORIZATION, format!("Key {}", config.manifold.api_key))
}

/// helper function for parsing both success and error responses
fn parse_response<T: DeserializeOwned>(resp: Response) -> Result<T, ManifoldError> {
    if resp.status().is_success() {
        match resp.json() {
            Ok(r) => Ok(r),
            Err(_) => Err(ManifoldError::UnexpectedResponseType), // TODO: wrap inner?
        }
    } else {
        let status = resp.status();
        let error: ManifoldErrorResponse = resp
            .json()
            .map_err(|_| ManifoldError::UnexpectedErrorType(status))?;
        Err(ManifoldError::ErrorResponse(status, error))
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum ManifoldOutcomeType {
    Binary,
}

impl CreateMarketArgs {
    fn title_from_question(question: &Question, config: &Settings) -> String {
        let tmpl = &config.manifold.template;
        let mut title = format!("[{}] {}", question.source, question.question);
        // TODO: factor out truncation function and use it for description as well
        if title.len() > tmpl.max_question_length {
            warn!(
                "Truncating question from {} to {} characters",
                title.len(),
                tmpl.max_question_length
            );
            let suffix_len = tmpl.title_retain_end_characters + 3;
            let to_remove = title.len() + 3 - tmpl.max_question_length;
            let cut_start = tmpl.max_question_length - suffix_len;
            let cut_end = cut_start + to_remove;
            title.replace_range(cut_start..cut_end, "...");
        }
        title
    }

    fn description_from_question(question: &Question, config: &Settings) -> String {
        let tmpl = &config.manifold.template;
        let embed = if let Some(embed_html) = &question.embed_html() {
            format!("\n\n{}", embed_html)
        } else {
            "".to_owned()
        };
        let mut description = format!(
            "### {title}\n\nResolves the same as [the original on {source}]({url}).{embed}\n\n---\n\n",
            title = question.question,
            source = question.source,
            url = question.source_url,
            embed = embed,
        );
        if let Some(criteria) = &question.criteria {
            description.push_str(&format!(
                "**Resolution criteria**\n\n{criteria}\n\n---\n\n",
                criteria = criteria
            ))
        }
        description.push_str(&tmpl.description_footer);
        if description.len() > tmpl.max_description_length {
            warn!(
                "Truncating description from {} to {} characters",
                description.len(),
                tmpl.max_description_length
            );
            description.truncate(tmpl.max_description_length - 3);
            description.push_str("...");
        }
        description
    }

    pub fn group_ids_from_question(question: &Question, config: &Settings) -> Vec<String> {
        let mut group_ids = Vec::new();
        match question.source {
            QuestionSource::Metaculus => {
                group_ids.extend(config.metaculus.add_group_ids.iter().cloned())
            }
            QuestionSource::Kalshi => group_ids.extend(config.kalshi.add_group_ids.iter().cloned()),
            QuestionSource::Polymarket => {
                todo!()
            }
            QuestionSource::Manual => {}
        }
        group_ids
    }

    pub fn from_question(config: &Settings, question: &Question) -> Self {
        Self {
            outcome_type: ManifoldOutcomeType::Binary,
            question: Self::title_from_question(question, config),
            description_markdown: Self::description_from_question(question, config),
            close_time: if question.end_date > Utc::now() {
                question.end_date + Duration::days(1)
            } else {
                warn!("Source question has end date in the past. Setting close date to a week from now.");
                Utc::now() + Duration::weeks(1)
            },
            initial_prob: 50,
            group_ids: Self::group_ids_from_question(question, config),
        }
    }
}
