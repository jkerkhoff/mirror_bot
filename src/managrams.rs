use crate::{
    db::{self, AnyMirror, MirrorRow},
    log_if_err,
    manifold::{self, GetManagramsArgs, Managram, ManifoldError, SendManagramArgs},
    metaculus, mirror,
    settings::Settings,
    types::QuestionSource,
};
use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use log::{debug, info, warn};
use reqwest::{blocking::Client, StatusCode, Url};

/// Fetch managrams from manifold and save to db for processing.
pub fn sync_managrams(client: &Client, db: &rusqlite::Connection, config: &Settings) -> Result<()> {
    info!("Syncing managrams");
    let last_managram_timestamp = db::last_managram_timestamp(db)?;
    for managram in manifold::get_managrams_depaginated(
        client,
        GetManagramsArgs {
            to_id: Some(config.manifold.user_id.to_owned()),
            after: last_managram_timestamp,
            ..Default::default()
        },
        config,
    )? {
        debug!("Inserting managram into db: {:?}", managram);
        db::insert_managram(db, &managram)?;
    }

    Ok(())
}

/// Fetch unprocessed managrams from db and process them.
pub fn process_managrams(
    client: &Client,
    db: &rusqlite::Connection,
    config: &Settings,
) -> Result<()> {
    for managram in db::get_unprocessed_managrams(db)? {
        log_if_err!(
            process_managram(client, db, config, &managram).with_context(|| format!(
                "while processing managram (id: {}, user_id: {})",
                managram.id, managram.from_id
            ))
        );
    }
    Ok(())
}

/// Process an unprocessed managram. Does not check processed state.
fn process_managram(
    client: &Client,
    db: &rusqlite::Connection,
    config: &Settings,
    managram: &Managram,
) -> Result<()> {
    debug!("Processing managram with txn_id {}", managram.id);
    let result = process_managram_command(client, db, config, managram);
    match result {
        Ok(()) => {
            db::set_managram_processed(db, &managram.id, true)?;
        }
        Err(ManagramProcessingError::UserFacing(msg)) => {
            warn!(
                "Command from managram with id {} failed (message: {}). Refunding.",
                managram.id, msg
            );
            // Mark processed before refunding so we don't keep sending the refund if we get an error response.
            // TODO: encode failure state in db somehow
            // maybe instead of "processed", have a state that can be new/complete/started/failed
            db::set_managram_processed(db, &managram.id, true)?;
            respond_to_managram(client, config, managram, ResponseAmount::Refund, msg)?;
        }
        Err(ManagramProcessingError::Internal(e)) => {
            // TODO: append error instead of failing silently
            db::set_managram_processed(db, &managram.id, true).ok();
            return Err(e);
        }
    }
    Ok(())
}

enum ManagramProcessingError {
    /// Errors expected during normal operation. These should lead to an error response for the user.
    UserFacing(String),
    /// Errors that indicate something went wrong in a way that leaves us in an unclear state.
    /// Fail silently from user perspective, fail loudly in logs.
    Internal(anyhow::Error),
}

/// Try to parse a command from a managram and execute it.
fn process_managram_command(
    client: &Client,
    db: &rusqlite::Connection,
    config: &Settings,
    managram: &Managram,
) -> Result<(), ManagramProcessingError> {
    // clap expects args in the form of a list of strings, since normally the shell
    // handles tokenization etc. For now this just splits on whitespace. If we want
    // quoted arguments in the future we'll have to do something fancier than this.
    let args = ManagramArgs::try_parse_from(managram.message.split_whitespace())
        .map_err(|e| ManagramProcessingError::UserFacing(e.to_string()))?;
    match args.command {
        ManagramCommands::Mirror(args) => {
            process_managram_mirror_command(client, db, config, managram, args)
        }
        ManagramCommands::Resolve(args) => {
            process_managram_resolve_command(client, db, config, managram, args)
        }
        ManagramCommands::Ping => {
            info!(
                "Managram ping received (id: {}, user id: {})",
                managram.id, managram.from_id
            );
            respond_to_managram(client, config, managram, ResponseAmount::Refund, "Pong!")
                .map_err(|e| ManagramProcessingError::Internal(e))?;
            db::set_managram_processed(db, &managram.id, true)
                .map_err(|e| ManagramProcessingError::Internal(e))
        }
        ManagramCommands::None(_) => {
            info!(
                "Managram with id {} from {} does not contain a known command. Ignoring.",
                managram.id, managram.from_id
            );
            db::set_managram_processed(db, &managram.id, true)
                .map_err(|e| ManagramProcessingError::Internal(e))
        }
    }
}

fn process_managram_resolve_command(
    client: &Client,
    db: &rusqlite::Connection,
    config: &Settings,
    managram: &Managram,
    ResolveArgs { target }: ResolveArgs,
) -> Result<(), ManagramProcessingError> {
    info!(
        "Processing managram resolve command. \
        Managram id: {}. From id: {}. Target: {:?}.",
        managram.id, managram.from_id, target
    );
    let cfg = &config.manifold.managrams;
    let required_amount = cfg.resolve_cost + cfg.min_amount;
    if managram.amount < required_amount {
        return Err(ManagramProcessingError::UserFacing(format!(
            "Resolve requests should include at least {} mana.",
            required_amount
        )));
    }
    let market_id = match target {
        MarketIdentifier::Id(id) => id,
        MarketIdentifier::Slug(slug) => match manifold::get_market_by_slug(client, &slug, config) {
            Ok(market) => {
                if market.author_id != config.manifold.user_id {
                    return Err(ManagramProcessingError::UserFacing(
                        "Market was not created by this bot".to_string(),
                    ));
                }
                if market.is_resolved {
                    return Err(ManagramProcessingError::UserFacing(
                        "Market is already resolved".to_string(),
                    ));
                }
                market.id
            }
            Err(ManifoldError::ErrorResponse(StatusCode::NOT_FOUND, _)) => {
                return Err(ManagramProcessingError::UserFacing(
                    "Market not found".to_string(),
                ))
            }
            Err(error) => return Err(ManagramProcessingError::Internal(error.into())),
        },
    };
    let market_row = match db::get_mirror_by_contract_id(db, &market_id) {
        Ok(Some(market)) => market,
        Ok(None) => {
            return Err(ManagramProcessingError::UserFacing(
                "Market not in bot database".to_string(),
            ))
        }
        Err(error) => return Err(ManagramProcessingError::Internal(error.into())),
    };
    let resolved = match mirror::sync_mirror(client, db, &market_row, config) {
        Ok(resolved) => resolved,
        Err(error) => return Err(ManagramProcessingError::Internal(error.into())),
    };
    let response = if resolved {
        "Resolved market!"
    } else {
        "Source question has not resolved yet"
    };
    respond_to_managram(client, config, managram, ResponseAmount::Refund, response)
        .map_err(|e| ManagramProcessingError::Internal(e))?;
    Ok(())
}

fn process_managram_mirror_command(
    client: &Client,
    db: &rusqlite::Connection,
    config: &Settings,
    managram: &Managram,
    MirrorArgs {
        target: MirrorTarget { source, source_id },
        force,
    }: MirrorArgs,
) -> Result<(), ManagramProcessingError> {
    info!(
        "Processing managram mirror command. \
        Managram id: {}. From id: {}. Question source: {}. Question id: {}. Force: {}.",
        managram.id, managram.from_id, source, source_id, force
    );
    let cfg = &config.manifold.managrams;
    let required_amount = cfg.mirror_cost + cfg.min_amount;
    if managram.amount < required_amount {
        return Err(ManagramProcessingError::UserFacing(format!(
            "Mirror requests should include at least {} mana.",
            required_amount
        )));
    }
    // TODO: we need to ensure we actually find a mirror if it exists.
    // I could see this going wrong with Kalshi (case insensitive id input).
    match db::get_any_mirror(db, &source, &source_id)
        .map_err(|e| ManagramProcessingError::Internal(e))?
    {
        Some(AnyMirror::Mirror(mirror)) => {
            return Err(ManagramProcessingError::UserFacing(format!(
                "Mirror already exists: {}",
                mirror.manifold_url,
            )));
        }
        Some(AnyMirror::ThirdPartyMirror(mirror)) => {
            if force {
                warn!("Ignoring third party mirror due to force flag.");
            } else {
                return Err(ManagramProcessingError::UserFacing(format!(
                    "Found an existing mirror from a different user at {}. \
                    Append --force to your request to create a new mirror anyway.",
                    mirror.manifold_url,
                )));
            }
        }
        None => {}
    }
    let mirror = match source {
        QuestionSource::Metaculus => {
            process_managram_mirror_metaculus(client, db, config, managram, &source_id)?
        }
        QuestionSource::Kalshi => todo!(),
        QuestionSource::Polymarket => todo!(),
    };
    db::set_managram_processed(db, &managram.id, true)
        .map_err(|e| ManagramProcessingError::Internal(e))?;
    respond_to_managram(
        client,
        config,
        managram,
        ResponseAmount::Minimum,
        format!("Created mirror at {}", mirror.manifold_url),
    )
    .map_err(|e| ManagramProcessingError::Internal(e))?;
    Ok(())
}

fn process_managram_mirror_metaculus(
    client: &Client,
    db: &rusqlite::Connection,
    config: &Settings,
    managram: &Managram,
    source_id: &str,
) -> Result<MirrorRow, ManagramProcessingError> {
    debug!("Metaculus mirror request.");
    let question = metaculus::get_question(client, source_id, config).map_err(|_| {
        ManagramProcessingError::UserFacing(format!(
            "Failed to fetch question with id {} from Metaculus.",
            source_id
        ))
    })?;
    metaculus::check_question_requirements(&question, &config.metaculus.request_filter)
        .map_err(|e| ManagramProcessingError::UserFacing(e.to_string()))?;
    info!(
        "Checks passed. Mirroring metaculus question with id {} (\"{}\") at user request. Managram id: {}. User id: {}",
        question.id, question.title, managram.id, managram.from_id
    );
    match mirror::mirror_metaculus_question(client, db, config, &question) {
        Ok(mirror) => Ok(mirror),
        // TODO: maybe split out some cases where we can safely respond
        Err(e) => Err(ManagramProcessingError::Internal(e.into())),
    }
}

fn respond_to_managram<M: Into<String>>(
    client: &Client,
    config: &Settings,
    managram: &Managram,
    amount: ResponseAmount,
    message: M,
) -> Result<()> {
    let amount = match amount {
        ResponseAmount::Refund => managram.amount,
        ResponseAmount::Minimum => config.manifold.managrams.min_amount,
        ResponseAmount::Amount(amount) => amount,
    };
    manifold::send_managram(
        client,
        config,
        &SendManagramArgs {
            amount,
            to_ids: vec![managram.from_id.clone()],
            message: message.into(),
        },
    )?;
    info!(
        "Responded to managram with id {} from user with id {}. Request amount: {}. Response amount: {}.",
        managram.id, managram.from_id, managram.amount, amount
    );
    Ok(())
}

#[derive(Debug)]
enum ResponseAmount {
    Refund,
    Minimum,
    Amount(f64),
}

#[derive(Debug, Parser)]
#[command(disable_help_flag(true))]
#[command(no_binary_name(true))]
struct ManagramArgs {
    #[command(subcommand)]
    pub command: ManagramCommands,
}

#[derive(Debug, Subcommand)]
enum ManagramCommands {
    /// Request a mirror for a specific question
    Mirror(MirrorArgs),
    /// Request resolution for a mirror of resolved source
    Resolve(ResolveArgs),
    /// Responds "Pong!", for testing purposes
    Ping,
    /// Anything else
    #[command(external_subcommand)]
    None(Vec<String>),
}

#[derive(Debug, Parser)]
struct ResolveArgs {
    /// Market to resolve (url)
    #[arg(value_parser = MarketIdentifier::parse_arg)]
    target: MarketIdentifier,
}

#[derive(Debug, Clone)]
enum MarketIdentifier {
    Id(String),
    Slug(String),
}

impl MarketIdentifier {
    fn parse_arg(s: &str) -> Result<Self, String> {
        // TODO: allow id/slug as input
        let url: Url = s.parse().map_err(|_| "Invalid url".to_string())?;
        match url.host_str() {
            Some("manifold.markets") => {}
            Some("dev.manifold.markets") => {}
            _ => return Err("invalid Manifold host".to_string()),
        }
        let manifold_error = "Failed to parse Manifold market url";
        let mut path = url.path_segments().ok_or(manifold_error.to_string())?;
        if path.next().is_none() {
            return Err(manifold_error.to_string());
        }
        // validate slug
        let slug = path.next().ok_or("Missing market slug".to_string())?;
        if !slug
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
            || slug.len() > 100
        {
            return Err("Invalid market slug".to_string());
        }
        Ok(Self::Slug(slug.to_string()))
    }
}

#[derive(Debug, Parser)]
struct MirrorArgs {
    /// Question to mirror (url)
    #[arg(value_parser = MirrorTarget::parse_arg)]
    target: MirrorTarget,
    /// Create mirror even if we think someone else already did
    #[arg(long = "force")]
    force: bool,
}

#[derive(Debug, Clone)]
struct MirrorTarget {
    source: QuestionSource,
    source_id: String,
}

impl MirrorTarget {
    fn parse_arg(s: &str) -> Result<Self, String> {
        let generic_error = "Invalid URL";
        let url: Url = s.parse().map_err(|_| generic_error.to_string())?;
        match url.host_str() {
            Some("www.metaculus.com") => {
                let metaculus_error = "Failed to parse Metaculus question url";
                let mut path = url.path_segments().ok_or(metaculus_error.to_string())?;
                if path.next() != Some("questions") {
                    return Err(metaculus_error.to_string());
                }
                // validate and normalize id
                let id = path
                    .next()
                    .ok_or("Missing Metaculus question id".to_string())?
                    .parse::<u64>()
                    .map_err(|_| "Metaculus question id must be a positive integer".to_string())?
                    .to_string();
                Ok(Self {
                    source: QuestionSource::Metaculus,
                    source_id: id.into(),
                })
            }
            Some("kalshi.com") => {
                Err("Managram mirroring for Kalshi has not been implemented yet.".to_string())
            }
            Some(host) => Err(format!("Unrecognized host `{}`", host)),
            None => Err(generic_error.to_string()),
        }
    }
}
