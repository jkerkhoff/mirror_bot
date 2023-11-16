use anyhow::Result;
use args::Cli;
use clap::Parser;
use tracing_subscriber::prelude::*;
use tracing_subscriber::{fmt, EnvFilter};

mod args;
mod commands;
mod db;
mod kalshi;
mod managrams;
mod manifold;
mod metaculus;
mod mirror;
mod settings;
mod types;
mod util;

fn main() -> Result<(), anyhow::Error> {
    dotenvy::dotenv().ok();

    tracing_subscriber::registry()
        .with(EnvFilter::from_default_env())
        .with(fmt::layer().json())
        .init();

    let config = settings::Settings::new()?;
    let args = Cli::parse();

    commands::run_command(config, args)
}
