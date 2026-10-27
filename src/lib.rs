pub mod engine;
pub mod model;
pub mod repository;
pub mod ui;
pub mod watch;

use clap::Parser;
use std::path::PathBuf;

#[derive(Parser, Debug, Clone)]
#[command(
    version,
    about = "Live Git changes and a bounded history of observed edits"
)]
pub struct Config {
    /// Any path within a Git repository or linked worktree
    #[arg(default_value = ".")]
    pub path: PathBuf,
    /// Extra repository-relative ignore glob (repeatable)
    #[arg(long = "ignore")]
    pub ignores: Vec<String>,
    #[arg(long, default_value_t = 1000, value_parser = clap::value_parser!(u32).range(1..))]
    pub history_entries: u32,
    #[arg(long, default_value_t = 64 * 1024 * 1024, value_parser = clap::value_parser!(u64).range(1024..))]
    pub history_bytes: u64,
    /// Maximum bytes loaded for a textual file
    #[arg(long, default_value_t = 1024 * 1024, value_parser = clap::value_parser!(u64).range(1..))]
    pub max_text_bytes: u64,
    /// Budget for retained clear-baseline and observed text contents
    #[arg(long, default_value_t = 256 * 1024 * 1024, value_parser = clap::value_parser!(u64).range(1024..))]
    pub snapshot_bytes: u64,
    #[arg(long, default_value_t = 75, value_parser = clap::value_parser!(u64).range(10..=2000))]
    pub debounce_ms: u64,
    /// Safety reconciliation interval (seconds)
    #[arg(long, default_value_t = 30, value_parser = clap::value_parser!(u64).range(1..))]
    pub reconcile_seconds: u64,
    #[arg(long)]
    pub reduced_motion: bool,
    #[arg(long)]
    pub no_animations: bool,
    #[arg(long, value_enum, default_value_t = Theme::Dark)]
    pub theme: Theme,
    /// Print one snapshot and exit, without a terminal UI
    #[arg(long)]
    pub once: bool,
}

#[derive(clap::ValueEnum, Debug, Clone, Copy)]
pub enum Theme {
    Dark,
    Light,
}
