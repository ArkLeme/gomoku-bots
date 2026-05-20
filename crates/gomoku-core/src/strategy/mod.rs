use crate::board::{Board, Piece, Position};
use crate::engine::DecisionPlan;
use crate::game::GameSnapshot;

mod frontier;
mod pattern;
mod router;
mod search;
mod tactical;
mod vcf;
mod vct;

#[cfg(test)]
mod tests;

pub use frontier::FrontierStrategy;
pub use pattern::PatternStrategy;
pub use router::StrategyRouter;
pub use search::SearchStrategy;
pub use tactical::TacticalStrategy;
pub use vcf::VcfStrategy;
pub use vct::VctStrategy;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StrategyKind {
    Tactical,
    Pattern,
    Vcf,
    Vct,
    Search,
}

pub trait Strategy {
    fn name(&self) -> &'static str;
    fn select_move(&self, snapshot: &GameSnapshot) -> Option<Position>;
}

impl StrategyKind {
    pub fn label(self) -> &'static str {
        match self {
            StrategyKind::Tactical => "tactical",
            StrategyKind::Pattern => "pattern",
            StrategyKind::Vcf => "vcf",
            StrategyKind::Vct => "vct",
            StrategyKind::Search => "search",
        }
    }
}

/// Adaptive inner candidate limit for search inner nodes.
///
/// At high-threat positions (best move scores ≥ 50 000) the tree is narrow
/// by definition — there are few good replies — so expanding to 15 costs
/// little and avoids missing critical defensive moves.  In quiet positions
/// the tighter limit of 7 keeps the branching factor low and lets IDDFS
/// reach deeper plies within the time budget.
pub(crate) fn adaptive_inner_limit(best_score: i32) -> usize {
    if best_score >= 50_000 { 15 } else { 7 }
}

pub(crate) fn position_score(snapshot: &GameSnapshot, position: Position) -> i32 {
    let board = &snapshot.board;
    let mine = board.score_move(position, snapshot.my_color);
    let theirs = board.score_move(position, snapshot.my_color.opponent());
    mine.max(theirs) * 2 + mine.min(theirs)
}

pub(crate) fn build_plan(_snapshot: &GameSnapshot, strategy: StrategyKind, position: Position, score: i32, reason: impl Into<String>) -> DecisionPlan {
    DecisionPlan::new(strategy, position, score, reason)
}

/// Returns candidate positions with score at or above `threshold`, sorted descending by score.
///
/// Calls `candidate_positions()` exactly once, scores each position for `attacker`,
/// filters by `threshold`, and returns `(Position, score)` pairs.
pub(super) fn threat_candidates(board: &Board, attacker: Piece, threshold: i32) -> Vec<(Position, i32)> {
    let mut result: Vec<(Position, i32)> = board
        .candidate_positions()
        .into_iter()
        .filter_map(|pos| {
            let score = board.score_move(pos, attacker);
            if score >= threshold { Some((pos, score)) } else { None }
        })
        .collect();
    result.sort_unstable_by(|a, b| b.1.cmp(&a.1));
    result
}
