use anyhow::Result;
use args::Cli;
use clap::Parser;

mod args;
mod commands;
mod db;
mod managrams;
mod manifold;
mod metaculus;
mod mirror;
mod settings;
mod types;
mod util;

fn main() -> Result<(), anyhow::Error> {
    dotenvy::dotenv().ok();
    env_logger::builder().format_indent(Some(4)).init();

    let config = settings::Settings::new()?;
    let args = Cli::parse();

    commands::run_command(config, args)
}
