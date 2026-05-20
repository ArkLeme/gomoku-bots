use anyhow::{Context, Result};
use gomoku_core::board::{Board, Move, Piece, Position};
use gomoku_core::engine::DecisionPlan;
use gomoku_core::game::{GameSnapshot, SessionContext};
use gomoku_core::protocol::{GameState, JoinRoomResponse, Role, User};
use gomoku_core::strategy::{
    PatternStrategy, SearchStrategy, Strategy, StrategyKind, TacticalStrategy, VcfStrategy,
    VctStrategy,
};
use serde_json::Value;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::{watch, Mutex};

use crate::args::StrategyChoice;
use crate::client::{HubEvent, SignalRClient};
use crate::ui_state::{
    board_to_grid, compute_candidates, finalize_candidate_merged_scores, piece_name,
    update_candidates_with_search_scores, CandidateMove, GamePhase, LastDecision, UiState,
};
use gomoku_core::protocol::ClientCommand;

const SAFETY_BUFFER_MS: u64 = 300;

#[derive(Debug)]
enum Step {
    Continue,
    Stop,
}

struct PonderInfo {
    predicted_pos: Position,
    predicted_at_move_count: usize,
    my_color: Piece,
    result: Arc<std::sync::Mutex<Option<DecisionPlan>>>,
    cancel: Arc<AtomicBool>,
}

#[derive(Clone)]
pub struct Runtime {
    client: SignalRClient,
    events: Arc<Mutex<tokio::sync::mpsc::UnboundedReceiver<HubEvent>>>,
    strategy: StrategyChoice,
    state: Arc<Mutex<RuntimeState>>,
    ui_tx: Option<Arc<watch::Sender<UiState>>>,
    exit_on_game_over: bool,
    ponder: Arc<Mutex<Option<PonderInfo>>>,
    search_state: Arc<std::sync::Mutex<SearchStrategy>>,
}

#[derive(Clone, Debug)]
struct RuntimeState {
    session: SessionContext,
    my_color: Option<Piece>,
    player_turn: Option<Piece>,
    board: Board,
    candidates: Vec<CandidateMove>,
    game_phase: GamePhase,
    game_result: Option<String>,
    turn_started_at_ms: Option<u64>,
    last_decision: Option<LastDecision>,
    initial_board_moves_history: Option<String>,
    merge_candidate_scores: bool,
    move_history: Vec<Move>,
    initial_move_count: usize,
}

impl RuntimeState {
    fn new(is_room_owner: bool, default_move_time_seconds: u32, strategy: StrategyChoice) -> Self {
        let mut session = SessionContext::new(is_room_owner);
        session.move_time_seconds = Some(default_move_time_seconds);
        Self {
            session,
            my_color: None,
            player_turn: None,
            board: Board::new(),
            candidates: Vec::new(),
            game_phase: GamePhase::Lobby,
            game_result: None,
            turn_started_at_ms: None,
            last_decision: None,
            initial_board_moves_history: None,
            merge_candidate_scores: matches!(strategy, StrategyChoice::Adaptive),
            move_history: Vec::new(),
            initial_move_count: 0,
        }
    }

    fn refresh_candidates(&mut self) {
        self.candidates = self
            .my_color
            .map(|color| compute_candidates(&self.board, color))
            .unwrap_or_default();

        if self.merge_candidate_scores {
            update_candidates_with_search_scores(&mut self.candidates, &[], true);
        }
    }

    fn snapshot(&self) -> Option<GameSnapshot> {
        Some(GameSnapshot::new(
            self.board.clone(),
            self.player_turn?,
            self.my_color?,
        ))
    }

    fn build_ui_state(&self) -> UiState {
        UiState {
            phase: self.game_phase.clone(),
            room_name: self.session.room_name.clone(),
            my_color: self.my_color.map(piece_name),
            player_turn: self.player_turn.map(piece_name),
            move_time_seconds: self.session.move_time_seconds,
            turn_started_at_ms: self.turn_started_at_ms,
            board: board_to_grid(&self.board),
            candidates: self.candidates.clone(),
            opponent_candidates: Vec::new(),
            last_decision: self.last_decision.clone(),
            opponent_last_decision: None,
            game_result: self.game_result.clone(),
            initial_board_moves_history: self.initial_board_moves_history.clone(),
            startup_error: None,
        }
    }
}

impl Runtime {
    pub fn new(
        client: SignalRClient,
        events: tokio::sync::mpsc::UnboundedReceiver<HubEvent>,
        is_room_owner: bool,
        strategy: StrategyChoice,
        ui_tx: Option<Arc<watch::Sender<UiState>>>,
        default_move_time_seconds: u32,
        exit_on_game_over: bool,
    ) -> Self {
        Self {
            client,
            events: Arc::new(Mutex::new(events)),
            strategy,
            state: Arc::new(Mutex::new(RuntimeState::new(is_room_owner, default_move_time_seconds, strategy))),
            ui_tx,
            exit_on_game_over,
            ponder: Arc::new(Mutex::new(None)),
            search_state: Arc::new(std::sync::Mutex::new(SearchStrategy::default())),
        }
    }

    pub async fn init_room(&self, response: JoinRoomResponse, room_name: String) -> Result<()> {
        let mut state = self.state.lock().await;
        state.session.room_name = Some(room_name);

        let data = response.data.context("join response missing room data")?;
        state.my_color = data.user.game_info.map(|info| info.color);
        state.session.move_time_seconds = Some(data.lobby_data.move_time_seconds);
        state.initial_board_moves_history = data.lobby_data.initial_board_moves_history.clone();

        if let Some(moves_history) = data.lobby_data.initial_board_moves_history.as_deref() {
            println!("lobbyData.initialBoardMovesHistory: {moves_history}");
            let (board, player_turn, initial_moves) = board_from_moves_history(moves_history)?;
            state.board = board;
            state.player_turn = Some(player_turn);
            state.initial_move_count = initial_moves.len();
            state.move_history = initial_moves;
        }

        if let Some(game_data) = data.game_data {
            state.player_turn = Some(game_data.player_turn);
            state.board = board_from_grid(game_data.grid);
        }

        state.refresh_candidates();
        self.broadcast(&state);
        Ok(())
    }

    pub async fn run(self) -> Result<()> {
        loop {
            let event = {
                let mut events = self.events.lock().await;
                events.recv().await
            };

            let Some(event) = event else {
                break;
            };

            match self.handle_event(event).await? {
                Step::Continue => {}
                Step::Stop => {
                    if self.exit_on_game_over {
                        std::process::exit(0);
                    }
                    break;
                }
            }
        }

        Ok(())
    }

    async fn handle_event(&self, event: HubEvent) -> Result<Step> {
        match event.target.as_str() {
            "RoomCreated" => {
                if let Some(room_name) = event.arguments.first().and_then(Value::as_str) {
                    let mut state = self.state.lock().await;
                    state.session.room_name = Some(room_name.to_string());
                    self.broadcast(&state);
                }
                Ok(Step::Continue)
            }
            "UserJoined" => {
                self.handle_user_joined(event.arguments).await?;
                Ok(Step::Continue)
            }
            "GameStarted" => {
                self.handle_game_started(event.arguments).await?;
                Ok(Step::Continue)
            }
            "MovePlayed" => {
                self.handle_move_played(event.arguments).await?;
                Ok(Step::Continue)
            }
            "ConnectionError" => {
                let message = event
                    .arguments
                    .first()
                    .and_then(Value::as_str)
                    .unwrap_or("connection error")
                    .to_string();
                Err(anyhow::anyhow!(message))
            }
            "GameOver" => self.handle_game_over(event.arguments).await,
            "PlayerDisqualified" => self.handle_player_disqualified(event.arguments).await,
            "SettingsChanged" => {
                self.handle_settings_changed(event.arguments).await?;
                Ok(Step::Continue)
            }
            "InitialBoardChanged" => {
                self.handle_initial_board_changed(event.arguments).await?;
                Ok(Step::Continue)
            }
            "Error" => {
                let msg = event.arguments.first()
                    .and_then(|v| v["title"].as_str())
                    .unwrap_or("unknown error");
                eprintln!("server error: {msg}");
                Ok(Step::Continue)
            }
            "PlayerLeft" | "RoleChanged" => {
                self.log_event(&event.target, &event.arguments).await;
                Ok(Step::Continue)
            }
            "Timeout" | "RoomClosed" => {
                self.log_event(&event.target, &event.arguments).await;
                Ok(Step::Continue)
            }
            _ => Ok(Step::Continue),
        }
    }

    async fn handle_user_joined(&self, arguments: Vec<Value>) -> Result<()> {
        let users = arguments
            .get(1)
            .cloned()
            .context("missing users list in UserJoined")?;
        let users: Vec<User> = serde_json::from_value(users)?;

        let should_start = {
            let state = self.state.lock().await;
            state.session.is_room_owner && users.iter().filter(|user| user.role == Role::Player).count() == 2
        };

        if should_start {
            let room_name = self.current_room_name().await?;
            let payload = serde_json::json!({ "roomName": room_name });
            let _ = self.client.send_void("StartGame", vec![payload]).await;
        }

        Ok(())
    }

    async fn handle_game_started(&self, arguments: Vec<Value>) -> Result<()> {
        let player_turn: Piece = serde_json::from_value(
            arguments.first().cloned().context("missing player turn in GameStarted")?,
        )?;

        {
            let mut state = self.state.lock().await;
            state.player_turn = Some(player_turn);
            state.game_phase = GamePhase::Playing;
            state.turn_started_at_ms = Some(now_ms());
            state.refresh_candidates();
            self.broadcast(&state);
        }

        self.try_play_next_move().await?;
        Ok(())
    }

    async fn handle_move_played(&self, arguments: Vec<Value>) -> Result<()> {
        let move_played: Move = serde_json::from_value(
            arguments.first().cloned().context("missing move payload")?,
        )?;

        {
            let mut state = self.state.lock().await;
            state.board.apply_move(move_played).context("failed to apply move")?;
            state.move_history.push(move_played);
            state.player_turn = Some(move_played.color.opponent());
            state.turn_started_at_ms = Some(now_ms());
            state.refresh_candidates();
            self.broadcast(&state);
        }

        self.try_play_next_move().await?;
        Ok(())
    }

    async fn handle_game_over(&self, arguments: Vec<Value>) -> Result<Step> {
        // Cancel any in-flight ponder on game end.
        if let Some(info) = self.ponder.lock().await.take() {
            info.cancel.store(true, Ordering::Relaxed);
        }

        let result_str = if let Some(val) = arguments.first() {
            let parsed: GameState = serde_json::from_value(val.clone())?;
            println!("game over: {:?}", parsed);
            serde_json::to_value(&parsed)
                .ok()
                .and_then(|v| v.as_str().map(String::from))
                .unwrap_or_else(|| format!("{parsed:?}"))
        } else {
            "Unknown".to_string()
        };

        {
            let mut state = self.state.lock().await;
            state.game_phase = GamePhase::GameOver;
            state.game_result = Some(result_str);
            state.turn_started_at_ms = None;
            let n_initial = state.initial_move_count;
            let n_game = state.move_history.len().saturating_sub(n_initial);
            if n_initial > 0 {
                println!("initial board ({n_initial} stones):");
                for mv in state.move_history.iter().take(n_initial) {
                    println!("  {:5} ({:2}, {:2})", format!("{:?}", mv.color), mv.row, mv.column);
                }
            }
            println!("game moves ({n_game} moves):");
            for (i, mv) in state.move_history.iter().skip(n_initial).enumerate() {
                println!("  {:2}. {:5} ({:2}, {:2})", i + 1, format!("{:?}", mv.color), mv.row, mv.column);
            }
            self.broadcast(&state);
        }

        if let Ok(room_name) = self.current_room_name().await {
            let cmd = ClientCommand::LeaveRoom { room_name };
            if let Err(error) = self.client.send_command(&cmd).await {
                eprintln!("failed to leave room after game over: {error:#}");
            }
        }

        let is_room_owner = {
            let state = self.state.lock().await;
            state.session.is_room_owner
        };

        if is_room_owner {
            if let Ok(room_name) = self.current_room_name().await {
                let cmd = ClientCommand::CloseRoom { room_name };
                if let Err(error) = self.client.send_command(&cmd).await {
                    eprintln!("failed to close room after game over: {error:#}");
                }
            }
        }

        // Give the UI time to receive the final state.
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        Ok(Step::Stop)
    }

    async fn try_play_next_move(&self) -> Result<()> {
        // Extract snapshot and timing in a brief lock window.
        let (snapshot, move_time_ms, turn_started_at_ms) = {
            let state = self.state.lock().await;
            let Some(snapshot) = state.snapshot() else { return Ok(()); };
            if snapshot.player_turn != snapshot.my_color {
                return Ok(());
            }
            let move_time_ms = state.session.move_time_seconds.unwrap_or(5) as u64 * 1000;
            let turn_started_at_ms = state.turn_started_at_ms.unwrap_or_else(now_ms);
            (snapshot, move_time_ms, turn_started_at_ms)
        };

        // Cancel any in-flight ponder and check for a cache hit.
        let ponder_plan = {
            let mut ponder_guard = self.ponder.lock().await;
            if let Some(info) = ponder_guard.take() {
                info.cancel.store(true, Ordering::Relaxed);
                check_ponder_hit(&info, &snapshot.board)
            } else {
                None
            }
        };

        // Budget: elapsed since the turn started, minus the safety reserve.
        let elapsed_ms = now_ms().saturating_sub(turn_started_at_ms);
        let remaining_ms = move_time_ms
            .saturating_sub(elapsed_ms)
            .saturating_sub(SAFETY_BUFFER_MS);
        // Guarantee at least 1ms so search_timed always returns its greedy depth-0 pick.
        let search_ms = remaining_ms.max(1);

        let strategy = self.strategy;
        let cancel = Arc::new(AtomicBool::new(false));
        let cancel_clone = Arc::clone(&cancel);
        let deadline = Instant::now() + Duration::from_millis(search_ms);
        let snapshot_clone = snapshot.clone();
        let search_state = Arc::clone(&self.search_state);

        // Stream each completed search depth to the UI in real time.
        let (progress_tx, mut progress_rx) =
            tokio::sync::mpsc::unbounded_channel::<(Position, i32, String, usize, Vec<(Position, i32)>)>();
        let state_for_progress = Arc::clone(&self.state);
        let ui_tx_for_progress = self.ui_tx.clone();
        let progress_task = tokio::spawn(async move {
            while let Some((pos, score, reason, depth, all_scores)) = progress_rx.recv().await {
                let mut state = state_for_progress.lock().await;
                let merge_candidate_scores = state.merge_candidate_scores;
                state.last_decision = Some(LastDecision {
                    strategy: format!("search d={depth}"),
                    row: pos.row,
                    column: pos.column,
                    score,
                    reason,
                });
                update_candidates_with_search_scores(&mut state.candidates, &all_scores, merge_candidate_scores);
                if let Some(tx) = &ui_tx_for_progress {
                    let _ = tx.send(state.build_ui_state());
                }
            }
        });

        let computed = tokio::task::spawn_blocking(move || {
            timed_select(&snapshot_clone, strategy, deadline, &cancel_clone, &search_state, move |pos, score, reason, depth, all_scores| {
                let _ = progress_tx.send((pos, score, reason.to_string(), depth, all_scores.to_vec()));
            })
        })
        .await
        .unwrap_or(None);

        // Wait for the progress task to drain any remaining channel messages.
        progress_task.await.ok();

        let Some(plan) = computed.or(ponder_plan) else {
            return Ok(());
        };

        {
            let mut state = self.state.lock().await;
            state.last_decision = Some(LastDecision {
                strategy: plan.strategy.label().to_string(),
                row: plan.position.row,
                column: plan.position.column,
                score: plan.score,
                reason: plan.reason.clone(),
            });
            finalize_candidate_merged_scores(
                &mut state.candidates,
                plan.position.row,
                plan.position.column,
            );
            self.broadcast(&state);
        }

        let room_name = self.current_room_name().await?;
        let cmd = ClientCommand::PlayMove {
            room_name,
            row: plan.position.row,
            column: plan.position.column,
        };
        println!("selected strategy: {}", plan.strategy.label());
        self.client.send_command(&cmd).await?;

        // Build the board as it will be after our move lands, then start pondering.
        let mut board_after = snapshot.board.clone();
        let _ = board_after.place(plan.position, snapshot.my_color);
        self.start_pondering(board_after, snapshot.my_color, move_time_ms).await;

        Ok(())
    }

    async fn start_pondering(&self, board_after_our_move: Board, my_color: Piece, move_time_ms: u64) {
        let opponent_color = my_color.opponent();
        let candidates = board_after_our_move.candidate_positions();

        let Some(predicted_pos) = candidates
            .iter()
            .copied()
            .max_by_key(|&pos| board_after_our_move.score_move(pos, opponent_color))
        else {
            return;
        };

        let mut ponder_board = board_after_our_move.clone();
        if ponder_board.place(predicted_pos, opponent_color).is_err() {
            return;
        }

        let predicted_at_move_count = board_after_our_move.move_count();
        let result: Arc<std::sync::Mutex<Option<DecisionPlan>>> = Arc::new(std::sync::Mutex::new(None));
        let cancel: Arc<AtomicBool> = Arc::new(AtomicBool::new(false));

        let result_clone = Arc::clone(&result);
        let cancel_clone = Arc::clone(&cancel);
        // Use 90% of a full turn budget for pondering.
        let ponder_deadline = Instant::now() + Duration::from_millis(move_time_ms * 9 / 10);
        let strategy = self.strategy;
        let ponder_snapshot = GameSnapshot::new(ponder_board, my_color, my_color);
        let search_state = Arc::clone(&self.search_state);

        // Stream ponder search progress to keep the heatmap live during the opponent's turn.
        let (ponder_progress_tx, mut ponder_progress_rx) =
            tokio::sync::mpsc::unbounded_channel::<(Position, i32, String, usize, Vec<(Position, i32)>)>();
        let state_for_ponder = Arc::clone(&self.state);
        let ui_tx_for_ponder = self.ui_tx.clone();
        tokio::spawn(async move {
            while let Some((_, _, _, _, all_scores)) = ponder_progress_rx.recv().await {
                let mut state = state_for_ponder.lock().await;
                let merge_candidate_scores = state.merge_candidate_scores;
                update_candidates_with_search_scores(&mut state.candidates, &all_scores, merge_candidate_scores);
                if let Some(tx) = &ui_tx_for_ponder {
                    let _ = tx.send(state.build_ui_state());
                }
            }
        });

        tokio::task::spawn_blocking(move || {
            if let Some(plan) = timed_select(&ponder_snapshot, strategy, ponder_deadline, &cancel_clone, &search_state,
                move |pos, score, reason, depth, all_scores| {
                    let _ = ponder_progress_tx.send((pos, score, reason.to_string(), depth, all_scores.to_vec()));
                }) {
                if let Ok(mut guard) = result_clone.lock() {
                    *guard = Some(plan);
                }
            }
        });

        *self.ponder.lock().await = Some(PonderInfo {
            predicted_pos,
            predicted_at_move_count,
            my_color,
            result,
            cancel,
        });
    }

    async fn handle_player_disqualified(&self, arguments: Vec<Value>) -> Result<Step> {
        let username = arguments.first()
            .and_then(|v| v["username"].as_str())
            .unwrap_or("unknown");
        let reason = arguments.get(1).and_then(Value::as_str).unwrap_or("unknown");
        println!("player disqualified: {username} — {reason}");

        let mut state = self.state.lock().await;
        state.game_phase = GamePhase::GameOver;
        state.game_result = Some(format!("{username} disqualified: {reason}"));
        state.turn_started_at_ms = None;
        self.broadcast(&state);
        Ok(Step::Stop)
    }

    async fn handle_settings_changed(&self, arguments: Vec<Value>) -> Result<()> {
        let new_time: u32 = serde_json::from_value(
            arguments.first().cloned().context("missing move time in SettingsChanged")?,
        )?;
        let mut state = self.state.lock().await;
        state.session.move_time_seconds = Some(new_time);
        println!("move time updated: {new_time}s");
        self.broadcast(&state);
        Ok(())
    }

    async fn handle_initial_board_changed(&self, arguments: Vec<Value>) -> Result<()> {
        let moves_history: String = serde_json::from_value(
            arguments.first().cloned().context("missing moves history in InitialBoardChanged")?,
        )?;
        println!("initial board updated: {moves_history}");
        let (board, player_turn, initial_moves) = board_from_moves_history(&moves_history)?;
        let mut state = self.state.lock().await;
        state.board = board;
        state.player_turn = Some(player_turn);
        state.initial_board_moves_history = Some(moves_history);
        state.initial_move_count = initial_moves.len();
        state.move_history = initial_moves;
        state.refresh_candidates();
        self.broadcast(&state);
        Ok(())
    }

    fn broadcast(&self, state: &RuntimeState) {
        if let Some(tx) = &self.ui_tx {
            let _ = tx.send(state.build_ui_state());
        }
    }

    async fn current_room_name(&self) -> Result<String> {
        self.state
            .lock()
            .await
            .session
            .room_name
            .clone()
            .context("room name not initialized")
    }

    async fn log_event(&self, target: &str, arguments: &[Value]) {
        println!("event {target}: {arguments:?}");
    }
}

fn check_ponder_hit(info: &PonderInfo, board: &Board) -> Option<DecisionPlan> {
    if board.move_count() != info.predicted_at_move_count + 1 {
        return None;
    }
    if board.get(info.predicted_pos) != Some(info.my_color.opponent()) {
        return None;
    }
    info.result.lock().ok()?.clone()
}

fn timed_select<F>(
    snapshot: &GameSnapshot,
    strategy: StrategyChoice,
    deadline: Instant,
    cancel: &AtomicBool,
    search_state: &Arc<std::sync::Mutex<SearchStrategy>>,
    on_depth: F,
) -> Option<DecisionPlan>
where
    F: FnMut(Position, i32, &str, usize, &[(Position, i32)]),
{
    match strategy {
        StrategyChoice::Tactical => TacticalStrategy::default().select_move(snapshot).map(|pos| {
            let score = snapshot.board.score_move(pos, snapshot.my_color);
            DecisionPlan::new(StrategyKind::Tactical, pos, score, String::new())
        }),
        StrategyChoice::Pattern => PatternStrategy::default().select_move(snapshot).map(|pos| {
            let score = snapshot.board.score_move(pos, snapshot.my_color);
            DecisionPlan::new(StrategyKind::Pattern, pos, score, String::new())
        }),
        StrategyChoice::Vcf => VcfStrategy::choose(snapshot)
            .map(|(pos, score, reason)| DecisionPlan::new(StrategyKind::Vcf, pos, score, reason))
            .or_else(|| {
                PatternStrategy::default().select_move(snapshot).map(|pos| {
                    let score = snapshot.board.score_move(pos, snapshot.my_color);
                    DecisionPlan::new(StrategyKind::Pattern, pos, score, "vcf-fallback".to_string())
                })
            }),
        StrategyChoice::Vct => VctStrategy::choose(snapshot)
            .map(|(pos, score, reason)| DecisionPlan::new(StrategyKind::Vct, pos, score, reason))
            .or_else(|| {
                PatternStrategy::default().select_move(snapshot).map(|pos| {
                    let score = snapshot.board.score_move(pos, snapshot.my_color);
                    DecisionPlan::new(StrategyKind::Pattern, pos, score, "vct-fallback".to_string())
                })
            }),
        StrategyChoice::Search => {
            search_state.lock().unwrap()
                .search_timed(snapshot, deadline, cancel, on_depth)
                .map(|(pos, score, reason)| DecisionPlan::new(StrategyKind::Search, pos, score, reason))
        }
        StrategyChoice::Adaptive => {
            // Instant win/block.
            let self_wins = snapshot.board.winning_moves(snapshot.my_color);
            if !self_wins.is_empty() {
                let pos = self_wins[0];
                let score = snapshot.board.score_move(pos, snapshot.my_color);
                return Some(DecisionPlan::new(StrategyKind::Tactical, pos, score, "win".to_string()));
            }
            let opp_wins = snapshot.board.winning_moves(snapshot.my_color.opponent());
            if !opp_wins.is_empty() {
                let pos = opp_wins[0];
                let score = snapshot.board.score_move(pos, snapshot.my_color);
                return Some(DecisionPlan::new(StrategyKind::Tactical, pos, score, "block".to_string()));
            }
            // Opening: prefer well-studied pattern moves.
            if snapshot.board.move_count() < 4 {
                return PatternStrategy::default().select_move(snapshot).map(|pos| {
                    let score = snapshot.board.score_move(pos, snapshot.my_color);
                    DecisionPlan::new(StrategyKind::Pattern, pos, score, String::new())
                });
            }
            // Pre-empt open-four threats the search might miss under time pressure.
            {
                let candidates = snapshot.board.candidate_positions();
                let best_self = candidates.iter()
                    .map(|&p| snapshot.board.score_move(p, snapshot.my_color))
                    .max()
                    .unwrap_or(0);
                if best_self < 100_000 {
                    if let Some(block_pos) = candidates.iter().copied()
                        .filter(|&p| snapshot.board.score_move(p, snapshot.my_color.opponent()) >= 100_000)
                        .max_by_key(|&p| snapshot.board.score_move(p, snapshot.my_color.opponent()))
                    {
                        let score = snapshot.board.score_move(block_pos, snapshot.my_color);
                        return Some(DecisionPlan::new(StrategyKind::Tactical, block_pos, score, "block open four".to_string()));
                    }
                }
            }
            // Mid/endgame: full PVS + TT + VCF search.
            search_state.lock().unwrap()
                .search_timed(snapshot, deadline, cancel, on_depth)
                .map(|(pos, score, reason)| DecisionPlan::new(StrategyKind::Search, pos, score, reason))
        }
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn board_from_grid(grid: Vec<Vec<Option<Piece>>>) -> Board {
    let mut board = Board::new();
    for (row_index, row) in grid.into_iter().enumerate() {
        for (column_index, cell) in row.into_iter().enumerate() {
            if let Some(piece) = cell {
                let _ = board.place(Position::new(row_index, column_index), piece);
            }
        }
    }
    board
}

fn board_from_moves_history(moves_history: &str) -> Result<(Board, Piece, Vec<Move>)> {
    let mut indexed = Vec::new();

    for entry in moves_history.split(';') {
        let entry = entry.trim().trim_end_matches('!');
        if entry.is_empty() {
            continue;
        }

        let parts: Vec<&str> = entry.split(':').collect();
        if parts.len() != 4 {
            return Err(anyhow::anyhow!("invalid initial board entry: {entry}"));
        }

        let index = parts[0]
            .parse::<usize>()
            .with_context(|| format!("invalid move index in initial board entry: {entry}"))?;
        let color = parse_piece(parts[1])?;
        let row = parts[2]
            .parse::<usize>()
            .with_context(|| format!("invalid row in initial board entry: {entry}"))?;
        let column = parts[3]
            .parse::<usize>()
            .with_context(|| format!("invalid column in initial board entry: {entry}"))?;

        indexed.push((index, Move { row, column, color }));
    }

    indexed.sort_by_key(|(index, _)| *index);

    let moves: Vec<Move> = indexed.into_iter().map(|(_, mv)| mv).collect();
    let mut board = Board::new();
    for &move_played in &moves {
        board.apply_move(move_played).context("failed to apply initial board move")?;
    }

    let next_turn = if board.move_count() % 2 == 0 {
        Piece::Black
    } else {
        Piece::White
    };

    Ok((board, next_turn, moves))
}

fn parse_piece(value: &str) -> Result<Piece> {
    match value {
        "Black" => Ok(Piece::Black),
        "White" => Ok(Piece::White),
        other => Err(anyhow::anyhow!("invalid piece in initial board entry: {other}")),
    }
}

#[cfg(test)]
mod tests {
    use super::board_from_moves_history;
    use gomoku_core::board::Piece;

    #[test]
    fn parses_initial_board_history_and_infers_next_turn() {
        let (board, next_turn, moves) = board_from_moves_history("0:Black:0:0;1:Black:17:17;2:White:17:0;3:White:0:17;4:Black:7:7;5:White:9:10;6:Black:9:8;7:White:8:9;8:Black:9:9;9:White:8:8;!").unwrap();

        assert_eq!(board.move_count(), 10);
        assert_eq!(next_turn, Piece::Black);
        assert_eq!(moves.len(), 10);
    }
}
