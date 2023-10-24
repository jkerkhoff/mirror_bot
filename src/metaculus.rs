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

use crate::settings::{MetaculusQuestionRequirements, Settings};
use crate::types::{BinaryResolution, Question, QuestionSource};

fn list_questions(
    client: &Client,
    params: MetaculusListQuestionsParams,
    config: &Settings,
) -> Result<MetaculusQuestionsResponse> {
    debug!("list_questions called"); // (params: {:?})", params);
    Ok(add_auth(
        client.get("https://www.metaculus.com/api2/questions/"),
        config,
    )
    .query(&params)
    .send()?
    .json()?)
}

/// list_questions but depaginated
pub fn get_questions(
    client: &Client,
    params: MetaculusListQuestionsParams,
    config: &Settings,
) -> Result<Vec<MetaculusQuestion>> {
    debug!("get_questions called"); // (params: {:?})", params);
    let mut questions = Vec::new();
    let initial_resp = list_questions(client, params, config)?;
    questions.extend(initial_resp.results.into_iter());
    let mut next = initial_resp.next;
    while let Some(next_url) = next {
        debug!("Fetching metaculus questions (next={})", next_url);
        let resp: MetaculusQuestionsResponse =
            add_auth(client.get(next_url), config).send()?.json()?;
        questions.extend(resp.results.into_iter());
        next = resp.next;
    }
    Ok(questions)
}

pub fn get_question(client: &Client, id: &str, config: &Settings) -> Result<MetaculusQuestion> {
    debug!("get_question called (id: {})", id);
    let id: u64 = id
        .parse()
        .with_context(|| "Metaculus question id should be a positive integer")?;
    Ok(add_auth(
        client.get(format!("https://www.metaculus.com/api2/questions/{}/", id)),
        config,
    )
    .send()?
    .json()?)
}

pub fn get_mirror_candidates(client: &Client, config: &Settings) -> Result<Vec<MetaculusQuestion>> {
    info!("Fetching mirror candidates from Metaculus");
    let requirements = &config.metaculus.auto_filter;
    let mut params = MetaculusListQuestionsParams {
        publish_time_gt: Some(Utc::now() - Duration::days(requirements.max_age_days)),
        resolve_time_gt: Some(Utc::now() + Duration::days(requirements.min_days_to_resolution)),
        resolve_time_lt: Some(Utc::now() + Duration::days(requirements.max_days_to_resolution)),
        r#type: Some(QuestionType::Forecast),
        forecast_type: Some("binary".to_string()), // TODO: use enum?
        unconditional: Some(true),
        order_by: Some("-votes".to_string()),
        limit: Some(100),
        ..Default::default()
    };
    if requirements.require_open {
        params.status = Some("open".to_string()); // TODO: use enum?
    }
    if requirements.exclude_grouped {
        params.has_group = Some(false);
    }
    let questions = get_questions(client, params, config)
        .with_context(|| "failed to fetch questions from metaculus")?
        .into_iter()
        .filter(|q| check_question_requirements(q, requirements).is_ok())
        .collect();
    Ok(questions)
}

pub fn check_question_requirements(
    question: &MetaculusQuestion,
    requirements: &MetaculusQuestionRequirements,
) -> Result<(), MetaculusCheckFailure> {
    // fixed requirements
    if !question.is_binary() {
        return Err(MetaculusCheckFailure::NotBinary);
    }
    if question.is_conditional() {
        return Err(MetaculusCheckFailure::Conditional);
    }
    if !question.is_forecast() {
        return Err(MetaculusCheckFailure::NotForecast);
    }
    // config requirements
    if requirements.require_visible_community_prediction && !question.community_prediction_visible()
    {
        return Err(MetaculusCheckFailure::NoCommunityPrediction);
    }
    if requirements.require_open && question.active_state != ActiveState::Open {
        return Err(MetaculusCheckFailure::NotOpen);
    }
    if requirements.exclude_resolved && question.active_state == ActiveState::Resolved {
        return Err(MetaculusCheckFailure::Resolved);
    }
    if requirements.exclude_grouped && question.is_grouped() {
        return Err(MetaculusCheckFailure::Grouped);
    }
    if let Some(forecasters) = question.number_of_forecasters {
        if forecasters < requirements.min_forecasters {
            return Err(MetaculusCheckFailure::NotEnoughForecasters {
                forecasters: forecasters,
                threshold: requirements.min_forecasters,
            });
        }
    } else {
        warn!(
            "Metaculus question with id {} has a null number_of_forecasters field and will be filtered out",
            question.id
        );
        return Err(MetaculusCheckFailure::NotEnoughForecasters {
            forecasters: -1,
            threshold: requirements.min_forecasters,
        });
    }
    if question.votes < requirements.min_votes {
        return Err(MetaculusCheckFailure::NotEnoughVotes {
            votes: question.votes,
            threshold: requirements.min_votes,
        });
    }
    if question.time_to_resolution() < Duration::days(requirements.min_days_to_resolution) {
        return Err(MetaculusCheckFailure::ResolvesTooSoon {
            days_remaining: question.time_to_resolution().num_days(),
            threshold: requirements.min_days_to_resolution,
        });
    }
    if question.time_to_resolution() > Duration::days(requirements.max_days_to_resolution) {
        return Err(MetaculusCheckFailure::ResolvesTooLate {
            days_remaining: question.time_to_resolution().num_days(),
            threshold: requirements.max_days_to_resolution,
        });
    }
    if let Some(last_active) = question.last_activity_time {
        let days_since_active = (Utc::now() - last_active).num_days();
        if days_since_active > requirements.max_last_active_days {
            return Err(MetaculusCheckFailure::NoRecentActivity {
                days_since_active,
                threshold: requirements.max_last_active_days,
            });
        }
    } else {
        return Err(MetaculusCheckFailure::NoRecentActivity {
            days_since_active: -1,
            threshold: requirements.max_last_active_days,
        });
    }
    if question.age() > Duration::days(requirements.max_age_days) {
        return Err(MetaculusCheckFailure::TooOld {
            age_days: question.age().num_days(),
            threshold: requirements.max_age_days,
        });
    }
    if let Some(p) = question.community_prediction_prob() {
        if p.max(1.0 - p) > requirements.max_confidence {
            return Err(MetaculusCheckFailure::TooExtreme {
                probability: p,
                threshold: requirements.max_confidence,
            });
        }
    }
    if requirements.exclude_ids.contains(&question.id) {
        return Err(MetaculusCheckFailure::Banned);
    }

    Ok(())
}

#[derive(Error, Debug)]
pub enum MetaculusCheckFailure {
    #[error("not a binary question")]
    NotBinary,
    #[error("conditional question")]
    Conditional,
    #[error("question is not a forecast")]
    NotForecast,
    #[error("community prediction still hidden")]
    NoCommunityPrediction,
    #[error("question is not open")]
    NotOpen,
    #[error("question is part of a group")]
    Grouped,
    #[error("question has {forecasters} forecasters, and the minimum is {threshold}")]
    NotEnoughForecasters { forecasters: i64, threshold: i64 },
    #[error("question has {votes} votes, and the minimum is {threshold}")]
    NotEnoughVotes { votes: i64, threshold: i64 },
    #[error("question resolves in {days_remaining} days, and the minimum is {threshold}")]
    ResolvesTooSoon { days_remaining: i64, threshold: i64 },
    #[error("question resolves in {days_remaining} days, and the maximum is {threshold}")]
    ResolvesTooLate { days_remaining: i64, threshold: i64 },
    #[error(
        "question was last active {days_since_active} days ago, and the maximum is {threshold}"
    )]
    NoRecentActivity {
        days_since_active: i64,
        threshold: i64,
    },
    #[error("question published {age_days} days ago, and the maximum is {threshold}")]
    TooOld { age_days: i64, threshold: i64 },
    #[error("community forecast suggests a probability of {probability}, and the maximum confidence is {threshold}")]
    TooExtreme { probability: f64, threshold: f64 },
    #[error("question has already resolved")]
    Resolved,
    #[error("question is banned in config")]
    Banned,
}

#[derive(Deserialize, Debug, PartialEq, Clone)]
pub enum QuestionStatus {
    #[serde(rename = "A")]
    Active,
    #[serde(rename = "T")]
    Draft,
    #[serde(rename = "I")]
    Inactive,
    #[serde(rename = "R")]
    Rejected,
    #[serde(rename = "D")]
    Deleted,
    #[serde(rename = "V")]
    Private,
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Clone)]
#[serde(rename_all = "snake_case")]
pub enum QuestionType {
    Forecast,
    Notebook,
    Discussion,
    Claim,
    Group,
    ConditionalGroup,
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Clone)]
#[serde(rename_all = "snake_case")]
pub enum ForecastType {
    Binary,
    Continuous,
    Unknown,
}

impl Default for ForecastType {
    fn default() -> Self {
        Self::Unknown
    }
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Clone)]
#[serde(rename_all = "UPPERCASE")]
pub enum ActiveState {
    Draft,
    Pending,
    Deferred,
    Unmoderated,
    Upcoming,
    Open,
    Closed,
    Resolved,
    PendingResolution,
}

#[derive(Deserialize, Debug, Clone)]
pub struct PossibilitiesStub {
    #[serde(default)]
    r#type: ForecastType,
}

#[derive(Deserialize, Debug, Clone)]
pub struct CpsFull {
    // q1: Option<f64>,
    q2: Option<f64>,
    // q3: Option<f64>,
}

#[derive(Deserialize, Debug, Clone)]
pub struct CommunityPredictionStub {
    full: Option<CpsFull>,
}

#[derive(Deserialize, Debug, Clone)]
pub struct MetaculusQuestion {
    pub active_state: ActiveState,
    /// API question url
    pub url: String,
    /// Relative question url.
    pub page_url: String,
    pub id: i64,
    pub author: i64,
    pub author_name: String,
    /// Question title. Max 200 characters.
    pub title: String,
    pub title_short: String,
    pub status: QuestionStatus,
    pub resolution: Option<f64>,
    pub publish_time: DateTime<Utc>,
    pub resolve_time: DateTime<Utc>,
    pub possibilities: PossibilitiesStub,
    pub r#type: QuestionType,
    pub edited_time: Option<DateTime<Utc>>,
    pub last_activity_time: Option<DateTime<Utc>>,
    pub votes: i64,
    pub community_prediction: Option<CommunityPredictionStub>,
    pub number_of_forecasters: Option<i64>,
    pub prediction_count: i64,
    pub group: Option<i64>,
    pub condition: Option<JsonValue>,
    /// only present in /questions/[id] response
    pub resolution_criteria: Option<String>,
}

impl MetaculusQuestion {
    pub fn age(&self) -> Duration {
        Utc::now() - self.publish_time
    }

    pub fn time_to_resolution(&self) -> Duration {
        self.resolve_time - Utc::now()
    }

    pub fn community_prediction_visible(&self) -> bool {
        // TODO: probably have to change this to support continuous etc.
        self.community_prediction_prob().is_some()
    }

    pub fn is_forecast(&self) -> bool {
        self.r#type == QuestionType::Forecast
    }

    pub fn is_binary(&self) -> bool {
        self.possibilities.r#type == ForecastType::Binary
    }

    /// Community Prediction, if available
    pub fn community_prediction_prob(&self) -> Option<f64> {
        self.community_prediction
            .as_ref()
            .and_then(|cps| cps.full.as_ref())
            .and_then(|cpsf| cpsf.q2)
    }

    pub fn is_grouped(&self) -> bool {
        self.group.is_some()
    }

    pub fn is_conditional(&self) -> bool {
        self.condition.is_some()
    }

    pub fn full_url(&self) -> String {
        // TODO: grab base from config (consistent with manifold)?
        // Don't think there even is a public dev instance though.
        format!("https://www.metaculus.com{}", self.page_url)
    }

    pub fn is_resolved(&self) -> bool {
        self.active_state == ActiveState::Resolved
    }

    #[allow(illegal_floating_point_literal_pattern)] // TODO: follow the law
    pub fn get_binary_resolution(&self) -> Result<Option<BinaryResolution>> {
        if self.active_state == ActiveState::Resolved {
            if self.possibilities.r#type == ForecastType::Binary {
                match self.resolution {
                    Some(-2.0) => {
                        // Annulled
                        Ok(Some(BinaryResolution::Cancel))
                    }
                    Some(-1.0) => {
                        // Ambiguous
                        Ok(Some(BinaryResolution::Cancel))
                    }
                    Some(0.0) => Ok(Some(BinaryResolution::No)),
                    Some(1.0) => Ok(Some(BinaryResolution::Yes)),
                    Some(resolution) => {
                        if 0.0 <= resolution && resolution <= 1.0 {
                            Ok(Some(BinaryResolution::Percent(resolution)))
                        } else {
                            Err(anyhow!("unexpected resolution value `{:?}`", resolution))
                        }
                    }
                    None => Ok(None),
                }
            } else {
                Err(anyhow!("question type is not binary"))
            }
        } else {
            Ok(None)
        }
    }
}

impl TryInto<Question> for &MetaculusQuestion {
    type Error = anyhow::Error;

    fn try_into(self) -> Result<Question> {
        if self.is_conditional() {
            return Err(anyhow!("conditional questions are not supported yet"));
        }
        if !self.is_binary() {
            return Err(anyhow!("non-binary questions are not supported yet"));
        }
        if !self.is_forecast() {
            return Err(anyhow!(
                "non-forecast questions are not supported, and this one has type {:?}",
                self.r#type
            ));
        }
        Ok(Question {
            source: QuestionSource::Metaculus,
            source_url: self.full_url(),
            source_id: self.id.to_string(),
            question: self.title.clone(),
            criteria: self.resolution_criteria.clone(),
            end_date: self.resolve_time,
        })
    }
}

#[derive(Deserialize, Debug)]
pub struct MetaculusQuestionsResponse {
    pub next: Option<String>,
    pub previous: Option<String>,
    pub results: Vec<MetaculusQuestion>,
}

#[derive(Serialize, Debug, Default)]
pub struct MetaculusListQuestionsParams {
    pub access: Option<String>,
    pub author: Option<i64>,
    pub categories: Option<String>,
    #[serde(rename = "close_time__gt")]
    pub close_time_gt: Option<DateTime<Utc>>,
    #[serde(rename = "close_time__lt")]
    pub close_time_lt: Option<DateTime<Utc>>,
    pub commented_by: Option<i64>,
    pub contest: Option<String>,
    pub forecast_type: Option<String>, // TODO: enum?
    pub group: Option<i64>,
    pub guessed_by: Option<i64>,
    pub has_group: Option<bool>,
    pub include_description: Option<bool>,
    pub limit: Option<i64>,
    pub not_guessed_by: Option<i64>,
    pub offset: Option<i64>,
    pub order_by: Option<String>,
    pub project: Option<String>,
    #[serde(rename = "publish_time__gt")]
    pub publish_time_gt: Option<DateTime<Utc>>,
    #[serde(rename = "publish_time__lt")]
    pub publish_time_lt: Option<DateTime<Utc>>,
    #[serde(rename = "resolve_time__gt")]
    pub resolve_time_gt: Option<DateTime<Utc>>,
    #[serde(rename = "resolve_time__lt")]
    pub resolve_time_lt: Option<DateTime<Utc>>,
    pub reversed_related: Option<i64>,
    pub search: Option<String>,
    pub status: Option<String>,
    pub r#type: Option<QuestionType>,
    pub unconditional: Option<bool>,
    pub upvoted_by: Option<i64>,
    pub username: Option<String>,
    pub visible_from_project: Option<String>,
}

fn add_auth(req: RequestBuilder, config: &Settings) -> RequestBuilder {
    req.header(AUTHORIZATION, format!("Token {}", config.metaculus.api_key))
}
