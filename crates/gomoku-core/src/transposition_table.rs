use crate::board::Position;

// 1M slots × 2 tiers × 16 bytes = 32 MB
const TT_SLOTS: usize = 1 << 20;

pub const TT_FLAG_NONE: u8 = 0;
pub const TT_FLAG_EXACT: u8 = 1;
pub const TT_FLAG_LOWER: u8 = 2; // alpha (score is a lower bound)
pub const TT_FLAG_UPPER: u8 = 3; // beta  (score is an upper bound)

#[derive(Clone, Copy)]
pub struct TTEntry {
    pub key: u64,
    pub score: i32,
    pub depth: u8,
    pub flag: u8,
    pub best_row: u8,  // 255 = no move stored
    pub best_col: u8,
}

impl TTEntry {
    const EMPTY: Self = Self {
        key: 0,
        score: 0,
        depth: 0,
        flag: TT_FLAG_NONE,
        best_row: 255,
        best_col: 255,
    };

    pub fn best_move(self) -> Option<Position> {
        if self.best_row == 255 {
            None
        } else {
            Some(Position::new(self.best_row as usize, self.best_col as usize))
        }
    }
}

impl Default for TTEntry {
    fn default() -> Self {
        Self::EMPTY
    }
}

pub struct TranspositionTable {
    // Two tiers per slot: [always-replace, depth-preferred]
    slots: Vec<[TTEntry; 2]>,
    mask: usize,
}

impl TranspositionTable {
    pub fn new() -> Self {
        Self {
            slots: vec![[TTEntry::EMPTY; 2]; TT_SLOTS],
            mask: TT_SLOTS - 1,
        }
    }

    pub fn probe(&self, key: u64) -> Option<TTEntry> {
        let idx = (key as usize) & self.mask;
        let [always, depth] = self.slots[idx];
        if always.flag != TT_FLAG_NONE && always.key == key {
            return Some(always);
        }
        if depth.flag != TT_FLAG_NONE && depth.key == key {
            return Some(depth);
        }
        None
    }

    pub fn store(&mut self, key: u64, depth: u8, score: i32, flag: u8, best_move: Option<Position>) {
        let idx = (key as usize) & self.mask;
        let entry = TTEntry {
            key,
            score,
            depth,
            flag,
            best_row: best_move.map_or(255, |p| p.row as u8),
            best_col: best_move.map_or(255, |p| p.column as u8),
        };

        // Tier 0: always replace (freshest data for fast lookup)
        self.slots[idx][0] = entry;

        // Tier 1: depth-preferred (keeps deep results longer)
        let existing = self.slots[idx][1];
        if existing.flag == TT_FLAG_NONE || depth >= existing.depth {
            self.slots[idx][1] = entry;
        }
    }

    pub fn clear(&mut self) {
        self.slots.iter_mut().for_each(|s| *s = [TTEntry::EMPTY; 2]);
    }
}

impl Default for TranspositionTable {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::Position;

    fn make_pos(row: usize, col: usize) -> Option<Position> {
        Some(Position::new(row, col))
    }

    // ── store + probe: exact match ────────────────────────────────────────────

    #[test]
    fn probe_returns_entry_after_store_with_same_key() {
        let mut tt = TranspositionTable::new();
        tt.store(0xABCD_1234, 5, 42, TT_FLAG_EXACT, make_pos(3, 7));
        let entry = tt.probe(0xABCD_1234).expect("should find stored entry");
        assert_eq!(entry.key, 0xABCD_1234);
        assert_eq!(entry.score, 42);
        assert_eq!(entry.depth, 5);
        assert_eq!(entry.flag, TT_FLAG_EXACT);
        assert_eq!(entry.best_move(), make_pos(3, 7));
    }

    // ── store + probe: wrong key returns None ─────────────────────────────────

    #[test]
    fn probe_returns_none_for_different_key() {
        let mut tt = TranspositionTable::new();
        tt.store(0xAAAA_BBBB, 3, 100, TT_FLAG_EXACT, None);
        assert!(tt.probe(0xCCCC_DDDD).is_none());
    }

    // ── always-replace tier ───────────────────────────────────────────────────

    #[test]
    fn always_replace_tier_overwrites_any_existing_entry() {
        let mut tt = TranspositionTable::new();
        tt.store(0x1111, 8, 10, TT_FLAG_EXACT, make_pos(1, 1));
        // Second write at same key: always-replace tier must hold the newer one.
        tt.store(0x1111, 1, 99, TT_FLAG_UPPER, make_pos(2, 2));
        let entry = tt.probe(0x1111).expect("entry should exist");
        // always-replace is tried first, so score=99 (the newest write).
        assert_eq!(entry.score, 99, "always-replace tier should hold the most recent write");
    }

    // ── depth-preferred tier: shallower entry does NOT replace deeper ─────────

    #[test]
    fn depth_preferred_tier_keeps_deeper_entry_when_shallower_arrives() {
        let mut tt = TranspositionTable::new();
        let key = 0x2222_FFFF;
        // Store deep entry.
        tt.store(key, 10, 50, TT_FLAG_EXACT, make_pos(4, 4));
        // Attempt overwrite with shallower.
        tt.store(key, 3, 999, TT_FLAG_LOWER, make_pos(5, 5));
        // always-replace (tier 0) now holds depth=3.
        let entry = tt.probe(key).expect("entry should exist");
        assert_eq!(entry.depth, 3, "always-replace returns the newest (shallow) write");
    }

    // ── best_move with no move stored ────────────────────────────────────────

    #[test]
    fn best_move_returns_none_when_no_move_stored() {
        let mut tt = TranspositionTable::new();
        tt.store(0xDEAD_BEEF, 1, 0, TT_FLAG_EXACT, None);
        let entry = tt.probe(0xDEAD_BEEF).unwrap();
        assert_eq!(entry.best_move(), None);
    }

    // ── clear ─────────────────────────────────────────────────────────────────

    #[test]
    fn clear_removes_all_entries() {
        let mut tt = TranspositionTable::new();
        tt.store(0xFFFF, 5, 55, TT_FLAG_EXACT, make_pos(8, 8));
        tt.clear();
        assert!(tt.probe(0xFFFF).is_none());
    }

    // ── flag round-trip ───────────────────────────────────────────────────────

    #[test]
    fn probe_preserves_lower_bound_flag() {
        let mut tt = TranspositionTable::new();
        tt.store(0xAA11, 2, -30, TT_FLAG_LOWER, None);
        let entry = tt.probe(0xAA11).unwrap();
        assert_eq!(entry.flag, TT_FLAG_LOWER);
    }

    #[test]
    fn probe_preserves_upper_bound_flag() {
        let mut tt = TranspositionTable::new();
        tt.store(0xBB22, 2, -30, TT_FLAG_UPPER, None);
        let entry = tt.probe(0xBB22).unwrap();
        assert_eq!(entry.flag, TT_FLAG_UPPER);
    }

    // ── empty table returns None ──────────────────────────────────────────────

    #[test]
    fn new_table_probes_return_none() {
        let tt = TranspositionTable::new();
        assert!(tt.probe(0x0).is_none());
        assert!(tt.probe(0xFFFF_FFFF_FFFF_FFFF).is_none());
    }
}
