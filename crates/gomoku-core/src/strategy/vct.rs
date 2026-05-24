/// VCT — Victory by Consecutive Threats
///
/// Extends VCF by allowing the attacker to also play open-three threats
/// (score ≥ 8 000). While an open-three doesn't force a single-move
/// block, combining two simultaneous threats (a fork) means the defender
/// can only stop one — letting the attacker escalate the other into a win.
///
/// The algorithm:
///   1. Fast-path: try VCF first.
///   2. Enumerate "threat moves" (score_move ≥ VCT_THREAT_THRESHOLD).
///   3. For each threat move, check for double-four (≥2 winning threats →
///      unblockable) or a four+open-three fork (forced block leaves a
///      surviving threat that VCF can convert).
///   4. Recurse to find deeper chains.
use std::time::Instant;

use crate::board::{Board, Piece, Position, SCORE_OPEN_THREE};
use crate::game::GameSnapshot;

use super::vcf::{VcfStrategy, VCF_MAX_DEPTH};
use super::{threat_candidates, PatternStrategy, Strategy};

/// Minimum score for "creates an open-three or better".
const VCT_THREAT_THRESHOLD: i32 = SCORE_OPEN_THREE;

/// Maximum attacker-ply depth explored by the VCT search.
pub const VCT_MAX_DEPTH: usize = 30;

/// Deadline abstraction: either no deadline (`None`) or a wall-clock cutoff (`At`).
#[derive(Clone, Copy)]
enum Dl {
    None,
    At(Instant),
}

impl Dl {
    fn expired(self) -> bool {
        matches!(self, Dl::At(t) if Instant::now() >= t)
    }
}

#[derive(Clone, Debug, Default)]
pub struct VctStrategy;

impl VctStrategy {
    /// Returns the first attacker move of a forced VCT win, or `None`.
    pub fn find_win(board: &mut Board, attacker: Piece, max_depth: usize) -> Option<Position> {
        Self::find_win_dl(board, attacker, max_depth, Dl::None)
    }

    /// Like `find_win` but aborts if `deadline` is reached.
    pub fn find_win_timed(
        board: &mut Board,
        attacker: Piece,
        max_depth: usize,
        deadline: Instant,
    ) -> Option<Position> {
        Self::find_win_dl(board, attacker, max_depth, Dl::At(deadline))
    }

    fn find_win_dl(board: &mut Board, attacker: Piece, max_depth: usize, dl: Dl) -> Option<Position> {
        // Fast path: VCF is a strict subset of VCT.
        let vcf_result = match dl {
            Dl::None => VcfStrategy::find_win(board, attacker, VCF_MAX_DEPTH),
            Dl::At(deadline) => VcfStrategy::find_win_timed(board, attacker, VCF_MAX_DEPTH, deadline),
        };
        if let Some(mv) = vcf_result {
            return Some(mv);
        }

        if max_depth == 0 || dl.expired() {
            return None;
        }

        let threat_moves = threat_candidates(board, attacker, VCT_THREAT_THRESHOLD);

        for (mv, _) in threat_moves {
            if dl.expired() {
                break;
            }

            board.place(mv, attacker).ok()?;

            // Immediate five-in-a-row.
            if board.would_win(mv, attacker) {
                board.undo(mv);
                return Some(mv);
            }

            let four_count = board.count_winning_moves_up_to(attacker, 2);
            let opp_has_win = board.has_winning_move(attacker.opponent());

            // Double-four: two unblockable winning threats.
            if four_count >= 2 && !opp_has_win {
                board.undo(mv);
                return Some(mv);
            }

            // Four + open-three fork: force defender to block the four, then
            // check whether the surviving threat allows a VCF win.
            if four_count == 1 && !opp_has_win {
                let forced_block = board.first_winning_move(attacker).unwrap();
                board.place(forced_block, attacker.opponent()).ok();

                let surviving_threats = threat_candidates(board, attacker, VCT_THREAT_THRESHOLD).len();

                let wins = if surviving_threats > 0 {
                    let vcf_ok = match dl {
                        Dl::None => VcfStrategy::find_win(board, attacker, VCF_MAX_DEPTH).is_some(),
                        Dl::At(deadline) => VcfStrategy::find_win_timed(board, attacker, VCF_MAX_DEPTH, deadline).is_some(),
                    };
                    vcf_ok || vct_recurse_dl(board, attacker, 2, max_depth, dl)
                } else {
                    vct_recurse_dl(board, attacker, 2, max_depth, dl)
                };

                board.undo(forced_block);
                board.undo(mv);

                if wins {
                    return Some(mv);
                }
                continue;
            }

            board.undo(mv);
        }

        None
    }

    /// Returns `(position, score, reason)` for the best VCT move, or `None`.
    ///
    /// Also checks whether the opponent has a VCT win and returns a blocking
    /// move in that case.
    pub fn choose(snapshot: &GameSnapshot) -> Option<(Position, i32, String)> {
        let attacker = snapshot.my_color;
        let mut board = snapshot.board.clone();

        if let Some(mv) = Self::find_win(&mut board, attacker, VCT_MAX_DEPTH) {
            return Some((mv, 950_000, "vct forced win".to_string()));
        }

        // Check if opponent has a VCT win and block the first move.
        let mut board = snapshot.board.clone();
        if let Some(mv) = Self::find_win(&mut board, attacker.opponent(), VCT_MAX_DEPTH) {
            return Some((mv, 850_000, "vct block".to_string()));
        }

        None
    }
}

// ── internal helpers ──────────────────────────────────────────────────────────

/// Recursive VCT helper. Returns `true` if the attacker has a forced win from
/// the current board position.
///
/// `depth` counts consumed plies (increments by 2 per attacker+defender pair).
/// `dl` carries the optional deadline; `Dl::None` means no time limit.
fn vct_recurse_dl(board: &mut Board, attacker: Piece, depth: usize, max_depth: usize, dl: Dl) -> bool {
    if depth >= max_depth || dl.expired() {
        return false;
    }

    // Fast path at each node.
    let vcf_ok = match dl {
        Dl::None => VcfStrategy::find_win(board, attacker, VCF_MAX_DEPTH).is_some(),
        Dl::At(deadline) => VcfStrategy::find_win_timed(board, attacker, VCF_MAX_DEPTH, deadline).is_some(),
    };
    if vcf_ok {
        return true;
    }

    let threat_moves = threat_candidates(board, attacker, VCT_THREAT_THRESHOLD);

    for (mv, _) in threat_moves {
        if dl.expired() {
            break;
        }

        if board.place(mv, attacker).is_err() {
            continue;
        }

        if board.would_win(mv, attacker) {
            board.undo(mv);
            return true;
        }

        let four_count = board.count_winning_moves_up_to(attacker, 2);
        let opp_has_win = board.has_winning_move(attacker.opponent());

        if four_count >= 2 && !opp_has_win {
            board.undo(mv);
            return true;
        }

        if four_count == 1 && !opp_has_win {
            let forced_block = board.first_winning_move(attacker).unwrap();
            if board.place(forced_block, attacker.opponent()).is_ok() {
                let surviving_threats = threat_candidates(board, attacker, VCT_THREAT_THRESHOLD).len();

                let wins = if surviving_threats > 0 {
                    let vcf_inner = match dl {
                        Dl::None => VcfStrategy::find_win(board, attacker, VCF_MAX_DEPTH).is_some(),
                        Dl::At(deadline) => VcfStrategy::find_win_timed(board, attacker, VCF_MAX_DEPTH, deadline).is_some(),
                    };
                    vcf_inner || vct_recurse_dl(board, attacker, depth + 2, max_depth, dl)
                } else {
                    vct_recurse_dl(board, attacker, depth + 2, max_depth, dl)
                };

                board.undo(forced_block);
                board.undo(mv);

                if wins {
                    return true;
                }
                continue;
            }
        }

        board.undo(mv);
    }

    false
}

// ── Strategy trait impl ───────────────────────────────────────────────────────

impl Strategy for VctStrategy {
    fn name(&self) -> &'static str {
        "vct"
    }

    fn select_move(&self, snapshot: &GameSnapshot) -> Option<Position> {
        Self::choose(snapshot)
            .map(|(pos, _, _)| pos)
            .or_else(|| PatternStrategy::default().select_move(snapshot))
    }
}

