use crate::board::BOARD_SIZE;
use std::sync::OnceLock;

static ZOBRIST_TABLE: OnceLock<[[[u64; 2]; BOARD_SIZE]; BOARD_SIZE]> = OnceLock::new();

pub fn table() -> &'static [[[u64; 2]; BOARD_SIZE]; BOARD_SIZE] {
    ZOBRIST_TABLE.get_or_init(|| {
        let mut t = [[[0u64; 2]; BOARD_SIZE]; BOARD_SIZE];
        // xorshift64 with a fixed seed — deterministic across runs
        let mut s: u64 = 0x6c62272e07bb0142;
        for row in 0..BOARD_SIZE {
            for col in 0..BOARD_SIZE {
                for color in 0..2 {
                    s ^= s << 13;
                    s ^= s >> 7;
                    s ^= s << 17;
                    t[row][col][color] = s;
                }
            }
        }
        t
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::{Board, Piece, Position};

    // ── table uniqueness ───────────────────────────────────────────────────────

    #[test]
    fn all_table_entries_are_nonzero() {
        let t = table();
        for row in 0..BOARD_SIZE {
            for col in 0..BOARD_SIZE {
                for color in 0..2 {
                    assert_ne!(t[row][col][color], 0, "entry [{row}][{col}][{color}] must be nonzero");
                }
            }
        }
    }

    #[test]
    fn all_table_entries_are_unique() {
        let t = table();
        let mut seen = std::collections::HashSet::new();
        for row in 0..BOARD_SIZE {
            for col in 0..BOARD_SIZE {
                for color in 0..2 {
                    let v = t[row][col][color];
                    assert!(seen.insert(v), "duplicate Zobrist value {v} at [{row}][{col}][{color}]");
                }
            }
        }
    }

    // ── hash_move / unhash_move are inverse (via Board::place + undo) ─────────

    #[test]
    fn place_then_undo_restores_hash_to_zero() {
        let mut board = Board::new();
        let initial_hash = board.hash;
        board.place(Position::new(5, 5), Piece::Black).unwrap();
        board.undo(Position::new(5, 5));
        assert_eq!(board.hash, initial_hash, "undo must restore the original hash");
    }

    #[test]
    fn two_different_positions_produce_different_hashes() {
        let mut board_a = Board::new();
        board_a.place(Position::new(0, 0), Piece::Black).unwrap();

        let mut board_b = Board::new();
        board_b.place(Position::new(0, 1), Piece::Black).unwrap();

        assert_ne!(board_a.hash, board_b.hash, "different positions must produce different hashes");
    }

    #[test]
    fn two_different_colors_at_same_position_produce_different_hashes() {
        let mut board_black = Board::new();
        board_black.place(Position::new(3, 3), Piece::Black).unwrap();

        let mut board_white = Board::new();
        board_white.place(Position::new(3, 3), Piece::White).unwrap();

        assert_ne!(board_black.hash, board_white.hash, "different colors at same position must hash differently");
    }

    #[test]
    fn hash_is_order_independent_for_same_set_of_moves() {
        // XOR is commutative, so placing stones in different orders yields the same hash.
        let mut board_ab = Board::new();
        board_ab.place(Position::new(1, 2), Piece::Black).unwrap();
        board_ab.place(Position::new(3, 4), Piece::White).unwrap();

        let mut board_ba = Board::new();
        board_ba.place(Position::new(3, 4), Piece::White).unwrap();
        board_ba.place(Position::new(1, 2), Piece::Black).unwrap();

        assert_eq!(board_ab.hash, board_ba.hash, "hash must be order-independent");
    }

    #[test]
    fn table_is_deterministic_across_calls() {
        let t1 = table();
        let t2 = table();
        // Same pointer — OnceLock guarantees single initialisation.
        assert!(std::ptr::eq(t1, t2), "table() must return the same static reference");
    }
}
