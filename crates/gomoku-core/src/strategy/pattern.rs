use crate::board::{Position, SCORE_OPEN_THREE};
use crate::game::GameSnapshot;

use super::{build_plan, position_score, Strategy, StrategyKind};

#[derive(Clone, Debug, Default)]
pub struct PatternStrategy;

impl PatternStrategy {
    pub fn choose(snapshot: &GameSnapshot) -> Option<(Position, i32, String)> {
        let mut candidates = snapshot.board.candidate_positions();
        candidates.sort_unstable_by_key(|position| -position_score(snapshot, *position));

        candidates.into_iter().take(12).fold(None, |best, position| {
            let score = position_score(snapshot, position);
            let reason = if score >= SCORE_OPEN_THREE {
                "strong pattern"
            } else if score >= 2_000 {
                "solid extension"
            } else {
                "positional improvement"
            };

            match best {
                Some((_, best_score, _)) if best_score >= score => best,
                _ => Some((position, score, reason.to_string())),
            }
        })
    }
}

impl Strategy for PatternStrategy {
    fn name(&self) -> &'static str {
        "pattern"
    }

    fn select_move(&self, snapshot: &GameSnapshot) -> Option<Position> {
        Self::choose(snapshot).map(|(position, _, _)| position)
    }
}

pub(super) fn choose(snapshot: &GameSnapshot) -> Option<crate::engine::DecisionPlan> {
    PatternStrategy::choose(snapshot)
        .map(|(position, score, reason)| build_plan(snapshot, StrategyKind::Pattern, position, score, reason))
}
