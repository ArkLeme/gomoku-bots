use crate::engine::DecisionPlan;
use crate::game::GameSnapshot;

use super::{frontier, pattern, search, tactical, StrategyKind};

#[derive(Clone, Debug, Default)]
pub struct StrategyRouter;

impl StrategyRouter {
    pub fn choose(&self, snapshot: &GameSnapshot) -> Option<DecisionPlan> {
        let insight = snapshot.board.inspect(snapshot.my_color);

        if insight.self_winning_moves > 0 || insight.opponent_winning_moves > 0 {
            return tactical::choose(snapshot);
        }

        if snapshot.board.move_count() < 4 {
            return pattern::choose(snapshot);
        }

        if insight.best_self_score >= 50_000 || insight.best_opponent_score >= 50_000 {
            return tactical::choose(snapshot).or_else(|| pattern::choose(snapshot));
        }

        if insight.candidate_count <= 8 {
            return search::choose(snapshot).or_else(|| tactical::choose(snapshot));
        }

        if insight.candidate_count <= 20 {
            return frontier::choose(snapshot).or_else(|| pattern::choose(snapshot));
        }

        pattern::choose(snapshot).or_else(|| tactical::choose(snapshot))
    }
}

impl StrategyRouter {
    pub fn selected_kind(&self, snapshot: &GameSnapshot) -> StrategyKind {
        self.choose(snapshot)
            .map(|plan| plan.strategy)
            .unwrap_or(StrategyKind::Pattern)
    }
}
