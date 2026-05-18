use clap::{Parser, ValueEnum};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Parser)]
#[command(author, version, about = "Gomoku websocket bot")]
pub struct Cli {
    #[arg(long, default_value = "guest")]
    pub mode: AuthMode,

    #[arg(long, default_value = "gomoku-bot")]
    pub bot_name: String,

    #[arg(long, default_value = "arkleme-room")]
    pub room_name: String,

    #[arg(long)]
    pub create_room: bool,

    #[arg(long)]
    pub initial_board_moves_history: Option<String>,

    #[arg(long)]
    pub username: Option<String>,

    #[arg(long)]
    pub password: Option<String>,

    #[arg(long, default_value = "adaptive")]
    pub strategy: StrategyChoice,

    #[arg(long, default_value = "https://api-connect5.dev.codebusters.cloud")]
    pub base_url: String,

    #[arg(long)]
    pub demo: bool,

    #[arg(long)]
    pub ui: bool,

    #[arg(long)]
    pub debug_websocket: bool,

    #[arg(long)]
    pub local_server: bool,

    #[arg(long, default_value = "8081")]
    pub local_server_port: u16,

    #[arg(long, default_value = "8080")]
    pub ui_port: u16,

    #[arg(long, default_value = "5")]
    pub move_time_seconds: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AuthMode {
    Guest,
    Registered,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum StrategyChoice {
    /// Immediate win/block only — fast but no lookahead.
    Tactical,
    /// Greedy positional heuristic — good for early moves.
    Pattern,
    /// Fast threat checks then PVS + TT + VCF deep search — recommended default.
    Adaptive,
    /// VCF — detects and plays forced-four sequences, falls back to greedy.
    Vcf,
    /// VCT — Victory by Consecutive Threats; extends VCF with open-three forks.
    Vct,
    /// PVS + transposition table + VCF at leaf nodes — best single-strategy mode.
    Search,
}
