mod app;
mod archive;
mod config;
mod console;
mod manifest;
mod paths;
mod progress;
mod update;
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
        /// 省略時は作業フォルダ・ログを既定値で動かす
        #[arg(short, long)]
        config: Option<PathBuf>,
        #[arg(short, long)]
        input: PathBuf,
        #[arg(short, long)]
        output: PathBuf,
    },
    /// GitHub Release の最新版に自分自身を置き換える
    Update,
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
        } => app::restore(config.as_ref(), &input, &output),
        Command::Update => update::run(),
    }
}
