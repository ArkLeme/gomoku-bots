/// VCF — Victory by Consecutive Fours
///
/// Searches for a forced win where the attacker only plays moves that create a
/// Four (closed or open) or better. The defender has exactly one legal response
/// to each Four (block it), so the search tree is nearly linear and can reach
/// depth 30-40 in milliseconds.
///
/// Used both as a standalone strategy and as a leaf-node extension inside the
/// heavier search strategies (search_tt, search_v2).
use std::time::Instant;

use crate::board::{Board, Piece, Position, SCORE_FOUR};
use crate::game::GameSnapshot;

use super::{threat_candidates, Strategy};

/// Score threshold for "creates a four or better".
/// Closed four = SCORE_FOUR, open four = SCORE_OPEN_FOUR, five = SCORE_FIVE.
const FOUR_SCORE_THRESHOLD: i32 = SCORE_FOUR;

/// Maximum attack plies explored by the VCF search.
/// Each attacker move + forced defender block = 2 plies consumed.
pub const VCF_MAX_DEPTH: usize = 40;

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
pub struct VcfStrategy;

impl VcfStrategy {
    /// Returns the first attacker move of a forced-win sequence, or `None`.
    pub fn find_win(board: &mut Board, attacker: Piece, max_depth: usize) -> Option<Position> {
        Self::find_win_dl(board, attacker, max_depth, Dl::None)
    }

    /// Like `find_win` but with a wall-clock deadline to avoid spending too long.
    pub fn find_win_timed(
        board: &mut Board,
        attacker: Piece,
        max_depth: usize,
        deadline: Instant,
    ) -> Option<Position> {
        Self::find_win_dl(board, attacker, max_depth, Dl::At(deadline))
    }

    fn find_win_dl(board: &mut Board, attacker: Piece, max_depth: usize, dl: Dl) -> Option<Position> {
        let candidates = threat_candidates(board, attacker, FOUR_SCORE_THRESHOLD);

        for (mv, _) in candidates {
            if dl.expired() {
                break;
            }
            // Immediate five-in-a-row: win found (check before placing).
            if board.would_win(mv, attacker) {
                return Some(mv);
            }

            board.place(mv, attacker).ok()?;

            // Verify we actually created a threat (filter out high-score false positives
            // that came from summed threes rather than a real four).
            let threat_count = board.count_winning_moves_up_to(attacker, 2);
            if threat_count == 0 {
                board.undo(mv);
                continue;
            }

            // Open four (≥2 threats): can't block both — win, unless defender has an immediate win.
            if threat_count >= 2 {
                let defender_wins = board.has_winning_move(attacker.opponent());
                board.undo(mv);
                if !defender_wins {
                    return Some(mv);
                }
                continue;
            }

            // Exactly one forced block. Recurse (defender-win check is inside vcf_recurse_dl).
            let wins = vcf_recurse_dl(board, attacker, 2, max_depth, dl);
            board.undo(mv);
            if wins {
                return Some(mv);
            }
        }
        None
    }

    pub fn choose(snapshot: &GameSnapshot) -> Option<(Position, i32, String)> {
        let mut board = snapshot.board.clone();
        let attacker = snapshot.my_color;

        if let Some(mv) = Self::find_win(&mut board, attacker, VCF_MAX_DEPTH) {
            return Some((mv, 900_000, "vcf forced win".to_string()));
        }
        // Check if opponent has a VCF win and block the first move of that sequence.
        let mut board = snapshot.board.clone();
        if let Some(mv) = Self::find_win(&mut board, attacker.opponent(), VCF_MAX_DEPTH) {
            return Some((mv, 800_000, "vcf block".to_string()));
        }
        None
    }
}

/// Internal recursive VCF — returns true if attacker has a forced win from here.
/// `depth` counts plies consumed (attacker + defender pairs), increments by 2 each level.
fn vcf_recurse_dl(board: &mut Board, attacker: Piece, depth: usize, max_depth: usize, dl: Dl) -> bool {
    if depth >= max_depth || dl.expired() {
        return false;
    }

    // Defender must block the previous four — their forced move is in `winning_moves`.
    let threat_count = board.count_winning_moves_up_to(attacker, 2);

    if threat_count >= 2 {
        // Open four: attacker wins unless the defender has an immediate winning move of their own.
        return !board.has_winning_move(attacker.opponent());
    }
    if threat_count == 0 {
        return false;
    }

    // One forced block — but the defender plays their own win instead of blocking.
    if board.has_winning_move(attacker.opponent()) {
        return false;
    }
    let block = board.first_winning_move(attacker).unwrap();
    board.place(block, attacker.opponent()).unwrap();

    let four_moves = threat_candidates(board, attacker, FOUR_SCORE_THRESHOLD);

    let mut found = false;
    for (mv, _) in four_moves {
        if dl.expired() {
            break;
        }
        if board.would_win(mv, attacker) {
            found = true;
            break;
        }

        board.place(mv, attacker).unwrap();

        let threats = board.winning_moves(attacker);
        let win = if threats.len() >= 2 {
            true
        } else if threats.is_empty() {
            false
        } else {
            vcf_recurse_dl(board, attacker, depth + 2, max_depth, dl)
        };

        board.undo(mv);
        if win {
            found = true;
            break;
        }
    }

    board.undo(block);
    found
}

impl Strategy for VcfStrategy {
    fn name(&self) -> &'static str {
        "vcf"
    }

    fn select_move(&self, snapshot: &GameSnapshot) -> Option<Position> {
        Self::choose(snapshot).map(|(pos, _, _)| pos)
    }
}

