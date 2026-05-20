use crate::board::Position;
use crate::game::GameSnapshot;

use super::{build_plan, position_score, Strategy, StrategyKind};

#[derive(Clone, Debug, Default)]
pub struct FrontierStrategy;

impl FrontierStrategy {
    pub fn choose(snapshot: &GameSnapshot) -> Option<(Position, i32, String)> {
        let mut candidates = snapshot.board.candidate_positions();
        candidates.sort_unstable_by_key(|position| -position_score(snapshot, *position));

        candidates.into_iter().take(8).fold(None, |best, position| {
            let score = position_score(snapshot, position);
            let reason = if score >= 15_000 {
                "frontier hot spot"
            } else {
                "frontier candidate"
            };

            match best {
                Some((_, best_score, _)) if best_score >= score => best,
                _ => Some((position, score, reason.to_string())),
            }
        })
    }
}

impl Strategy for FrontierStrategy {
    fn name(&self) -> &'static str {
        "frontier"
    }

    fn select_move(&self, snapshot: &GameSnapshot) -> Option<Position> {
        Self::choose(snapshot).map(|(position, _, _)| position)
    }
}

pub(super) fn choose(snapshot: &GameSnapshot) -> Option<crate::engine::DecisionPlan> {
    // Tagged as Search: frontier is a lightweight search pass over the top-8 candidates,
    // not a pattern move. Pattern is reserved for pure positional/opening decisions.
    FrontierStrategy::choose(snapshot)
        .map(|(position, score, reason)| build_plan(snapshot, StrategyKind::Search, position, score, reason))
}
