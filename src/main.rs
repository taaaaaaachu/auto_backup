mod app;
mod archive;
mod config;
mod console;
mod manifest;
mod paths;
mod progress;
#[cfg(windows)]
mod vss;

use std::path::PathBuf;

use anyhow::Result;
use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(name = "auto_backup")]
#[command(about = "Create and restore split tar.zst directory backups.")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    Backup {
        #[arg(short, long)]
        config: PathBuf,
    },
    Restore {
        #[arg(short, long)]
        config: PathBuf,
        #[arg(short, long)]
        input: PathBuf,
        #[arg(short, long)]
        output: PathBuf,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let _quick_edit = console::QuickEditGuard::disable();

    match cli.command {
        Command::Backup { config } => app::backup(&config),
        Command::Restore {
            config,
            input,
            output,
        } => app::restore(&config, &input, &output),
    }
}
