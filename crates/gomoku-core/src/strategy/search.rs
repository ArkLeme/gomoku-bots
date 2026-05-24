/// SearchV2 — PVS + Transposition Table + killer/history + VCF at leaf nodes.
///
/// All techniques from SearchTt plus:
///  - VCF (Victory by Consecutive Fours) at every leaf node, acting like
///    quiescence search in chess.  The attacker only creates forced fours; the
///    defender has a single legal response; the tree is ~linear so depth 30-40
///    costs < 10 ms.  This detects forced wins that are completely invisible to
///    a fixed-horizon search.
///  - Quick pre-search VCF check before IDDFS begins: if a forced win exists
///    from the root it is returned instantly, preserving the whole time budget
///    for future turns.
///  - Quick opponent VCF check: if the opponent has a VCF win we block the
///    first move of that sequence immediately (tactical priority).
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use crate::board::{Board, Piece, Position, BOARD_SIZE};
use crate::game::GameSnapshot;
use crate::strategy::vcf::{VcfStrategy, VCF_MAX_DEPTH};
use crate::transposition_table::{
    TranspositionTable, TT_FLAG_EXACT, TT_FLAG_LOWER, TT_FLAG_UPPER,
};

use super::{build_plan, position_score, Strategy, StrategyKind};

const ROOT_CANDIDATE_LIMIT: usize = 15;
const DEADLINE_POLL_NODES: u64 = 512;
const MAX_PLY: usize = 32;
const WIN_SCORE: i32 = 1_000_000;
/// Time budget reserved for the pre-IDDFS VCF check.
const VCF_PRE_BUDGET_MS: u64 = 200;
/// VCF depth at leaf nodes (shallower than standalone to bound per-leaf cost).
const LEAF_VCF_DEPTH: usize = 20;

#[derive(Default)]
pub struct SearchStrategy {
    pub tt: TranspositionTable,
}


struct V2Ctx<'a> {
    killers: [[Option<Position>; 2]; MAX_PLY],
    history: [[i32; BOARD_SIZE]; BOARD_SIZE],
    tt: &'a mut TranspositionTable,
    nodes: u64,
    deadline: Instant,
    cancel: &'a AtomicBool,
}

impl<'a> V2Ctx<'a> {
    fn new(tt: &'a mut TranspositionTable, deadline: Instant, cancel: &'a AtomicBool) -> Self {
        Self {
            killers: [[None; 2]; MAX_PLY],
            history: [[0; BOARD_SIZE]; BOARD_SIZE],
            tt,
            nodes: 0,
            deadline,
            cancel,
        }
    }

    fn check_deadline(&self) -> bool {
        self.nodes % DEADLINE_POLL_NODES == 0
            && (Instant::now() >= self.deadline || self.cancel.load(Ordering::Relaxed))
    }

    fn store_killer(&mut self, ply: usize, pos: Position) {
        let ply = ply.min(MAX_PLY - 1);
        if self.killers[ply][0] != Some(pos) {
            self.killers[ply][1] = self.killers[ply][0];
            self.killers[ply][0] = Some(pos);
        }
    }
}

impl SearchStrategy {
    pub fn search_timed<F>(
        &mut self,
        snapshot: &GameSnapshot,
        deadline: Instant,
        cancel: &AtomicBool,
        mut on_depth: F,
    ) -> Option<(Position, i32, String)>
    where
        F: FnMut(Position, i32, &str, usize, &[(Position, i32)]),
    {
        let my_color = snapshot.my_color;
        let opponent = my_color.opponent();

        // --- Fast VCF pre-check (uses a small fraction of the budget) ---
        // Cap to the main deadline so the pre-check never steals time from the IDDFS.
        let vcf_deadline = (Instant::now() + Duration::from_millis(VCF_PRE_BUDGET_MS)).min(deadline);

        // Our own forced win: play immediately.
        {
            let mut board = snapshot.board.clone();
            if let Some(mv) = VcfStrategy::find_win_timed(&mut board, my_color, VCF_MAX_DEPTH, vcf_deadline) {
                return Some((mv, WIN_SCORE - 1, "vcf forced win".to_string()));
            }
        }

        // Opponent's forced win: record the blocking move so it is guaranteed to
        // be in the candidate list and tried first.  We still run IDDFS so that a
        // counter-attack or better block can be found, but this ensures we never
        // drop a critical defensive move off the top-N list.
        let vcf_block: Option<Position> = {
            let mut board = snapshot.board.clone();
            VcfStrategy::find_win_timed(&mut board, opponent, VCF_MAX_DEPTH, vcf_deadline)
        };

        // --- Full IDDFS ---
        let mut candidates = snapshot.board.candidate_positions();
        if candidates.is_empty() {
            return None;
        }

        // Sort by max(self_score, opponent_score) so a cell that's critical for
        // either side is ranked high — prevents pure-offense sort from burying blocks.
        candidates.sort_unstable_by_key(|&p| {
            let mine   = snapshot.board.score_move(p, my_color);
            let theirs = snapshot.board.score_move(p, opponent);
            -(mine.max(theirs) * 2 + mine.min(theirs))
        });
        let mut candidates: Vec<Position> = candidates.into_iter().take(ROOT_CANDIDATE_LIMIT).collect();

        // Inject the VCF blocking move at position 0: even if it scored low
        // offensively it must be evaluated, and trying it first maximises
        // the chance IDDFS confirms it before the deadline.
        if let Some(block) = vcf_block {
            if let Some(idx) = candidates.iter().position(|&p| p == block) {
                candidates.swap(0, idx);
            } else {
                candidates.insert(0, block);
                candidates.truncate(ROOT_CANDIDATE_LIMIT + 1);
            }
        }

        let mut best = Some((candidates[0], position_score(snapshot, candidates[0]), "greedy".to_string()));
        let mut ctx = V2Ctx::new(&mut self.tt, deadline, cancel);

        for depth in 1usize..=16 {
            if Instant::now() >= deadline || cancel.load(Ordering::Relaxed) {
                break;
            }

            match search_root_v2(&snapshot.board, my_color, &candidates, depth, &mut ctx) {
                Ok(Some((pos, score, ref reason, ref all_scores))) => {
                    on_depth(pos, score, reason, depth, all_scores);
                    best = Some((pos, score, reason.clone()));
                }
                Ok(None) => {}
                Err(()) => break,
            }
        }

        best
    }

    pub fn choose(snapshot: &GameSnapshot) -> Option<(Position, i32, String)> {
        let deadline = Instant::now() + Duration::from_millis(4500);
        let cancel = AtomicBool::new(false);
        Self::default().search_timed(snapshot, deadline, &cancel, |_, _, _, _, _| {})
    }
}

fn search_root_v2(
    board: &Board,
    my_color: Piece,
    candidates: &[Position],
    depth: usize,
    ctx: &mut V2Ctx,
) -> Result<Option<(Position, i32, String, Vec<(Position, i32)>)>, ()> {
    let mut board = board.clone();
    let mut best: Option<(Position, i32, String)> = None;
    let mut alpha = i32::MIN / 4;
    let beta = i32::MAX / 4;
    let mut all_scores: Vec<(Position, i32)> = Vec::with_capacity(candidates.len());

    let tt_first = ctx.tt.probe(board.hash).and_then(|e| e.best_move());
    let ordered_root = reorder_with_tt(candidates.to_vec(), tt_first);

    for (i, pos) in ordered_root.iter().enumerate() {
        let pos = *pos;
        if Instant::now() >= ctx.deadline || ctx.cancel.load(Ordering::Relaxed) {
            return Err(());
        }

        let score = if board.would_win(pos, my_color) {
            all_scores.push((pos, WIN_SCORE));
            return Ok(Some((pos, WIN_SCORE, format!("win depth {depth}"), all_scores)));
        } else {
            if board.place(pos, my_color).is_err() {
                continue;
            }
            if i == 0 || depth == 1 {
                let s = -pvs_v2(&mut board, depth.saturating_sub(1), -beta, -alpha, my_color.opponent(), 1, ctx)?;
                board.undo(pos);
                s
            } else {
                let s = -pvs_v2(&mut board, depth.saturating_sub(1), -alpha - 1, -alpha, my_color.opponent(), 1, ctx)?;
                let s = if s > alpha && s < beta {
                    board.undo(pos);
                    if board.place(pos, my_color).is_err() { continue; }
                    let full = -pvs_v2(&mut board, depth.saturating_sub(1), -beta, -alpha, my_color.opponent(), 1, ctx)?;
                    board.undo(pos);
                    full
                } else {
                    board.undo(pos);
                    s
                };
                s
            }
        };

        all_scores.push((pos, score));
        if score > alpha {
            alpha = score;
        }

        let reason = if score >= WIN_SCORE / 2 { "forced win" } else if score >= 50_000 { "strong" } else { "search" };
        match best {
            Some((_, best_score, _)) if best_score >= score => {}
            _ => best = Some((pos, score, reason.to_string())),
        }
    }

    Ok(best.map(|(pos, score, reason)| (pos, score, reason, all_scores)))
}

/// Negamax PVS with TT + VCF leaf extension.
fn pvs_v2(
    board: &mut Board,
    depth: usize,
    mut alpha: i32,
    mut beta: i32,
    color: Piece,
    ply: usize,
    ctx: &mut V2Ctx,
) -> Result<i32, ()> {
    ctx.nodes += 1;
    if ctx.check_deadline() {
        return Err(());
    }

    // --- TT probe ---
    let key = board.hash;
    let tt_move: Option<Position>;
    if let Some(entry) = ctx.tt.probe(key) {
        tt_move = entry.best_move();
        if entry.depth >= depth as u8 {
            match entry.flag {
                TT_FLAG_EXACT => return Ok(entry.score),
                TT_FLAG_LOWER => alpha = alpha.max(entry.score),
                TT_FLAG_UPPER => beta = beta.min(entry.score),
                _ => {}
            }
            if alpha >= beta {
                return Ok(entry.score);
            }
        }
    } else {
        tt_move = None;
    }

    if depth == 0 {
        // VCF quiescence: extend past the horizon if a forced win exists.
        let vcf_deadline = Instant::now() + Duration::from_millis(5);
        let score = vcf_leaf_eval(board, color, vcf_deadline);
        ctx.tt.store(key, 0, score, TT_FLAG_EXACT, None);
        return Ok(score);
    }

    let candidates = board.candidate_positions();
    if candidates.is_empty() {
        return Ok(board.evaluate_for_candidates(color, &candidates) - board.evaluate_for_candidates(color.opponent(), &candidates));
    }

    let ordered = order_moves_v2(candidates, color, ply, board, ctx, tt_move);
    let limit = super::adaptive_inner_limit(
        ordered.first().map(|p| board.score_move(*p, color)).unwrap_or(0),
    );
    let orig_alpha = alpha;
    let mut best_score = i32::MIN / 4;
    let mut best_move: Option<Position> = None;

    for (i, pos) in ordered.into_iter().take(limit).enumerate() {
        let score = if board.would_win(pos, color) {
            WIN_SCORE
        } else {
            if board.place(pos, color).is_err() {
                continue;
            }
            if i == 0 {
                let s = -pvs_v2(board, depth - 1, -beta, -alpha, color.opponent(), ply + 1, ctx)?;
                board.undo(pos);
                s
            } else {
                let s = -pvs_v2(board, depth - 1, -alpha - 1, -alpha, color.opponent(), ply + 1, ctx)?;
                let s = if s > alpha && s < beta {
                    board.undo(pos);
                    if board.place(pos, color).is_err() { continue; }
                    let full = -pvs_v2(board, depth - 1, -beta, -alpha, color.opponent(), ply + 1, ctx)?;
                    board.undo(pos);
                    full
                } else {
                    board.undo(pos);
                    s
                };
                s
            }
        };

        if score > best_score {
            best_score = score;
            best_move = Some(pos);
        }
        if score > alpha {
            alpha = score;
        }
        if alpha >= beta {
            ctx.store_killer(ply, pos);
            ctx.history[pos.row][pos.column] += (depth * depth) as i32;
            break;
        }
    }

    let flag = if best_score <= orig_alpha {
        TT_FLAG_UPPER
    } else if best_score >= beta {
        TT_FLAG_LOWER
    } else {
        TT_FLAG_EXACT
    };
    ctx.tt.store(key, depth as u8, best_score, flag, best_move);

    Ok(best_score)
}

/// Leaf evaluation: static score augmented by a short VCF search.
fn vcf_leaf_eval(board: &mut Board, color: Piece, deadline: Instant) -> i32 {
    // Self VCF win?
    if VcfStrategy::find_win_timed(board, color, LEAF_VCF_DEPTH, deadline).is_some() {
        return WIN_SCORE - 10; // slightly less than horizon win to prefer shorter mates
    }
    // Opponent VCF win?
    if VcfStrategy::find_win_timed(board, color.opponent(), LEAF_VCF_DEPTH, deadline).is_some() {
        return -(WIN_SCORE - 10);
    }
    let candidates = board.candidate_positions();
    board.evaluate_for_candidates(color, &candidates) - board.evaluate_for_candidates(color.opponent(), &candidates)
}

fn order_moves_v2(
    mut candidates: Vec<Position>,
    color: Piece,
    ply: usize,
    board: &Board,
    ctx: &V2Ctx,
    tt_move: Option<Position>,
) -> Vec<Position> {
    let killers = ctx.killers[ply.min(MAX_PLY - 1)];

    candidates.sort_unstable_by(|&a, &b| {
        let sa = move_order_score_v2(a, color, board, &killers, &ctx.history, tt_move);
        let sb = move_order_score_v2(b, color, board, &killers, &ctx.history, tt_move);
        sb.cmp(&sa)
    });
    candidates
}

fn move_order_score_v2(
    pos: Position,
    color: Piece,
    board: &Board,
    killers: &[Option<Position>; 2],
    history: &[[i32; BOARD_SIZE]; BOARD_SIZE],
    tt_move: Option<Position>,
) -> i64 {
    if board.would_win(pos, color) {
        return 100_000_000;
    }
    if board.would_win(pos, color.opponent()) {
        return 90_000_000;
    }
    if tt_move == Some(pos) {
        return 80_000_000;
    }
    if killers[0] == Some(pos) {
        return 10_000_000;
    }
    if killers[1] == Some(pos) {
        return 9_000_000;
    }
    let static_score = board.score_move(pos, color) as i64;
    let defensive = board.score_move(pos, color.opponent()) as i64;
    let hist = history[pos.row][pos.column] as i64;
    static_score + defensive + hist
}

fn reorder_with_tt(mut candidates: Vec<Position>, tt_move: Option<Position>) -> Vec<Position> {
    if let Some(tm) = tt_move {
        if let Some(idx) = candidates.iter().position(|&p| p == tm) {
            candidates.swap(0, idx);
        }
    }
    candidates
}

impl Strategy for SearchStrategy {
    fn name(&self) -> &'static str {
        "search"
    }

    fn select_move(&self, snapshot: &GameSnapshot) -> Option<Position> {
        Self::choose(snapshot).map(|(pos, _, _)| pos)
    }
}

#[allow(dead_code)]
pub(super) fn choose(snapshot: &GameSnapshot) -> Option<crate::engine::DecisionPlan> {
    SearchStrategy::choose(snapshot)
        .map(|(pos, score, reason)| build_plan(snapshot, StrategyKind::Search, pos, score, reason))
}
