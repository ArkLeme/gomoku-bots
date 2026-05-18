mod args;
mod client;
mod local_server;
mod runtime;
mod ui_state;
mod web_server;

use anyhow::{Context, Result};
use args::{AuthMode, Cli};
use clap::Parser;
use gomoku_core::protocol::{ClientCommand, JoinRoomError, JoinRoomResponse};
use reqwest::Client;
use serde_json::Value;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::watch;
use url::Url;

use crate::client::SignalRClient;
use crate::runtime::Runtime;
use crate::ui_state::UiState;
use crate::web_server::{BotDefaults, StartRequest};

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    if cli.local_server {
        return local_server::run(cli.local_server_port).await;
    }
    if cli.demo {
        run_demo();
        return Ok(());
    }
    if cli.ui {
        run_ui_mode(cli).await
    } else {
        run_bot_mode(cli).await
    }
}

async fn run_ui_mode(cli: Cli) -> Result<()> {
    let (ui_tx, ui_rx) = watch::channel(UiState::default());
    let ui_tx = Arc::new(ui_tx);
    let (start_tx, mut start_rx) = tokio::sync::mpsc::unbounded_channel::<StartRequest>();
    let (error_tx, mut error_rx) = tokio::sync::mpsc::unbounded_channel::<String>();

    let defaults = BotDefaults {
        room_name: cli.room_name.clone(),
        move_time_seconds: cli.move_time_seconds,
        base_url: cli.base_url.clone(),
        bot_name: cli.bot_name.clone(),
        strategy: cli.strategy,
    };

    let port = cli.ui_port;
    let ui_rx_web = ui_rx.clone();
    tokio::spawn(async move {
        if let Err(e) = web_server::run(ui_rx_web, defaults, start_tx, port).await {
            eprintln!("web server error: {e}");
        }
    });

    let mut task_handles: Vec<tokio::task::JoinHandle<()>> = Vec::new();

    loop {
        tokio::select! {
            maybe_config = start_rx.recv() => {
                let Some(config) = maybe_config else { break; };

                for handle in task_handles.drain(..) {
                    handle.abort();
                }
                let _ = ui_tx.send(UiState::default());

                let error_tx1 = error_tx.clone();
                let config1 = config.clone();
                let debug_websocket = cli.debug_websocket;

                // Per-game watch channels; a merge task folds both into the shared ui_tx.
                let (bot1_tx_chan, mut bot1_rx) = watch::channel(UiState::default());
                let (bot2_tx_chan, mut bot2_rx) = watch::channel(UiState::default());
                let bot1_tx = Arc::new(bot1_tx_chan);
                let bot2_tx = Arc::new(bot2_tx_chan);

                let merge_ui_tx = ui_tx.clone();
                tokio::spawn(async move {
                    let mut state1 = UiState::default();
                    let mut opp_candidates = Vec::new();
                    let mut opp_last_decision = None;
                    loop {
                        tokio::select! {
                            res = bot1_rx.changed() => {
                                if res.is_err() { break; }
                                state1 = bot1_rx.borrow_and_update().clone();
                                state1.opponent_candidates = opp_candidates.clone();
                                state1.opponent_last_decision = opp_last_decision.clone();
                                let _ = merge_ui_tx.send(state1.clone());
                            }
                            res = bot2_rx.changed() => {
                                if res.is_err() { continue; }
                                let state2 = bot2_rx.borrow_and_update().clone();
                                opp_candidates = state2.candidates.clone();
                                opp_last_decision = state2.last_decision.clone();
                                let mut merged = state1.clone();
                                merged.opponent_candidates = opp_candidates.clone();
                                merged.opponent_last_decision = opp_last_decision.clone();
                                let _ = merge_ui_tx.send(merged);
                            }
                        }
                    }
                });

                // create a readiness channel so we start bot2 only after bot1 finished joining/initializing
                let (ready_tx, ready_rx) = tokio::sync::oneshot::channel::<()>();

                let h1 = tokio::spawn(async move {
                    if let Err(e) = launch_bot(&config1, true, Some(bot1_tx), Some(ready_tx), debug_websocket).await {
                        eprintln!("bot1 error: {e}");
                        let _ = error_tx1.send(e.to_string());
                    }
                });

                // wait for bot1 readiness with a timeout; if it times out, proceed to start bot2 anyway
                match tokio::time::timeout(Duration::from_secs(5), ready_rx).await {
                    Ok(Ok(())) => {}
                    Ok(Err(_)) => eprintln!("bot1 readiness sender dropped"),
                    Err(_) => eprintln!("timed out waiting for bot1 readiness, starting bot2 anyway"),
                }

                let error_tx2 = error_tx.clone();
                let config2 = config.clone();
                let h2 = tokio::spawn(async move {
                    if let Err(e) = launch_bot(&config2, false, Some(bot2_tx), None, debug_websocket).await {
                        eprintln!("bot2 error: {e}");
                        let _ = error_tx2.send(e.to_string());
                    }
                });

                task_handles.push(h1);
                task_handles.push(h2);
            }
            maybe_error = error_rx.recv() => {
                let Some(error) = maybe_error else { continue; };

                for handle in task_handles.drain(..) {
                    handle.abort();
                }

                let _ = ui_tx.send(UiState::startup_error(error));
            }
        }
    }

    Ok(())
}

async fn launch_bot(
    config: &StartRequest,
    is_bot1: bool,
    ui_tx: Option<Arc<watch::Sender<UiState>>>,
    ready_tx: Option<tokio::sync::oneshot::Sender<()>>,
    debug_websocket: bool,
) -> Result<()> {
    let bot = if is_bot1 { &config.bot1 } else { &config.bot2 };

    let auth = match bot.auth {
        AuthMode::Guest => {
            let name = bot.name.clone().unwrap_or_else(|| "bot".to_string());
            Auth::guest(name)
        }
        AuthMode::Registered => {
            let username = bot.username.clone().context("username required for registered auth")?;
            let password = bot.password.clone().context("password required for registered auth")?;
            let token = login(&config.base_url, &username, &password).await?;
            Auth::registered(token)
        }
    };

    let url = build_ws_url(&config.base_url, auth.query_value())?;
    println!("bot{} connecting", if is_bot1 { 1 } else { 2 });
    let (client, events) = SignalRClient::connect(&url, debug_websocket).await?;
    println!("bot{} connected", if is_bot1 { 1 } else { 2 });

    let runtime = Runtime::new(
        client.clone(),
        events,
        is_bot1,
        bot.strategy,
        ui_tx,
        config.move_time_seconds,
        false,
    );

    if is_bot1 {
        println!("creating room {}", config.room_name);
        let cmd = ClientCommand::CreateTwoPlayerRoom {
            room_name: config.room_name.clone(),
            move_time_seconds: config.move_time_seconds,
        };
        let created_room: serde_json::Value = client.invoke_command(&cmd).await?;
        println!("create room response: {created_room}");
        if let Some(err) = created_room.get("error").and_then(Value::as_str) {
            if !err.is_empty() {
                return Err(anyhow::anyhow!("create room failed: {err}"));
            }
        }

        if let Some(moves_history) = config.initial_board_moves_history.as_ref() {
            println!("setting initial board for room {}", config.room_name);
            let initial_board_cmd = ClientCommand::SetInitialBoard {
                room_id: config.room_name.clone(),
                moves_history: moves_history.clone(),
            };
            let resp: serde_json::Value = client.invoke_command(&initial_board_cmd).await?;
            println!("set initial board response: {resp}");
        }
    }

    let join_response = if is_bot1 {
        join_room_until_preload(&client, &config.room_name, config.initial_board_moves_history.as_deref()).await?
    } else {
        join_room_until_exists(&client, &config.room_name).await?
    };

    runtime.init_room(join_response, config.room_name.clone()).await?;
    println!("bot{} ready", if is_bot1 { 1 } else { 2 });

    // signal readiness to any caller waiting for the bot to be initialized
    if let Some(tx) = ready_tx {
        let _ = tx.send(());
    }
    runtime.run().await?;
    Ok(())
}

async fn run_bot_mode(cli: Cli) -> Result<()> {
    let auth = match cli.mode {
        AuthMode::Guest => Auth::guest(cli.bot_name.clone()),
        AuthMode::Registered => {
            let username = cli.username.clone().context("--username is required in registered mode")?;
            let password = cli.password.clone().context("--password is required in registered mode")?;
            let token = login(&cli.base_url, &username, &password).await?;
            Auth::registered(token)
        }
    };

    let url = build_ws_url(&cli.base_url, auth.query_value())?;
    println!("connecting to websocket hub");
    let (client, events) = SignalRClient::connect(&url, cli.debug_websocket).await?;
    println!("connected");

    let runtime = Runtime::new(
        client.clone(),
        events,
        cli.create_room,
        cli.strategy,
        None,
        cli.move_time_seconds,
        true,
    );

    if cli.create_room {
        println!("creating room {}", cli.room_name);
        let cmd = ClientCommand::CreateTwoPlayerRoom {
            room_name: cli.room_name.clone(),
            move_time_seconds: cli.move_time_seconds,
        };
        let created_room: serde_json::Value = client.invoke_command(&cmd).await?;
        println!("create room response: {created_room}");
        if let Some(err) = created_room.get("error").and_then(Value::as_str) {
            if !err.is_empty() {
                return Err(anyhow::anyhow!("create room failed: {err}"));
            }
        }

        if let Some(moves_history) = cli.initial_board_moves_history.as_ref() {
            println!("setting initial board for room {}", cli.room_name);
            let initial_board_cmd = ClientCommand::SetInitialBoard {
                room_id: cli.room_name.clone(),
                moves_history: moves_history.clone(),
            };
            let set_initial_board_response: serde_json::Value =
                client.invoke_command(&initial_board_cmd).await?;
            println!("set initial board response: {set_initial_board_response}");
        }
    }

    println!("joining room {}", cli.room_name);
    let join_response = join_room_until_preload(
        &client,
        &cli.room_name,
        cli.initial_board_moves_history.as_deref(),
    )
    .await?;

    runtime.init_room(join_response, cli.room_name).await?;
    println!("runtime ready, waiting for game events");
    runtime.run().await?;
    Ok(())
}

async fn join_room_until_exists(
    client: &SignalRClient,
    room_name: &str,
) -> Result<JoinRoomResponse> {
    let join_cmd = ClientCommand::JoinRoom { room_name: room_name.to_string() };
    for attempt in 1..=20 {
        let response: JoinRoomResponse = client.invoke_command(&join_cmd).await?;
        if response.success {
            return Ok(response);
        }
        if matches!(response.error, Some(JoinRoomError::RoomDoesNotExist)) {
            println!("room {room_name} not yet available, retrying ({attempt}/20)");
            tokio::time::sleep(Duration::from_millis(500)).await;
            continue;
        }
        return Err(anyhow::anyhow!("join room failed: {:?}", response.error));
    }
    Err(anyhow::anyhow!("room never became available after 20 attempts"))
}

async fn join_room_until_preload(
    client: &SignalRClient,
    room_name: &str,
    expected_initial_board_moves_history: Option<&str>,
) -> Result<JoinRoomResponse> {
    let join_cmd = ClientCommand::JoinRoom {
        room_name: room_name.to_string(),
    };

    for attempt in 1..=10 {
        let join_response: JoinRoomResponse = client.invoke_command(&join_cmd).await?;

        if !join_response.success {
            return Err(anyhow::anyhow!(
                "join room failed before preload became available: {:?}",
                join_response.error
            ));
        }

        if expected_initial_board_moves_history.is_none() {
            return Ok(join_response);
        }

        let has_initial_board = join_response
            .data
            .as_ref()
            .and_then(|data| data.lobby_data.initial_board_moves_history.as_deref())
            .is_some();

        if has_initial_board {
            return Ok(join_response);
        }

        println!(
            "join response does not yet include initial board history, retrying ({attempt}/10)"
        );
        tokio::time::sleep(Duration::from_millis(500)).await;
    }

    Err(anyhow::anyhow!(
        "join room response never included initialBoardMovesHistory"
    ))
}

fn build_ws_url(base_url: &str, auth: Option<(String, String)>) -> Result<Url> {
    let mut url = Url::parse(&format!("{}/gameHub", base_url.trim_end_matches('/')))?;
    let scheme = match url.scheme() {
        "https" => "wss",
        "http" => "ws",
        other => other,
    }
    .to_string();
    if url.set_scheme(&scheme).is_err() {
        return Err(anyhow::anyhow!("failed to convert API URL to websocket URL"));
    }
    if let Some((key, value)) = auth {
        url.query_pairs_mut().append_pair(&key, &value);
    }
    Ok(url)
}

async fn login(base_url: &str, username: &str, password: &str) -> Result<String> {
    let client = Client::new();
    let response = client
        .post(format!("{}/api/auth/login", base_url.trim_end_matches('/')))
        .json(&serde_json::json!({
            "username": username,
            "password": password,
        }))
        .send()
        .await?
        .error_for_status()?;

    let payload: serde_json::Value = response.json().await?;
    let token = payload
        .get("accessToken")
        .and_then(Value::as_str)
        .context("missing accessToken in login response")?;
    Ok(token.to_string())
}

#[derive(Clone)]
struct Auth {
    query: Option<(String, String)>,
}

impl Auth {
    fn guest(bot_name: String) -> Self {
        Self {
            query: Some(("guestUsername".to_string(), bot_name)),
        }
    }

    fn registered(token: String) -> Self {
        Self {
            query: Some(("access_token".to_string(), token)),
        }
    }

    fn query_value(self) -> Option<(String, String)> {
        self.query
    }
}

fn run_demo() {
    use gomoku_core::board::{Board, Piece, Position};
    use gomoku_core::engine::DecisionEngine;
    use gomoku_core::game::GameSnapshot;

    let mut board = Board::new();
    board.place(Position::new(9, 9), Piece::Black).unwrap();
    board.place(Position::new(9, 10), Piece::White).unwrap();

    let snapshot = GameSnapshot::new(board, Piece::Black, Piece::Black);
    let engine = DecisionEngine::default();

    if let Some(plan) = engine.choose_plan(&snapshot) {
        println!("strategy: {}", plan.strategy.label());
        println!("move: ({}, {})", plan.position.row, plan.position.column);
        println!("reason: {}", plan.reason);
    } else {
        println!("no move found");
    }
}
