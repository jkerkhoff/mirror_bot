use anyhow::Context;
use chrono::{Duration, Utc};
use log::{debug, error, info};
use regex::Regex;
use reqwest::blocking::Client;
use thiserror::Error;

use crate::{
    db::{self, MirrorRow},
    kalshi::{self, KalshiMarket},
    log_if_err,
    manifold::{self, CreateMarketArgs, ManifoldMarket},
    metaculus::{self, MetaculusQuestion},
    settings::Settings,
    types::{BinaryResolution, Question, QuestionSource},
};

// TODO: migrate from anyhow to this where it makes sense
#[derive(Error, Debug)]
pub enum MirrorError {
    #[error("Question has already been mirrored at {}", .0.manifold_url)]
    AlreadyMirrored(MirrorRow),
    #[error(transparent)]
    KalshiError(#[from] kalshi::KalshiError),
    #[error(transparent)]
    ManifoldError(#[from] manifold::ManifoldError),
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

/// Attempt to mirror a question to Manifold.
/// Will fail if bot already mirrored the question, but does no other checks.
pub fn mirror_question(
    client: &Client,
    db: &rusqlite::Connection,
    question: &Question,
    config: &Settings,
) -> Result<MirrorRow, MirrorError> {
    info!(
        "Mirroring \"{}\" (id: {}) from {}",
        question.question, question.source_id, question.source
    );
    if let Some(mirror) = db::get_mirror_by_source_id(&db, &question.source, &question.source_id)? {
        return Err(MirrorError::AlreadyMirrored(mirror));
    }
    let market = manifold::create_market(
        client,
        CreateMarketArgs::from_question(config, question),
        config,
    )?;
    Ok(db::insert_mirror(db, &market, &question, config)?)
}

/// Attempt to mirror a Kalshi question.
/// Does not check configurable question requirements.
/// Will error if given a multimarket.
pub fn mirror_kalshi_question(
    client: &Client,
    db: &rusqlite::Connection,
    config: &Settings,
    kalshi_market: &KalshiMarket,
) -> Result<MirrorRow, MirrorError> {
    debug!(
        "Attempting to mirror kalshi question with id {} (\"{}\")",
        kalshi_market.id(),
        kalshi_market.title()
    );
    let question: Question = kalshi_market
        .try_into()
        .with_context(|| "failed to convert Kalshi question to common format")?;
    Ok(mirror_question(client, db, &question, config)?)
}

/// Attempt to mirror a metaculus question.
/// Does not check configurable question requirements.
pub fn mirror_metaculus_question(
    client: &Client,
    db: &rusqlite::Connection,
    config: &Settings,
    metaculus_question: &MetaculusQuestion,
) -> Result<MirrorRow, MirrorError> {
    debug!(
        "Attempting to mirror metaculus question with id {} (\"{}\")",
        metaculus_question.id, metaculus_question.title
    );
    let metaculus_question =
        if config.metaculus.fetch_criteria && metaculus_question.resolution_criteria.is_none() {
            debug!("fetching criteria");
            metaculus::get_question(client, &metaculus_question.id.to_string(), config)?
        } else {
            metaculus_question.to_owned()
        };
    let question: Question = (&metaculus_question)
        .try_into()
        .with_context(|| "failed to convert Metaculus question to common format")?;
    Ok(mirror_question(client, db, &question, config)?)
}

/// Automatically pick and mirror Kalshi questions based on config.
pub fn auto_mirror_kalshi(
    client: &Client,
    db: &rusqlite::Connection,
    config: &Settings,
    dry_run: bool,
) -> Result<(), MirrorError> {
    // TODO: this should be cleaned up in general
    let existing_clones = db::get_unresolved_mirrors(db, Some(QuestionSource::Kalshi))?;
    let candidates: Vec<KalshiMarket> = kalshi::get_mirror_candidates(client, config)?
        .into_iter()
        .filter(|q| {
            db::get_any_mirror(db, &QuestionSource::Kalshi, &q.id())
                .unwrap() // TODO: handle error?
                .is_none()
        })
        .collect();
    info!(
        "Obtained {} candidates for cloning from Kalshi",
        candidates.len()
    );
    let clone_count_today = existing_clones
        .iter()
        .filter(|m| m.clone_date > Utc::now() - Duration::days(1))
        .count();
    let remaining_budget =
        config.kalshi.max_clones_per_day - clone_count_today.min(config.kalshi.max_clones_per_day); // TODO: might want to write a query for this?
    info!(
        "Cloned {} kalshi questions in last 24 hours. Remaining budget: {}",
        clone_count_today, remaining_budget
    );
    let to_clone_count = remaining_budget.min(candidates.len());
    info!("Attempting to clone top {} candidates", to_clone_count);
    for kalshi_question in candidates.into_iter().take(to_clone_count) {
        if dry_run {
            info!(
                "dry run -> skipping clone of question with id {}, ({}, {})",
                kalshi_question.id(),
                kalshi_question.title(),
                kalshi_question.full_url()
            );
            continue;
        }
        match mirror_kalshi_question(client, db, config, &kalshi_question).with_context(|| {
            format!(
                "failed to mirror question with id {} (\"{}\")",
                kalshi_question.id(),
                kalshi_question.title()
            )
        }) {
            Ok(market) => {
                info!("Created a mirror:\n{:#?}", market);
            }
            Err(e) => error!("{:#}", e),
        }
    }
    Ok(())
}

/// Automatically pick and mirror Metaculus questions based on config.
pub fn auto_mirror_metaculus(
    client: &Client,
    db: &rusqlite::Connection,
    config: &Settings,
    dry_run: bool,
) -> Result<(), MirrorError> {
    // TODO: this should be cleaned up in general
    let existing_clones = db::get_unresolved_mirrors(db, Some(QuestionSource::Metaculus))?;
    let candidates: Vec<MetaculusQuestion> = metaculus::get_mirror_candidates(client, config)?
        .into_iter()
        .filter(|q| {
            db::get_any_mirror(db, &QuestionSource::Metaculus, &q.id.to_string())
                .unwrap() // TODO: handle error?
                .is_none()
        })
        .collect();
    info!(
        "Obtained {} candidates for cloning from Metaculus",
        candidates.len()
    );
    let clone_count_today = existing_clones
        .iter()
        .filter(|m| m.clone_date > Utc::now() - Duration::days(1))
        .count();
    let remaining_budget = config.metaculus.max_clones_per_day
        - clone_count_today.min(config.metaculus.max_clones_per_day); // TODO: might want to write a query for this?
    info!(
        "Cloned {} metaculus questions in last 24 hours. Remaining budget: {}",
        clone_count_today, remaining_budget
    );
    let to_clone_count = remaining_budget.min(candidates.len());
    info!("Attempting to clone top {} candidates", to_clone_count);
    for metaculus_question in candidates.into_iter().take(to_clone_count) {
        if dry_run {
            info!(
                "dry run -> skipping clone of question with id {}, ({}, {})",
                metaculus_question.id,
                metaculus_question.title,
                metaculus_question.full_url()
            );
            continue;
        }
        match mirror_metaculus_question(client, db, config, &metaculus_question).with_context(
            || {
                format!(
                    "failed to mirror question with id {} (\"{}\")",
                    metaculus_question.id, metaculus_question.title
                )
            },
        ) {
            Ok(market) => {
                info!("Created a mirror:\n{:#?}", market);
            }
            Err(e) => error!("{:#}", e),
        }
    }
    Ok(())
}

/// Resolve mirrored market.
fn resolve_mirror(
    client: &Client,
    db: &rusqlite::Connection,
    mirror: &MirrorRow,
    resolution: BinaryResolution,
    config: &Settings,
) -> Result<(), MirrorError> {
    manifold::resolve_market(
        client,
        &mirror.manifold_contract_id,
        resolution.try_into().map_err(anyhow::Error::from)?,
        config,
    )?;
    db::set_mirror_resolved(db, mirror.id, true)?;
    Ok(())
}

/// Check if Kalshi question has resolved and sync resolution to mirror.
fn sync_kalshi_mirror(
    client: &Client,
    db: &rusqlite::Connection,
    mirror: &MirrorRow,
    config: &Settings,
) -> Result<bool, MirrorError> {
    assert!(mirror.source == QuestionSource::Kalshi);
    let kalshi_question = kalshi::get_question(client, &mirror.source_id, config)?;
    if let Some(resolution) = kalshi_question.get_binary_resolution()? {
        info!(
            "Kalshi question \"{}\" (source id: {}) has resolved {:?}. Syncing.",
            mirror.question, mirror.source_id, resolution
        );
        resolve_mirror(client, db, &mirror, resolution, config)?;
        Ok(true)
    } else {
        debug!("Source has not resolved yet");
        Ok(false)
    }
}

/// Check if Metaculus question has resolved and sync resolution to mirror.
fn sync_metaculus_mirror(
    client: &Client,
    db: &rusqlite::Connection,
    mirror: &MirrorRow,
    config: &Settings,
) -> Result<bool, MirrorError> {
    assert!(mirror.source == QuestionSource::Metaculus);
    let metaculus_question = metaculus::get_question(client, &mirror.source_id, config)?;
    if let Some(resolution) = metaculus_question.get_binary_resolution()? {
        info!(
            "Metaculus question \"{}\" (source id: {}) has resolved {:?}. Syncing.",
            mirror.question, mirror.source_id, resolution
        );
        resolve_mirror(client, db, &mirror, resolution, config)?;
        Ok(true)
    } else {
        debug!("Source has not resolved yet");
        Ok(false)
    }
}

/// Check if source resolved and sync resolution to Manifold
pub fn sync_mirror(
    client: &Client,
    db: &rusqlite::Connection,
    mirror: &MirrorRow,
    config: &Settings,
) -> Result<bool, MirrorError> {
    debug!(
        "Syncing resolution for {} question at {}",
        mirror.source, mirror.source_url
    );
    Ok(match mirror.source {
        crate::types::QuestionSource::Metaculus => {
            sync_metaculus_mirror(client, db, &mirror, config)?
        }
        crate::types::QuestionSource::Kalshi => sync_kalshi_mirror(client, db, &mirror, config)?,
        crate::types::QuestionSource::Polymarket => todo!(),
    })
}

/// Resolve any mirrored markets where the source has resolved
pub fn sync_resolutions_to_manifold(
    client: &Client,
    db: &rusqlite::Connection,
    config: &Settings,
    source: Option<QuestionSource>,
) -> Result<(), MirrorError> {
    info!("Syncing resolutions to Manifold (source = {:?})", source);
    for row in db::get_unresolved_mirrors(&db, source)? {
        log_if_err!(sync_mirror(client, db, &row, config).with_context(|| {
            format!(
                "failed to sync resolution for market with row id {}",
                row.id
            )
        }));
    }
    Ok(())
}

/// Ensure database state matches Manifold for mirrored questions
pub fn sync_manifold_to_db(
    client: &Client,
    db: &rusqlite::Connection,
    config: &Settings,
) -> Result<(), MirrorError> {
    info!("Syncing Manifold state to database.");
    for mirror in db::get_mirrors(db)? {
        if let Err(e) = sync_manifold_mirror_to_db(client, db, &mirror, config).with_context(|| {
            format!(
                "failed to sync Manifold market state to db for market with row id {}",
                mirror.id
            )
        }) {
            error!("{:#}", e);
        }
    }
    Ok(())
}

/// Ensure database state matches Manifold for mirror
fn sync_manifold_mirror_to_db(
    client: &Client,
    db: &rusqlite::Connection,
    mirror: &MirrorRow,
    config: &Settings,
) -> Result<(), MirrorError> {
    debug!(
        "Syncing mirror with row id {} (\"{}\") to database.",
        mirror.id, mirror.question
    );
    let manifold_market = manifold::get_market(client, &mirror.manifold_contract_id, config)?;
    if mirror.resolved != manifold_market.is_resolved {
        info!(
            "Updating resolution state ({} -> {}) for mirror with row id {} (\"{}\")",
            mirror.resolved, manifold_market.is_resolved, mirror.id, mirror.question
        );
        db::set_mirror_resolved(db, mirror.id, manifold_market.is_resolved)?;
    }
    Ok(())
}

/// Look for mirrors created by others and sync to db.
pub fn sync_third_party_mirrors(
    client: &Client,
    db: &rusqlite::Connection,
    config: &Settings,
) -> Result<(), MirrorError> {
    info!("Syncing third-party mirrors from Manifold to db");
    let metaculus_link_regex = Regex::new(r"metaculus\.com\/questions\/(\d+\b)").unwrap();
    // TODO: make this a separate config entry?
    for group_id in config.metaculus.add_group_ids.iter() {
        log_if_err!(sync_third_party_metaculus_mirrors_from_group(
            client,
            db,
            config,
            &*group_id,
            &metaculus_link_regex,
        )
        .with_context(|| {
            format!(
                "failed to sync third party Metaculus mirrors from group with id {}",
                group_id
            )
        }));
    }
    Ok(())
}

/// Look for Metaculus mirrors created by others in group and sync to db.
fn sync_third_party_metaculus_mirrors_from_group(
    client: &Client,
    db: &rusqlite::Connection,
    config: &Settings,
    group_id: &str,
    pattern: &Regex,
) -> Result<(), MirrorError> {
    for market in manifold::get_group_markets(client, group_id, config)?
        .iter()
        .filter(|m| !m.is_resolved)
    {
        if db::get_third_party_mirror_by_contract_id(db, &market.id)?.is_some() {
            continue;
        }
        if db::get_mirror_by_contract_id(db, &market.id)?.is_some() {
            continue;
        }
        match manifold::get_market(client, &market.id, config) {
            Ok(market) => {
                let description = market.description.to_string();
                if let Some(caps) = pattern.captures(&description) {
                    let metaculus_question_id = &caps[1];
                    info!(
                        "Found third party mirror for Metaculus question with id {} at {}.",
                        metaculus_question_id,
                        market.url(config)
                    );
                    db::insert_third_party_mirror(
                        db,
                        &(&market).into(), // TODO: ??
                        &QuestionSource::Metaculus,
                        metaculus_question_id,
                        config,
                    )?;
                }
            }
            Err(e) => error!("{:#}", e),
        }
    }
    Ok(())
}
