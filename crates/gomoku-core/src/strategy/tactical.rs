use crate::board::{Position, SCORE_OPEN_FOUR, SCORE_OPEN_THREE};
use crate::game::GameSnapshot;

use super::{build_plan, position_score, Strategy, StrategyKind};

#[derive(Clone, Debug, Default)]
pub struct TacticalStrategy;

impl TacticalStrategy {
    pub fn analyze(snapshot: &GameSnapshot) -> Option<(Position, i32, String)> {
        let board = &snapshot.board;
        let color = snapshot.my_color;
        let opponent = color.opponent();

        if let Some(position) = board.winning_moves(color).into_iter().next() {
            return Some((position, i32::MAX / 4, String::from("winning move")));
        }

        if let Some(position) = board.winning_moves(opponent).into_iter().next() {
            return Some((position, i32::MAX / 5, String::from("must block opponent win")));
        }

        let candidates = board.candidate_positions();

        // Block opponent's open four before it becomes unblockable.
        // Only skip if we already have our own open four (counter-threat wins faster).
        let my_best_score = candidates.iter()
            .map(|&p| board.score_move(p, color))
            .max()
            .unwrap_or(0);
        if my_best_score < SCORE_OPEN_FOUR {
            if let Some(position) = candidates.iter().copied()
                .filter(|&p| board.score_move(p, opponent) >= SCORE_OPEN_FOUR)
                .max_by_key(|&p| board.score_move(p, opponent))
            {
                return Some((position, i32::MAX / 6, String::from("block open four")));
            }
        }

        let mut best_open_four: Option<(Position, i32, String)> = None;
        for position in candidates {
            if !board.is_empty_at(position) {
                continue;
            }

            let mine = board.score_move(position, color);
            let theirs = board.score_move(position, opponent);
            let score = mine.max(theirs) * 2 + mine.min(theirs);
            let reason = if mine >= SCORE_OPEN_FOUR {
                "open four pressure"
            } else if mine >= SCORE_OPEN_THREE {
                "threat building"
            } else if theirs >= SCORE_OPEN_THREE {
                "block open three"
            } else {
                "local tactical pressure"
            };

            if best_open_four.as_ref().map(|(_, current, _)| score > *current).unwrap_or(true) {
                best_open_four = Some((position, score, reason.to_string()));
            }
        }

        best_open_four
    }
}

impl Strategy for TacticalStrategy {
    fn name(&self) -> &'static str {
        "tactical"
    }

    fn select_move(&self, snapshot: &GameSnapshot) -> Option<Position> {
        Self::analyze(snapshot).map(|(position, _, _)| position)
    }
}

pub(super) fn choose(snapshot: &GameSnapshot) -> Option<crate::engine::DecisionPlan> {
    TacticalStrategy::analyze(snapshot).map(|(position, score, reason)| {
        build_plan(snapshot, StrategyKind::Tactical, position, score + position_score(snapshot, position), reason)
    })
}
