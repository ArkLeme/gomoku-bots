use gomoku_core::board::{Board, Piece, Position, BOARD_SIZE};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum GamePhase {
    Lobby,
    Playing,
    GameOver,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CandidateMove {
    pub row: usize,
    pub column: usize,
    pub score_self: i32,
    pub score_opponent: i32,
    pub normalized_self: f32,
    pub normalized_opponent: f32,
    pub search_score: Option<i32>,
    pub normalized_search: f32,
    /// Unified score across all strategies: max(normalized_self, normalized_search).
    /// Stamped to 1.0 for the chosen move after the decision is finalised.
    pub normalized_merged: f32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LastDecision {
    pub strategy: String,
    pub row: usize,
    pub column: usize,
    pub score: i32,
    pub reason: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UiState {
    pub phase: GamePhase,
    pub room_name: Option<String>,
    pub my_color: Option<String>,
    pub player_turn: Option<String>,
    pub move_time_seconds: Option<u32>,
    pub turn_started_at_ms: Option<u64>,
    pub board: Vec<Vec<Option<String>>>,
    pub candidates: Vec<CandidateMove>,
    pub opponent_candidates: Vec<CandidateMove>,
    pub last_decision: Option<LastDecision>,
    pub opponent_last_decision: Option<LastDecision>,
    pub game_result: Option<String>,
    pub initial_board_moves_history: Option<String>,
    pub startup_error: Option<String>,
}

impl Default for UiState {
    fn default() -> Self {
        Self {
            phase: GamePhase::Lobby,
            room_name: None,
            my_color: None,
            player_turn: None,
            move_time_seconds: None,
            turn_started_at_ms: None,
            board: vec![vec![None; BOARD_SIZE]; BOARD_SIZE],
            candidates: Vec::new(),
            opponent_candidates: Vec::new(),
            last_decision: None,
            opponent_last_decision: None,
            game_result: None,
            initial_board_moves_history: None,
            startup_error: None,
        }
    }
}

impl UiState {
    pub fn startup_error(message: impl Into<String>) -> Self {
        Self {
            startup_error: Some(message.into()),
            ..Self::default()
        }
    }
}

pub fn piece_name(piece: Piece) -> String {
    match piece {
        Piece::Black => "Black".to_string(),
        Piece::White => "White".to_string(),
    }
}

pub fn board_to_grid(board: &Board) -> Vec<Vec<Option<String>>> {
    (0..BOARD_SIZE)
        .map(|row| {
            (0..BOARD_SIZE)
                .map(|col| board.get(Position::new(row, col)).map(piece_name))
                .collect()
        })
        .collect()
}

pub fn compute_candidates(board: &Board, my_color: Piece) -> Vec<CandidateMove> {
    let positions = board.candidate_positions();
    if positions.is_empty() {
        return Vec::new();
    }

    let scored_self: Vec<(Position, i32)> = positions
        .iter()
        .map(|&pos| (pos, board.score_move(pos, my_color)))
        .collect();

    let scored_opponent: Vec<(Position, i32)> = positions
        .iter()
        .map(|&pos| (pos, board.score_move(pos, my_color.opponent())))
        .collect();

    let max_self = scored_self.iter().map(|(_, s)| *s).max().unwrap_or(1).max(1);
    let max_opponent = scored_opponent.iter().map(|(_, s)| *s).max().unwrap_or(1).max(1);

    scored_self
        .iter()
        .zip(scored_opponent.iter())
        .map(|((pos, self_score), (_, opp_score))| {
            let normalized_self = (*self_score as f32 / max_self as f32).clamp(0.0, 1.0);
            CandidateMove {
                row: pos.row,
                column: pos.column,
                score_self: *self_score,
                score_opponent: *opp_score,
                normalized_self,
                normalized_opponent: (*opp_score as f32 / max_opponent as f32).clamp(0.0, 1.0),
                search_score: None,
                normalized_search: 0.0,
                normalized_merged: normalized_self,
            }
        })
        .collect()
}

pub fn update_candidates_with_search_scores(
    candidates: &mut [CandidateMove],
    scores: &[(Position, i32)],
    merge_base_scores: bool,
) {
    for candidate in candidates.iter_mut() {
        let base_score = candidate
            .score_self
            .saturating_mul(3)
            .saturating_sub(candidate.score_opponent);
        let search_score = scores
            .iter()
            .find(|(p, _)| p.row == candidate.row && p.column == candidate.column)
            .map(|(_, score)| *score);

        candidate.search_score = if merge_base_scores {
            Some(base_score.saturating_add(search_score.unwrap_or(0)))
        } else {
            search_score.or(candidate.search_score)
        };
    }

    let Some((min_score, max_score)) = candidates
        .iter()
        .filter_map(|candidate| candidate.search_score)
        .fold(None, |acc, score| match acc {
            None => Some((score, score)),
            Some((min_score, max_score)) => Some((min_score.min(score), max_score.max(score))),
        })
    else {
        return;
    };

    let range = (max_score - min_score).max(1) as f32;
    for candidate in candidates.iter_mut() {
        if let Some(score) = candidate.search_score {
            candidate.normalized_search = ((score - min_score) as f32 / range).clamp(0.0, 1.0);
        }
    }
    for candidate in candidates.iter_mut() {
        candidate.normalized_merged = candidate.normalized_self.max(candidate.normalized_search);
    }
}

/// Stamps the chosen position's `normalized_merged` to 1.0 so it always renders
/// as the highest-rated cell regardless of which sub-strategy selected it.
pub fn finalize_candidate_merged_scores(
    candidates: &mut [CandidateMove],
    chosen_row: usize,
    chosen_col: usize,
) {
    for c in candidates.iter_mut() {
        c.normalized_merged = c.normalized_self.max(c.normalized_search);
    }
    // Lift the chosen position to at least the current maximum so it is always
    // one of the top-ranked candidates, without suppressing any other scores.
    let current_max = candidates
        .iter()
        .map(|c| c.normalized_merged)
        .fold(0.0f32, f32::max);
    if let Some(chosen) = candidates
        .iter_mut()
        .find(|c| c.row == chosen_row && c.column == chosen_col)
    {
        chosen.normalized_merged = chosen.normalized_merged.max(current_max);
    }
}
