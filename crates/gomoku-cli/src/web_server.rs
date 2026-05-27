use axum::{
    extract::{
        ws::{Message, WebSocket},
        State, WebSocketUpgrade,
    },
    http::StatusCode,
    response::{Html, IntoResponse},
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex, watch};

use crate::args::{AuthMode, StrategyChoice};
use crate::ui_state::UiState;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BotFormSpec {
    pub name: Option<String>,
    pub auth: AuthMode,
    pub username: Option<String>,
    pub password: Option<String>,
    pub strategy: StrategyChoice,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StartRequest {
    pub room_name: String,
    pub move_time_seconds: u32,
    pub base_url: String,
    pub initial_board_moves_history: Option<String>,
    pub bot1: BotFormSpec,
    pub bot2: BotFormSpec,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BotDefaults {
    pub room_name: String,
    pub move_time_seconds: u32,
    pub base_url: String,
    pub bot_name: String,
    pub strategy: StrategyChoice,
}

#[derive(Clone)]
struct AppState {
    ui_rx: watch::Receiver<UiState>,
    defaults: BotDefaults,
    start_tx: mpsc::UnboundedSender<StartRequest>,
    // current running configuration (set on /api/start)
    current_config: Arc<Mutex<Option<StartRequest>>>,
    // wins per strategy label
    stats: Arc<Mutex<HashMap<String, u32>>>,
}

pub async fn run(
    ui_rx: watch::Receiver<UiState>,
    defaults: BotDefaults,
    start_tx: mpsc::UnboundedSender<StartRequest>,
    port: u16,
) -> anyhow::Result<()> {
    let state = AppState { ui_rx, defaults, start_tx, current_config: Arc::new(Mutex::new(None)), stats: Arc::new(Mutex::new(HashMap::new())) };

    // spawn a background task to observe UI state changes and update stats on GameOver
    let stats_clone = state.stats.clone();
    let config_clone = state.current_config.clone();
    let mut rx_for_stats = state.ui_rx.clone();
    tokio::spawn(async move {
        let mut last_phase: Option<String> = None;
        loop {
            if rx_for_stats.changed().await.is_err() { break; }
            let s = rx_for_stats.borrow().clone();
            let phase = format!("{:?}", s.phase);
            if phase != last_phase.as_deref().unwrap_or_default() {
                if let Some(result) = &s.game_result {
                    // determine winner color by checking for Black/White
                    let winner = if result.contains("Black") { Some("Black") } else if result.contains("White") { Some("White") } else { None };
                    if let Some(winner_color) = winner {
                        let cfg_opt = { config_clone.lock().await.clone() };
                        if let Some(cfg) = cfg_opt {
                            let strat = if winner_color == "Black" { cfg.bot1.strategy } else { cfg.bot2.strategy };
                            let label = format!("{:?}", strat);
                            let mut map = stats_clone.lock().await;
                            *map.entry(label).or_insert(0) += 1;
                        }
                    }
                }
                last_phase = Some(phase);
            }
        }
    });

    let app = Router::new()
        .route("/", get(serve_index))
        .route("/ws", get(ws_handler))
        .route("/api/defaults", get(get_defaults))
        .route("/api/start", post(post_start))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{port}")).await?;
    println!("UI available at http://localhost:{port}");
    axum::serve(listener, app).await?;
    Ok(())
}

async fn serve_index() -> Html<&'static str> {
    Html(include_str!("web/index.html"))
}

async fn ws_handler(ws: WebSocketUpgrade, State(state): State<AppState>) -> impl IntoResponse {
    let rx = state.ui_rx.clone();
    let stats = state.stats.clone();
    ws.on_upgrade(move |socket| handle_socket(socket, rx, stats))
}

async fn get_defaults(State(state): State<AppState>) -> Json<BotDefaults> {
    Json(state.defaults)
}

async fn post_start(
    State(state): State<AppState>,
    Json(request): Json<StartRequest>,
) -> impl IntoResponse {
    // record current config so we can attribute subsequent game results to the strategies used
    {
        let mut cur = state.current_config.lock().await;
        *cur = Some(request.clone());
    }

    if state.start_tx.send(request).is_ok() {
        StatusCode::OK
    } else {
        StatusCode::INTERNAL_SERVER_ERROR
    }
}

async fn handle_socket(mut socket: WebSocket, mut rx: watch::Receiver<UiState>, stats: Arc<Mutex<HashMap<String, u32>>>) {
    // On connect, send the current state augmented with stats
    {
        let state = rx.borrow_and_update().clone();
        let mut v = serde_json::to_value(&state).unwrap_or(serde_json::json!({}));
        // attach stats if present
        let map = stats.lock().await;
        if !map.is_empty() {
            if let Ok(sv) = serde_json::to_value(&*map) {
                v.as_object_mut().map(|m| m.insert("stats".to_string(), sv));
            }
        }
        if socket.send(Message::Text(v.to_string())).await.is_err() { return; }
    }

    loop {
        tokio::select! {
            result = rx.changed() => {
                if result.is_err() { break; }
                    let state = rx.borrow_and_update().clone();
                    // merge stats into the serialized payload so UI can display aggregated info
                    if let Ok(mut v) = serde_json::to_value(&state) {
                        let map = stats.lock().await;
                        if !map.is_empty() {
                            if let Ok(sv) = serde_json::to_value(&*map) {
                                v.as_object_mut().map(|m| m.insert("stats".to_string(), sv));
                            }
                        }
                        if socket.send(Message::Text(v.to_string())).await.is_err() { break; }
                    }
            }
            msg = socket.recv() => {
                match msg {
                    Some(Ok(Message::Close(_))) | None => break,
                    _ => {}
                }
            }
        }
    }
}

// (no helper)
