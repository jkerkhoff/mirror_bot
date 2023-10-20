use clap::{Parser, Subcommand};

use crate::types::QuestionSource;

#[derive(Debug, Parser)]
#[command(name = "mirror_bot")]
#[command(about = "External market mirror bot for Manifold.", long_about = None)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Debug, Subcommand)]
pub enum ListCommands {
    /// List mirror markets managed by the bot
    Mirrors {
        /// Show resolved mirrors instead of unresolved
        #[arg(short = 'r', long = "resolved")]
        resolved: bool,
    },
    /// List mirrors created by others that we know about
    ThirdParty,
}

#[derive(Debug, Subcommand)]
pub enum Commands {
    /// List mirrors, managrams, etc.
    #[command(subcommand)]
    List(ListCommands),
    #[command(arg_required_else_help = true)]
    /// Mirror a specific question to Manifold
    Mirror {
        source: QuestionSource,
        id: String,
        /// Mirror question even if source has already resolved
        #[arg(short = 'r', long = "allow-resolved")]
        allow_resolved: bool,
    },
    /// Sync source resolutions to Manifold
    #[command()]
    Sync {
        /// Sync Kalshi resolutions to manifold
        #[arg(short = 'k', long = "kalshi")]
        kalshi: bool,
        /// Sync Metaculus resolutions to manifold
        #[arg(short = 'm', long = "metaculus")]
        metaculus: bool,
        /// Sync Manifold managrams to db
        #[arg(short = 'g', long = "managrams")]
        managrams: bool,
        /// Sync state of our mirror markets from Manifold to db
        #[arg(short = 's', long = "manifold-self")]
        manifold_self: bool,
        /// Sync state of third party mirror markets from Manifold to db
        #[arg(short = 'o', long = "manifold-other")]
        manifold_other: bool,
        /// Sync everything
        #[arg(short = 'a', long = "all")]
        all: bool,
    },
    /// Mirror new questions from source platforms to Manifold
    #[command()]
    AutoMirror {
        source: QuestionSource,
        #[arg(long = "dry-run")]
        dry_run: bool,
    },
    /// Send a managram
    #[command()]
    SendManagram {
        amount: f64,
        to_id: String,
        message: String,
    },
    /// Process managram requests
    #[command()]
    ProcessManagrams,
}
