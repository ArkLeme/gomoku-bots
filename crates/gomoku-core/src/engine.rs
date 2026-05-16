use crate::board::{Move, Position};
use crate::game::GameSnapshot;
use crate::strategy::{Strategy, StrategyKind, StrategyRouter};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DecisionPlan {
    pub strategy: StrategyKind,
    pub position: Position,
    pub score: i32,
    pub reason: String,
}

#[derive(Clone, Debug)]
pub struct DecisionEngine {
    router: StrategyRouter,
}

impl Default for DecisionEngine {
    fn default() -> Self {
        Self {
            router: StrategyRouter::default(),
        }
    }
}

impl DecisionEngine {
    pub fn choose_plan(&self, snapshot: &GameSnapshot) -> Option<DecisionPlan> {
        self.router.choose(snapshot)
    }

    pub fn choose_move(&self, snapshot: &GameSnapshot) -> Option<Move> {
        self.choose_plan(snapshot).map(|plan| Move {
            row: plan.position.row,
            column: plan.position.column,
            color: snapshot.my_color,
        })
    }
}

impl Strategy for DecisionEngine {
    fn name(&self) -> &'static str {
        "decision-engine"
    }

    fn select_move(&self, snapshot: &GameSnapshot) -> Option<Position> {
        self.choose_plan(snapshot).map(|plan| plan.position)
    }
}

impl DecisionPlan {
    pub fn new(strategy: StrategyKind, position: Position, score: i32, reason: impl Into<String>) -> Self {
        Self {
            strategy,
            position,
            score,
            reason: reason.into(),
        }
    }
}
