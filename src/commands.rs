use anyhow::{anyhow, bail, Context, Result};
use log::{info, warn};
use reqwest::blocking::Client;

use crate::args::{self, Commands, ListCommands};
use crate::manifold::{self, SendManagramArgs};
use crate::settings::Settings;
use crate::types::QuestionSource;
use crate::{db, log_if_err, kalshi, managrams, metaculus, mirror};

pub(crate) fn run_command(
    config: Settings,
    args: args::Cli,
) -> std::result::Result<(), anyhow::Error> {
    match args.command {
        Commands::List(cmd) => list_markets(&config, cmd),
        Commands::Mirror {
            source,
            id,
            allow_resolved,
        } => mirror_question(&config, source, id, allow_resolved),
        Commands::Sync {
            kalshi,
            metaculus,
            managrams,
            manifold_self,
            manifold_other,
            all,
        } => sync(
            &config,
            kalshi,
            metaculus,
            managrams,
            manifold_self,
            manifold_other,
            all,
        ),
        Commands::AutoMirror { source, dry_run } => auto_mirror(&config, source, dry_run),
        Commands::SendManagram {
            amount,
            to_id,
            message,
        } => send_managram(&config, amount, to_id, message),
        Commands::ProcessManagrams => process_managrams(&config),
    }
}

pub fn process_managrams(config: &Settings) -> Result<()> {
    let client = Client::new();
    let db = db::open(&config)?;
    log_if_err!(managrams::sync_managrams(&client, &db, config));
    managrams::process_managrams(&client, &db, config)?;
    Ok(())
}

pub fn list_markets(config: &Settings, subcommand: ListCommands) -> Result<()> {
    let db = db::open(&config)?;
    match subcommand {
        ListCommands::Mirrors { resolved } => {
            let mirrors = if resolved {
                db::get_resolved_mirrors(&db, None)
            } else {
                db::get_unresolved_mirrors(&db, None)
            };
            for mirror in mirrors? {
                println!("{:#?}", mirror);
            }
        }
        ListCommands::ThirdParty => {
            for mirror in db::get_third_party_mirrors(&db)? {
                println!("{:#?}", mirror);
            }
        }
    }
    Ok(())
}

pub fn mirror_question(
    config: &Settings,
    source: QuestionSource,
    id: String,
    allow_resolved: bool,
) -> Result<()> {
    let client = Client::new();
    let db = db::open(&config)?;
    match source {
        QuestionSource::Metaculus => {
            let metaculus_question = metaculus::get_question(&client, &id, config)
                .with_context(|| "failed to fetch question from Metaculus")?;
            if metaculus_question.is_resolved() {
                if allow_resolved {
                    warn!("question has already resolved");
                } else {
                    return Err(anyhow!("question has already resolved"));
                }
            }
            // TODO: use function clone_metaculus
            let question = (&metaculus_question)
                .try_into()
                .with_context(|| "failed to convert Metaculus question to common format")?;
            let row = mirror::mirror_question(&client, &db, &question, config)?;
            println!("Mirrored question:\n{:#?}", row);
        }
        QuestionSource::Kalshi => {
            let kalshi_question = kalshi::get_question(&client, &id, config)
                .with_context(|| "failed to fetch question from Kalshi")?;
            if kalshi_question.is_resolved() {
                if allow_resolved {
                    warn!("question has already resolved");
                } else {
                    return Err(anyhow!("question has already resolved"));
                }
            }
            let question = (&kalshi_question)
                .try_into()
                .with_context(|| "failed to convert Kalshi question to common format")?;
            let row = mirror::mirror_question(&client, &db, &question, config)?;
            println!("Mirrored question:\n{:#?}", row);
        }
        QuestionSource::Polymarket => {
            bail!("Polymarket mirroring hasn't been implemented yet");
        }
    }
    Ok(())
}

pub fn sync(
    config: &Settings,
    kalshi: bool,
    metaculus: bool,
    managrams: bool,
    manifold_self: bool,
    manifold_other: bool,
    all: bool,
) -> Result<()> {
    if !(kalshi || metaculus || managrams || manifold_self || manifold_other || all) {
        bail!("Provide at least one sync target.");
    }

    let client = Client::new();
    let db = db::open(&config)?;

    if manifold_self || all {
        log_if_err!(mirror::sync_manifold_to_db(&client, &db, config));
    }

    if manifold_other || all {
        log_if_err!(mirror::sync_third_party_mirrors(&client, &db, config));
    }

    if kalshi || all {
        log_if_err!(mirror::sync_resolutions_to_manifold(
            &client,
            &db,
            config,
            Some(QuestionSource::Kalshi)
        ));
    }

    if metaculus || all {
        log_if_err!(mirror::sync_resolutions_to_manifold(
            &client,
            &db,
            config,
            Some(QuestionSource::Metaculus)
        ));
    }

    if managrams || all {
        log_if_err!(managrams::sync_managrams(&client, &db, config));
    }

    Ok(())
}

pub fn auto_mirror(config: &Settings, source: QuestionSource, dry_run: bool) -> Result<()> {
    let client = Client::new();
    let db = db::open(&config)?;
    match source {
        QuestionSource::Metaculus => mirror::auto_mirror_metaculus(&client, &db, config, dry_run)?,
        QuestionSource::Kalshi => mirror::auto_mirror_kalshi(&client, &db, config, dry_run)?,
        QuestionSource::Polymarket => {
            todo!()
        }
    }
    Ok(())
}

pub fn send_managram(config: &Settings, amount: f64, to_id: String, message: String) -> Result<()> {
    let client = Client::new();
    info!("Sending managram to {}", to_id);
    manifold::send_managram(
        &client,
        config,
        &SendManagramArgs {
            amount,
            to_ids: vec![to_id],
            message,
        },
    )?;
    Ok(())
}
