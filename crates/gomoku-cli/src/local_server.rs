use anyhow::{anyhow, Context, Result};
use axum::{
    extract::{
        ws::{Message, WebSocket},
        Query, State, WebSocketUpgrade,
    },
    response::IntoResponse,
    routing::get,
    Router,
};
use futures_util::{SinkExt, StreamExt};
use gomoku_core::board::{Board, Move, Piece, Position, BOARD_SIZE};
use gomoku_core::protocol::{
    Coordinate, DisqualificationReason, GameData, GameInfo, GameOverData, GameState,
    JoinRoomError, JoinRoomResponse, JoinRoomResponseData, LobbyData, PlayersTime, Role,
    RoomClosedReason, User, UserType,
};
use serde::Deserialize;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};
use std::time::Instant;
use tokio::sync::{mpsc, Mutex};

use crate::client::RECORD_SEPARATOR;

#[derive(Clone)]
struct AppState {
    state: Arc<Mutex<ServerState>>,
    next_connection_id: Arc<AtomicU64>,
}

#[derive(Default)]
struct ServerState {
    connections: HashMap<u64, Connection>,
    rooms: HashMap<String, Room>,
}

struct Connection {
    username: String,
    room_name: Option<String>,
    sender: mpsc::UnboundedSender<Message>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MemberKind {
    Player(Piece),
    Observer,
}

#[derive(Clone, Copy, Debug)]
struct RoomMember {
    connection_id: u64,
    kind: MemberKind,
}

struct Room {
    name: String,
    owner_connection_id: u64,
    move_time_seconds: u32,
    board: Board,
    moves: Vec<Move>,
    members: Vec<RoomMember>,
    started: bool,
    game_over: Option<GameState>,
    initial_board_moves_history: Option<String>,
    player_turn: Option<Piece>,
    black_time_ms: u64,
    white_time_ms: u64,
    turn_started_at: Option<Instant>,
    turn_start_date: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateTwoPlayerRoomRequest {
    room_name: String,
    move_time_seconds: u32,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SetInitialBoardRequest {
    room_id: String,
    moves_history: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct JoinRoomRequest {
    room_name: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StartGameRequest {
    room_name: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LeaveRoomRequest {
    room_name: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CloseRoomRequest {
    room_name: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PlayMoveRequest {
    room_name: String,
    row: usize,
    column: usize,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SetMoveTimeRequest {
    room_id: String,
    seconds: u32,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ChangeRoleRequest {
    room_id: String,
    new_role: Role,
    preferred_color: Option<Piece>,
}

pub async fn run(port: u16) -> Result<()> {
    let state = AppState {
        state: Arc::new(Mutex::new(ServerState::default())),
        next_connection_id: Arc::new(AtomicU64::new(1)),
    };

    let app = Router::new()
        .route("/", get(root))
        .route("/gameHub", get(ws_handler))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{port}")).await?;
    println!("local hub available at http://localhost:{port}");
    axum::serve(listener, app).await?;
    Ok(())
}

async fn root() -> &'static str {
    "Gomoku local hub is running"
}

async fn ws_handler(
    Query(query): Query<HashMap<String, String>>,
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
) -> impl IntoResponse {
    let username = query
        .get("guestUsername")
        .cloned()
        .unwrap_or_else(|| "guest".to_string());
    let connection_id = state.next_connection_id.fetch_add(1, Ordering::Relaxed);
    ws.on_upgrade(move |socket| handle_socket(socket, state, connection_id, username))
}

async fn handle_socket(socket: WebSocket, state: AppState, connection_id: u64, username: String) {
    let (mut sender, mut receiver) = socket.split();
    let (outbound_tx, mut outbound_rx) = mpsc::unbounded_channel::<Message>();

    {
        let mut server = state.state.lock().await;
        server.connections.insert(
            connection_id,
            Connection {
                username,
                room_name: None,
                sender: outbound_tx.clone(),
            },
        );
    }

    let writer = tokio::spawn(async move {
        while let Some(message) = outbound_rx.recv().await {
            if sender.send(message).await.is_err() {
                break;
            }
        }
    });

    while let Some(result) = receiver.next().await {
        let Ok(message) = result else {
            break;
        };

        match message {
            Message::Text(text) => {
                if let Err(error) = handle_text_frame(&state, connection_id, &text).await {
                    eprintln!("local hub error: {error:#}");
                }
            }
            Message::Close(_) => break,
            _ => {}
        }
    }

    cleanup_connection(&state, connection_id).await;
    drop(outbound_tx);
    writer.abort();
}

async fn handle_text_frame(state: &AppState, connection_id: u64, text: &str) -> Result<()> {
    for frame in text.split(RECORD_SEPARATOR).filter(|frame| !frame.trim().is_empty()) {
        let value: Value = serde_json::from_str(frame).context("failed to parse local hub frame")?;

        if is_handshake(&value) {
            send_to_connection(state, connection_id, serde_json::json!({})).await?;
            continue;
        }

        if value.get("type").and_then(Value::as_u64) != Some(1) {
            continue;
        }

        let target = value
            .get("target")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("missing hub target"))?
            .to_string();
        let invocation_id = value
            .get("invocationId")
            .and_then(Value::as_str)
            .map(str::to_string);
        let arguments = value
            .get("arguments")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();

        let result = handle_invocation(state, connection_id, &target, arguments).await;

        if let Some(invocation_id) = invocation_id {
            let payload = match result {
                Ok(value) => serde_json::json!({
                    "type": 3,
                    "invocationId": invocation_id,
                    "result": value,
                }),
                Err(error) => serde_json::json!({
                    "type": 3,
                    "invocationId": invocation_id,
                    "error": error.to_string(),
                }),
            };
            send_to_connection(state, connection_id, payload).await?;
        }
    }

    Ok(())
}

fn is_handshake(value: &Value) -> bool {
    value.get("protocol").and_then(Value::as_str) == Some("json")
        && value.get("version").and_then(Value::as_u64) == Some(1)
}

async fn handle_invocation(
    state: &AppState,
    connection_id: u64,
    target: &str,
    arguments: Vec<Value>,
) -> Result<Value> {
    match target {
        "CreateTwoPlayerRoom" => {
            let request: CreateTwoPlayerRoomRequest = parse_argument(arguments)?;
            create_room(state, connection_id, request).await?;
            Ok(serde_json::json!({ "success": true, "data": null, "error": null }))
        }
        "SetInitialBoard" => {
            let request = parse_set_initial_board(arguments)?;
            set_initial_board(state, connection_id, request).await?;
            Ok(serde_json::json!({ "success": true, "data": null, "error": null }))
        }
        "JoinRoom" => {
            let request: JoinRoomRequest = parse_argument(arguments)?;
            let response = join_room(state, connection_id, request).await?;
            Ok(serde_json::to_value(response)?)
        }
        "StartGame" => {
            let request: StartGameRequest = parse_argument(arguments)?;
            start_game(state, connection_id, request).await?;
            Ok(Value::Null)
        }
        "PlayMove" => {
            let request: PlayMoveRequest = parse_argument(arguments)?;
            play_move(state, connection_id, request).await?;
            Ok(Value::Null)
        }
        "LeaveRoom" => {
            let request: LeaveRoomRequest = parse_argument(arguments)?;
            leave_room(state, connection_id, request).await?;
            Ok(Value::Null)
        }
        "CloseRoom" => {
            let request: CloseRoomRequest = parse_argument(arguments)?;
            close_room(state, connection_id, request).await?;
            Ok(Value::Null)
        }
        "SetMoveTime" => {
            let request: SetMoveTimeRequest = parse_argument(arguments)?;
            set_move_time(state, connection_id, request).await?;
            Ok(Value::Null)
        }
        "ChangeRole" => {
            let request: ChangeRoleRequest = parse_argument(arguments)?;
            change_role(state, connection_id, request).await?;
            Ok(Value::Null)
        }
        "Quit" => {
            quit(state, connection_id).await?;
            Ok(Value::Null)
        }
        other => Err(anyhow!("unsupported hub target: {other}")),
    }
}

fn parse_set_initial_board(arguments: Vec<Value>) -> Result<SetInitialBoardRequest> {
    let room_id = arguments
        .get(0)
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("SetInitialBoard: missing room_id argument"))?
        .to_string();
    let moves_history = arguments
        .get(1)
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("SetInitialBoard: missing moves_history argument"))?
        .to_string();
    Ok(SetInitialBoardRequest { room_id, moves_history })
}

fn parse_argument<T>(mut arguments: Vec<Value>) -> Result<T>
where
    T: for<'de> Deserialize<'de>,
{
    let value = arguments
        .drain(..)
        .next()
        .ok_or_else(|| anyhow!("missing invocation argument"))?;
    Ok(serde_json::from_value(value).context("failed to decode invocation argument")?)
}

async fn create_room(
    state: &AppState,
    connection_id: u64,
    request: CreateTwoPlayerRoomRequest,
) -> Result<()> {
    let mut server = state.state.lock().await;
    server.rooms.insert(
        request.room_name.clone(),
        Room {
            name: request.room_name,
            owner_connection_id: connection_id,
            move_time_seconds: request.move_time_seconds,
            board: Board::new(),
            moves: Vec::new(),
            members: Vec::new(),
            started: false,
            game_over: None,
            initial_board_moves_history: None,
            player_turn: None,
            black_time_ms: 0,
            white_time_ms: 0,
            turn_started_at: None,
            turn_start_date: None,
        },
    );
    Ok(())
}

async fn set_initial_board(
    state: &AppState,
    connection_id: u64,
    request: SetInitialBoardRequest,
) -> Result<()> {
    let mut server = state.state.lock().await;
    let room = server
        .rooms
        .get_mut(&request.room_id)
        .ok_or_else(|| anyhow!("room not found"))?;
    if room.owner_connection_id != connection_id {
        return Err(anyhow!("only the room owner can preload the board"));
    }

    let moves = parse_moves_history(&request.moves_history)?;
    let mut board = Board::new();
    for mv in &moves {
        board.apply_move(*mv).context("failed to apply initial move")?;
    }

    let history = request.moves_history.clone();
    room.board = board;
    room.moves = moves;
    room.initial_board_moves_history = Some(request.moves_history);
    room.player_turn = Some(next_turn(&room.moves));
    room.game_over = None;
    room.started = false;
    room.black_time_ms = 0;
    room.white_time_ms = 0;
    room.turn_started_at = None;
    room.turn_start_date = None;

    let room_id = request.room_id.clone();
    drop(server);
    broadcast_to_room(state, &room_id, "InitialBoardChanged", serde_json::json!([history])).await?;
    Ok(())
}

async fn join_room(
    state: &AppState,
    connection_id: u64,
    request: JoinRoomRequest,
) -> Result<JoinRoomResponse> {
    let mut server = state.state.lock().await;

    let connection_username = server
        .connections
        .get(&connection_id)
        .ok_or_else(|| anyhow!("connection not found"))?
        .username
        .clone();

    let (member_snapshot, owner_connection_id, move_time_seconds, initial_history, started, game_over, board, player_turn, kind, moves_history, black_time_ms, white_time_ms, turn_start_date, room_name_log) = {
        let room = server
            .rooms
            .get_mut(&request.room_name)
            .ok_or_else(|| anyhow!("room not found"))?;

        let owner_joined = room
            .members
            .iter()
            .any(|member| member.connection_id == room.owner_connection_id);
        if room.owner_connection_id != connection_id && !owner_joined {
            return Ok(JoinRoomResponse {
                success: false,
                data: None,
                error: Some(JoinRoomError::RoomDoesNotExist),
            });
        }

        let player_count = room
            .members
            .iter()
            .filter(|member| matches!(member.kind, MemberKind::Player(_)))
            .count();
        let kind = if room.owner_connection_id == connection_id {
            MemberKind::Player(Piece::Black)
        } else if player_count == 0 {
            MemberKind::Player(Piece::Black)
        } else if player_count == 1 {
            MemberKind::Player(Piece::White)
        } else {
            MemberKind::Observer
        };

        if let Some(member) = room
            .members
            .iter_mut()
            .find(|member| member.connection_id == connection_id)
        {
            member.kind = kind;
        } else {
            room.members.push(RoomMember { connection_id, kind });
        }

        (
            room.members.clone(),
            room.owner_connection_id,
            room.move_time_seconds,
            room.initial_board_moves_history.clone(),
            room.started,
            room.game_over.clone(),
            room.board.clone(),
            room.player_turn,
            kind,
            room.moves_history_string(),
            room.black_time_ms,
            room.white_time_ms,
            room.turn_start_date.clone().unwrap_or_default(),
            room.name.clone(),
        )
    };

    {
        let connection = server
            .connections
            .get_mut(&connection_id)
            .ok_or_else(|| anyhow!("connection not found"))?;
        connection.room_name = Some(request.room_name.clone());
    }
    println!("[local] {} joined room {}", connection_username, room_name_log);

    let users = build_users(&member_snapshot, &server.connections);
    let user = build_user(connection_id, &connection_username, kind);
    let game_data = if started || game_over.is_some() {
        Some(GameData {
            time: PlayersTime {
                black_time_ms,
                white_time_ms,
                turn_start_date,
            },
            grid: board_to_grid(&board),
            player_turn: player_turn.unwrap_or(Piece::Black),
        })
    } else {
        None
    };
    let game_over_data = game_over.map(|game_state| GameOverData {
        winning_points: None,
        moves_history: moves_history.clone(),
        game_state,
    });

    let response = JoinRoomResponse {
        success: true,
        data: Some(JoinRoomResponseData {
            user: user.clone(),
            lobby_data: LobbyData {
                users: users.clone(),
                is_room_owner: owner_connection_id == connection_id,
                move_time_seconds,
                initial_board_moves_history: initial_history,
            },
            game_data,
            game_over_data,
        }),
        error: None,
    };

    let room_name = request.room_name.clone();
    drop(server);

    broadcast_to_room(
        state,
        &room_name,
        "UserJoined",
        serde_json::json!([user, users]),
    )
    .await?;

    Ok(response)
}

async fn start_game(state: &AppState, connection_id: u64, request: StartGameRequest) -> Result<()> {
    let player_turn = {
        let mut server = state.state.lock().await;
        let room = server
            .rooms
            .get_mut(&request.room_name)
            .ok_or_else(|| anyhow!("room not found"))?;
        if room.owner_connection_id != connection_id {
            return Err(anyhow!("only the room owner can start the game"));
        }
        if room.started {
            return Ok(());
        }

        room.started = true;
        let turn = next_turn(&room.moves);
        room.player_turn = Some(turn);
        room.turn_started_at = Some(Instant::now());
        room.turn_start_date = Some(now_utc_rfc3339());
        turn
    };

    broadcast_to_room(
        state,
        &request.room_name,
        "GameStarted",
        serde_json::json!([player_turn]),
    )
    .await
}

async fn play_move(state: &AppState, connection_id: u64, request: PlayMoveRequest) -> Result<()> {
    let (move_message, game_over_message) = {
        let mut server = state.state.lock().await;
        let username = server.connections.get(&connection_id).map(|c| c.username.clone()).unwrap_or_default();
        let room = server
            .rooms
            .get_mut(&request.room_name)
            .ok_or_else(|| anyhow!("room not found"))?;
        if room.game_over.is_some() {
            return Err(anyhow!("game is already over"));
        }
        if !room.started {
            return Err(anyhow!("game has not started yet"));
        }

        let member_kind = room
            .members
            .iter()
            .find(|member| member.connection_id == connection_id)
            .map(|member| member.kind)
            .ok_or_else(|| anyhow!("connection is not in the room"))?;
        let color = match member_kind {
            MemberKind::Player(color) => color,
            MemberKind::Observer => return Err(anyhow!("observers cannot play moves")),
        };
        if room.player_turn != Some(color) {
            return Err(anyhow!("it is not this player's turn"));
        }

        if let Some(started_at) = room.turn_started_at.take() {
            let elapsed_ms = started_at.elapsed().as_millis() as u64;
            match color {
                Piece::Black => room.black_time_ms = room.black_time_ms.saturating_add(elapsed_ms),
                Piece::White => room.white_time_ms = room.white_time_ms.saturating_add(elapsed_ms),
            }
        }

        let move_played = Move {
            row: request.row,
            column: request.column,
            color,
        };
        let position = Position::new(request.row, request.column);

        let won = room.board.would_win(position, color);

        if room.board.apply_move(move_played).is_err() {
            let disq_state = match color {
                Piece::Black => GameState::BlackDisqualified,
                Piece::White => GameState::WhiteDisqualified,
            };
            let moves_history = room.moves_history_string();
            room.started = false;
            room.game_over = Some(disq_state.clone());
            let disq_member_kind = room.members.iter()
                .find(|m| m.connection_id == connection_id)
                .map(|m| m.kind)
                .unwrap_or(MemberKind::Player(color));
            let disq_user = build_user(connection_id, &username, disq_member_kind);
            let disq_message = serde_json::json!([disq_user, DisqualificationReason::IllegalMove]);
            let game_over_message = serde_json::json!([disq_state, moves_history, Vec::<Coordinate>::new()]);
            drop(server);
            broadcast_to_room(state, &request.room_name, "PlayerDisqualified", disq_message).await?;
            broadcast_to_room(state, &request.room_name, "GameOver", game_over_message).await?;
            return Ok(());
        }
        room.moves.push(move_played);
        room.player_turn = Some(color.opponent());
        room.turn_started_at = Some(Instant::now());
        room.turn_start_date = Some(now_utc_rfc3339());

        let move_message = serde_json::json!([
            move_played,
            request.room_name.clone(),
            PlayersTime {
                black_time_ms: room.black_time_ms,
                white_time_ms: room.white_time_ms,
                turn_start_date: room.turn_start_date.clone().unwrap_or_default(),
            },
        ]);

        let game_over_message = if won {
            let game_state = match color {
                Piece::Black => GameState::BlackWin,
                Piece::White => GameState::WhiteWin,
            };
            room.started = false;
            room.game_over = Some(game_state.clone());
            let winning_points = winning_points(&room.board, position, color)
                .into_iter()
                .map(Coordinate::from)
                .collect::<Vec<_>>();
            Some(serde_json::json!([
                game_state,
                room.moves_history_string(),
                winning_points,
            ]))
        } else if room.moves.len() >= BOARD_SIZE * BOARD_SIZE {
            room.started = false;
            room.game_over = Some(GameState::Draw);
            Some(serde_json::json!([
                GameState::Draw,
                room.moves_history_string(),
                Vec::<Coordinate>::new(),
            ]))
        } else {
            None
        };

        (move_message, game_over_message)
    };

    broadcast_to_room(state, &request.room_name, "MovePlayed", move_message).await?;
    if let Some(game_over_message) = game_over_message {
        broadcast_to_room(state, &request.room_name, "GameOver", game_over_message).await?;
    }
    Ok(())
}

async fn leave_room(state: &AppState, connection_id: u64, request: LeaveRoomRequest) -> Result<()> {
    let senders = {
        let mut server = state.state.lock().await;
        let ServerState { rooms, connections } = &mut *server;
        let room = rooms
            .get_mut(&request.room_name)
            .ok_or_else(|| anyhow!("room not found"))?;
        room.members.retain(|member| member.connection_id != connection_id);
        if let Some(connection) = connections.get_mut(&connection_id) {
            connection.room_name = None;
        }
        room.members.iter()
            .filter_map(|m| connections.get(&m.connection_id))
            .map(|c| c.sender.clone())
            .collect::<Vec<_>>()
    };
    send_broadcast(senders, serde_json::json!({
        "type": 1,
        "target": "PlayerLeft",
        "arguments": [format!("local-{connection_id}")],
    }));
    Ok(())
}

async fn close_room(state: &AppState, connection_id: u64, request: CloseRoomRequest) -> Result<()> {
    let recipients = {
        let mut server = state.state.lock().await;
        let room = server
            .rooms
            .get(&request.room_name)
            .ok_or_else(|| anyhow!("room not found"))?;
        if room.owner_connection_id != connection_id {
            return Err(anyhow!("only the room owner can close the room"));
        }

        let recipients = room
            .members
            .iter()
            .filter_map(|member| server.connections.get(&member.connection_id))
            .map(|connection| connection.sender.clone())
            .collect::<Vec<_>>();
        server.rooms.remove(&request.room_name);
        for connection in server.connections.values_mut() {
            if connection.room_name.as_deref() == Some(&request.room_name) {
                connection.room_name = None;
            }
        }
        recipients
    };

    send_broadcast(
        recipients,
        serde_json::json!({
            "type": 1,
            "target": "RoomClosed",
            "arguments": [RoomClosedReason::ClosedByOwner],
        }),
    );
    Ok(())
}

async fn cleanup_connection(state: &AppState, connection_id: u64) {
    let user_id = format!("local-{connection_id}");

    let notifications = {
        let mut server = state.state.lock().await;
        server.connections.remove(&connection_id);

        let room_names: Vec<String> = server.rooms.keys().cloned().collect();
        let mut notifications: Vec<(Vec<mpsc::UnboundedSender<Message>>, Value)> = Vec::new();
        let mut rooms_to_close: Vec<String> = Vec::new();

        let ServerState { rooms, connections } = &mut *server;
        for room_name in &room_names {
            if let Some(room) = rooms.get_mut(room_name) {
                let was_member = room.members.iter().any(|m| m.connection_id == connection_id);
                if !was_member {
                    continue;
                }

                let is_owner = room.owner_connection_id == connection_id;
                room.members.retain(|m| m.connection_id != connection_id);

                let remaining: Vec<_> = room.members.iter()
                    .filter_map(|m| connections.get(&m.connection_id))
                    .map(|c| c.sender.clone())
                    .collect();

                notifications.push((remaining.clone(), serde_json::json!({
                    "type": 1,
                    "target": "PlayerLeft",
                    "arguments": [user_id],
                })));

                if is_owner {
                    notifications.push((remaining, serde_json::json!({
                        "type": 1,
                        "target": "RoomClosed",
                        "arguments": [RoomClosedReason::RoomOwnerLeft],
                    })));
                    rooms_to_close.push(room_name.clone());
                }
            }
        }

        for room_name in rooms_to_close {
            rooms.remove(&room_name);
        }

        // Remove empty inactive rooms that had no owner-departure event.
        let empty: Vec<String> = rooms.iter()
            .filter_map(|(name, room)| {
                if room.members.is_empty() && !room.started && room.game_over.is_none() {
                    Some(name.clone())
                } else {
                    None
                }
            })
            .collect();
        for room_name in empty {
            rooms.remove(&room_name);
        }

        notifications
    };

    for (senders, message) in notifications {
        send_broadcast(senders, message);
    }
}

async fn broadcast_to_room(
    state: &AppState,
    room_name: &str,
    target: &str,
    arguments: Value,
) -> Result<()> {
    let recipients = {
        let server = state.state.lock().await;
        let Some(room) = server.rooms.get(room_name) else {
            return Ok(());
        };
        room
            .members
            .iter()
            .filter_map(|member| server.connections.get(&member.connection_id))
            .map(|connection| connection.sender.clone())
            .collect::<Vec<_>>()
    };

    send_broadcast(
        recipients,
        serde_json::json!({
            "type": 1,
            "target": target,
            "arguments": arguments,
        }),
    );
    Ok(())
}

async fn send_to_connection(state: &AppState, connection_id: u64, message: Value) -> Result<()> {
    let sender = {
        let server = state.state.lock().await;
        server
            .connections
            .get(&connection_id)
            .map(|connection| connection.sender.clone())
    };

    if let Some(sender) = sender {
        let _ = sender.send(Message::Text(text_frame(message).into()));
    }
    Ok(())
}

fn send_broadcast(recipients: Vec<mpsc::UnboundedSender<Message>>, message: Value) {
    let payload = text_frame(message);
    for sender in recipients {
        let _ = sender.send(Message::Text(payload.clone().into()));
    }
}

fn text_frame(value: Value) -> String {
    let mut payload = value.to_string();
    payload.push(RECORD_SEPARATOR);
    payload
}

async fn set_move_time(
    state: &AppState,
    connection_id: u64,
    request: SetMoveTimeRequest,
) -> Result<()> {
    let new_time = {
        let mut server = state.state.lock().await;
        let room = server
            .rooms
            .get_mut(&request.room_id)
            .ok_or_else(|| anyhow!("room not found"))?;
        if room.owner_connection_id != connection_id {
            return Err(anyhow!("only the room owner can change move time"));
        }
        let seconds = request.seconds.clamp(2, 300);
        room.move_time_seconds = seconds;
        seconds
    };
    broadcast_to_room(state, &request.room_id, "SettingsChanged", serde_json::json!([new_time])).await
}

async fn change_role(
    state: &AppState,
    connection_id: u64,
    request: ChangeRoleRequest,
) -> Result<()> {
    let (user, room_name) = {
        let mut server = state.state.lock().await;
        let room = server
            .rooms
            .get_mut(&request.room_id)
            .ok_or_else(|| anyhow!("room not found"))?;
        if room.started {
            return Err(anyhow!("cannot change role while game is running"));
        }

        let new_kind = match request.new_role {
            Role::Observer => MemberKind::Observer,
            Role::Player => {
                let taken_colors: Vec<Piece> = room.members.iter()
                    .filter(|m| m.connection_id != connection_id)
                    .filter_map(|m| if let MemberKind::Player(c) = m.kind { Some(c) } else { None })
                    .collect();
                let preferred = request.preferred_color.unwrap_or(Piece::Black);
                let color = if !taken_colors.contains(&preferred) {
                    preferred
                } else if !taken_colors.contains(&preferred.opponent()) {
                    preferred.opponent()
                } else {
                    return Err(anyhow!("no player slot available"));
                };
                MemberKind::Player(color)
            }
        };

        if let Some(member) = room.members.iter_mut().find(|m| m.connection_id == connection_id) {
            member.kind = new_kind;
        }

        let username = server.connections.get(&connection_id)
            .map(|c| c.username.clone())
            .unwrap_or_default();
        let user = build_user(connection_id, &username, new_kind);
        (user, request.room_id.clone())
    };
    broadcast_to_room(state, &room_name, "RoleChanged", serde_json::json!([user])).await
}

async fn quit(state: &AppState, connection_id: u64) -> Result<()> {
    let room_name = {
        let server = state.state.lock().await;
        server.connections.get(&connection_id)
            .and_then(|c| c.room_name.clone())
    };
    if let Some(room_name) = room_name {
        leave_room(state, connection_id, LeaveRoomRequest { room_name }).await?;
    }
    Ok(())
}

fn build_user(connection_id: u64, username: &str, kind: MemberKind) -> User {
    User {
        id: format!("local-{connection_id}"),
        username: username.to_string(),
        role: match kind {
            MemberKind::Player(_) => Role::Player,
            MemberKind::Observer => Role::Observer,
        },
        game_info: match kind {
            MemberKind::Player(color) => Some(GameInfo { color }),
            MemberKind::Observer => None,
        },
        user_type: UserType::Guest,
    }
}

fn build_users(members: &[RoomMember], connections: &HashMap<u64, Connection>) -> Vec<User> {
    members
        .iter()
        .filter_map(|member| {
            let connection = connections.get(&member.connection_id)?;
            Some(build_user(member.connection_id, &connection.username, member.kind))
        })
        .collect()
}

fn now_utc_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Micros, true)
}

fn board_to_grid(board: &Board) -> Vec<Vec<Option<Piece>>> {
    let mut grid = Vec::with_capacity(BOARD_SIZE);
    for row in 0..BOARD_SIZE {
        let mut output_row = Vec::with_capacity(BOARD_SIZE);
        for column in 0..BOARD_SIZE {
            output_row.push(board.get(Position::new(row, column)));
        }
        grid.push(output_row);
    }
    grid
}

fn parse_moves_history(history: &str) -> Result<Vec<Move>> {
    let mut moves = Vec::new();
    for entry in history
        .split(';')
        .filter(|entry| !entry.trim().is_empty() && *entry != "!")
    {
        let parts = entry.split(':').collect::<Vec<_>>();
        if parts.len() != 4 {
            return Err(anyhow!("invalid moves history entry: {entry}"));
        }
        let color = match parts[1] {
            "Black" => Piece::Black,
            "White" => Piece::White,
            other => return Err(anyhow!("invalid piece color: {other}")),
        };
        let row = parts[2]
            .parse::<usize>()
            .context("invalid row in moves history")?;
        let column = parts[3]
            .parse::<usize>()
            .context("invalid column in moves history")?;
        moves.push(Move { row, column, color });
    }
    Ok(moves)
}

fn next_turn(moves: &[Move]) -> Piece {
    moves
        .last()
        .map(|mv| mv.color.opponent())
        .unwrap_or(Piece::Black)
}

fn winning_points(board: &Board, position: Position, color: Piece) -> Vec<Position> {
    const DIRECTIONS: &[(isize, isize)] = &[(1, 0), (0, 1), (1, 1), (1, -1)];

    for (row_delta, column_delta) in DIRECTIONS {
        let mut line = vec![position];

        let mut row = position.row as isize + row_delta;
        let mut column = position.column as isize + column_delta;
        while same_color(board, row, column, color) {
            line.push(Position::new(row as usize, column as usize));
            row += row_delta;
            column += column_delta;
        }

        let mut prefix = Vec::new();
        let mut row = position.row as isize - row_delta;
        let mut column = position.column as isize - column_delta;
        while same_color(board, row, column, color) {
            prefix.push(Position::new(row as usize, column as usize));
            row -= row_delta;
            column -= column_delta;
        }

        if prefix.len() + line.len() >= 5 {
            prefix.reverse();
            prefix.extend(line);
            return prefix;
        }
    }

    vec![position]
}

fn same_color(board: &Board, row: isize, column: isize, color: Piece) -> bool {
    if row < 0 || column < 0 {
        return false;
    }

    board.get(Position::new(row as usize, column as usize)) == Some(color)
}

impl Room {
    fn moves_history_string(&self) -> String {
        let mut history = String::new();
        for (index, mv) in self.moves.iter().enumerate() {
            history.push_str(&format!("{index}:{:?}:{}:{};", mv.color, mv.row, mv.column));
        }
        history.push('!');
        history
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_moves_history() {
        let moves = parse_moves_history("0:Black:9:9;1:White:9:10;!").unwrap();
        assert_eq!(moves.len(), 2);
        assert_eq!(moves[0], Move { row: 9, column: 9, color: Piece::Black });
        assert_eq!(moves[1], Move { row: 9, column: 10, color: Piece::White });
    }

    #[test]
    fn next_turn_after_moves() {
        let moves = vec![Move { row: 0, column: 0, color: Piece::Black }];
        assert_eq!(next_turn(&moves), Piece::White);
        assert_eq!(next_turn(&[]), Piece::Black);
    }
}
