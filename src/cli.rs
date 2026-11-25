use clap::{Parser, Subcommand};
#[derive(Parser)]
#[command(version, about = "Remove apps. Leave no traces.")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,
}
#[derive(Subcommand)]
pub enum Command {
    /// Inspect durable cleanup journals (read-only).
    History {
        id: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Re-scan data using saved app metadata; never moves files.
    Recover {
        id: String,
        #[arg(long)]
        json: bool,
    },
    List {
        #[arg(long)]
        json: bool,
    },
    Scan {
        application: String,
        #[arg(long)]
        json: bool,
    },
    Remove {
        application: String,
        #[arg(long)]
        dry_run: bool,
    },
    Orphans {
        #[arg(long)]
        json: bool,
    },
    Analyze {
        #[arg(long)]
        json: bool,
    },
}
