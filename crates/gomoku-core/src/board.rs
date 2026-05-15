use serde::{Deserialize, Serialize};

pub const BOARD_SIZE: usize = 18;
pub const CENTER: usize = BOARD_SIZE / 2;
const MAX_STEPS_FROM_STONE: usize = 2;

pub const SCORE_FIVE:       i32 = 1_000_000;
pub const SCORE_OPEN_FOUR:  i32 = 100_000;
pub const SCORE_FOUR:       i32 = 25_000;
pub const SCORE_OPEN_THREE: i32 = 8_000;

#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum Piece {
    Black,
    White,
}

impl Piece {
    pub fn opponent(self) -> Self {
        match self {
            Piece::Black => Piece::White,
            Piece::White => Piece::Black,
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Position {
    pub row: usize,
    pub column: usize,
}

impl Position {
    pub const fn new(row: usize, column: usize) -> Self {
        Self { row, column }
    }
}

impl From<Move> for Position {
    fn from(value: Move) -> Self {
        Self { row: value.row, column: value.column }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Move {
    pub row: usize,
    pub column: usize,
    pub color: Piece,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BoardError {
    OutOfBounds(Position),
    Occupied(Position),
}

impl std::fmt::Display for BoardError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BoardError::OutOfBounds(position) => {
                write!(f, "position out of bounds: ({}, {})", position.row, position.column)
            }
            BoardError::Occupied(position) => {
                write!(f, "position already occupied: ({}, {})", position.row, position.column)
            }
        }
    }
}

impl std::error::Error for BoardError {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Board {
    cells: [[Option<Piece>; BOARD_SIZE]; BOARD_SIZE],
    move_count: usize,
    pub hash: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BoardInsights {
    pub candidate_count: usize,
    pub self_winning_moves: usize,
    pub opponent_winning_moves: usize,
    pub best_self_score: i32,
    pub best_opponent_score: i32,
}

impl Board {
    pub fn new() -> Self {
        Self {
            cells: [[None; BOARD_SIZE]; BOARD_SIZE],
            move_count: 0,
            hash: 0,
        }
    }

    pub fn move_count(&self) -> usize {
        self.move_count
    }

    pub fn is_empty(&self) -> bool {
        self.move_count == 0
    }

    pub fn in_bounds(position: Position) -> bool {
        position.row < BOARD_SIZE && position.column < BOARD_SIZE
    }

    pub fn get(&self, position: Position) -> Option<Piece> {
        if Self::in_bounds(position) {
            self.cells[position.row][position.column]
        } else {
            None
        }
    }

    pub fn is_empty_at(&self, position: Position) -> bool {
        self.get(position).is_none()
    }

    pub fn place(&mut self, position: Position, piece: Piece) -> Result<(), BoardError> {
        if !Self::in_bounds(position) {
            return Err(BoardError::OutOfBounds(position));
        }

        if self.cells[position.row][position.column].is_some() {
            return Err(BoardError::Occupied(position));
        }

        self.cells[position.row][position.column] = Some(piece);
        self.hash ^= crate::zobrist::table()[position.row][position.column][piece_index(piece)];
        self.move_count += 1;
        Ok(())
    }

    pub fn undo(&mut self, position: Position) {
        if let Some(piece) = self.cells[position.row][position.column].take() {
            self.hash ^= crate::zobrist::table()[position.row][position.column][piece_index(piece)];
            self.move_count -= 1;
        }
    }

    pub fn apply_move(&mut self, mv: Move) -> Result<(), BoardError> {
        self.place(Position::new(mv.row, mv.column), mv.color)
    }

    pub fn iter_positions(&self) -> impl Iterator<Item = Position> + '_ {
        (0..BOARD_SIZE).flat_map(|row| (0..BOARD_SIZE).map(move |column| Position::new(row, column)))
    }

    pub fn candidate_positions(&self) -> Vec<Position> {
        if self.is_empty() {
            return vec![Position::new(CENTER, CENTER)];
        }

        let mut seen = [false; BOARD_SIZE * BOARD_SIZE];
        let mut candidates = Vec::new();

        for row in 0..BOARD_SIZE {
            for column in 0..BOARD_SIZE {
                if self.cells[row][column].is_none() {
                    continue;
                }

                let row_start = row.saturating_sub(MAX_STEPS_FROM_STONE);
                let row_end = usize::min(BOARD_SIZE - 1, row + MAX_STEPS_FROM_STONE);
                let column_start = column.saturating_sub(MAX_STEPS_FROM_STONE);
                let column_end = usize::min(BOARD_SIZE - 1, column + MAX_STEPS_FROM_STONE);

                for candidate_row in row_start..=row_end {
                    for candidate_column in column_start..=column_end {
                        let position = Position::new(candidate_row, candidate_column);
                        if !self.is_empty_at(position) {
                            continue;
                        }

                        let index = candidate_row * BOARD_SIZE + candidate_column;
                        if seen[index] {
                            continue;
                        }

                        seen[index] = true;
                        candidates.push(position);
                    }
                }
            }
        }

        candidates
    }

    pub fn winning_moves(&self, color: Piece) -> Vec<Position> {
        self.candidate_positions()
            .into_iter()
            .filter(|position| self.would_win(*position, color))
            .collect()
    }

    pub fn has_winning_move(&self, color: Piece) -> bool {
        self.candidate_positions()
            .into_iter()
            .any(|position| self.would_win(position, color))
    }

    pub fn first_winning_move(&self, color: Piece) -> Option<Position> {
        self.candidate_positions()
            .into_iter()
            .find(|position| self.would_win(*position, color))
    }

    /// Returns the number of winning moves up to `cap`, stopping early once `cap` is reached.
    pub fn count_winning_moves_up_to(&self, color: Piece, cap: usize) -> usize {
        let mut count = 0;
        for position in self.candidate_positions() {
            if self.would_win(position, color) {
                count += 1;
                if count >= cap {
                    break;
                }
            }
        }
        count
    }

    pub fn would_win(&self, position: Position, color: Piece) -> bool {
        if !Self::in_bounds(position) || !self.is_empty_at(position) {
            return false;
        }

        self.score_patterns(position, color)
            .into_iter()
            .any(|pattern| pattern.five_in_a_row)
    }

    pub fn score_move(&self, position: Position, color: Piece) -> i32 {
        if !Self::in_bounds(position) || !self.is_empty_at(position) {
            return i32::MIN / 4;
        }

        let mut score = self.center_bonus(position);
        for pattern in self.score_patterns(position, color) {
            score += pattern.score;
        }
        score
    }

    pub fn evaluate_for(&self, color: Piece) -> i32 {
        let candidates = self.candidate_positions();
        self.evaluate_for_candidates(color, &candidates)
    }

    pub fn evaluate_for_candidates(&self, color: Piece, candidates: &[Position]) -> i32 {
        let mut top = [i32::MIN / 2; 3];
        for &pos in candidates {
            let s = self.score_move(pos, color);
            if s > top[2] {
                top[2] = s;
                if top[2] > top[1] {
                    top.swap(1, 2);
                    if top[1] > top[0] {
                        top.swap(0, 1);
                    }
                }
            }
        }
        top.iter().filter(|&&s| s > i32::MIN / 2).sum()
    }

    pub fn inspect(&self, color: Piece) -> BoardInsights {
        let candidates = self.candidate_positions();
        let self_winning_moves = candidates.iter().filter(|position| self.would_win(**position, color)).count();
        let opponent_winning_moves = candidates
            .iter()
            .filter(|position| self.would_win(**position, color.opponent()))
            .count();
        let best_self_score = candidates
            .iter()
            .map(|position| self.score_move(*position, color))
            .max()
            .unwrap_or(i32::MIN / 4);
        let best_opponent_score = candidates
            .iter()
            .map(|position| self.score_move(*position, color.opponent()))
            .max()
            .unwrap_or(i32::MIN / 4);

        BoardInsights {
            candidate_count: candidates.len(),
            self_winning_moves,
            opponent_winning_moves,
            best_self_score,
            best_opponent_score,
        }
    }

    fn center_bonus(&self, position: Position) -> i32 {
        let row_distance = position.row.abs_diff(CENTER) as i32;
        let column_distance = position.column.abs_diff(CENTER) as i32;
        60 - (row_distance + column_distance) * 2
    }

    fn score_patterns(&self, position: Position, color: Piece) -> Vec<PatternScore> {
        const DIRECTIONS: &[(isize, isize)] = &[(1, 0), (0, 1), (1, 1), (1, -1)];

        DIRECTIONS
            .iter()
            .map(|(row_delta, column_delta)| self.score_direction(position, color, *row_delta, *column_delta))
            .collect()
    }

    fn score_direction(&self, position: Position, color: Piece, row_delta: isize, column_delta: isize) -> PatternScore {
        // Positive direction: count contiguous friendly stones.
        let mut contiguous_positive = 0usize;
        let mut row = position.row as isize + row_delta;
        let mut column = position.column as isize + column_delta;
        while self.is_cell_color(row, column, color) {
            contiguous_positive += 1;
            row += row_delta;
            column += column_delta;
        }
        // (row, column) is now at the first non-same cell — the open end or a blocker.
        let open_positive = self.is_empty_cell(row, column);
        // Look through a single empty gap for skip-connected stones.
        // After filling that gap: total_in_line = direct_total + 1 (gap) + skip.
        // A gap-win threat exists when direct_total + skip >= 4 (filling gap reaches 5).
        let skip_positive = if open_positive {
            let mut sr = row + row_delta;
            let mut sc = column + column_delta;
            let mut n = 0usize;
            while self.is_cell_color(sr, sc, color) {
                n += 1;
                sr += row_delta;
                sc += column_delta;
            }
            n
        } else {
            0
        };

        // Negative direction.
        let mut contiguous_negative = 0usize;
        let mut row = position.row as isize - row_delta;
        let mut column = position.column as isize - column_delta;
        while self.is_cell_color(row, column, color) {
            contiguous_negative += 1;
            row -= row_delta;
            column -= column_delta;
        }
        let open_negative = self.is_empty_cell(row, column);
        let skip_negative = if open_negative {
            let mut sr = row - row_delta;
            let mut sc = column - column_delta;
            let mut n = 0usize;
            while self.is_cell_color(sr, sc, color) {
                n += 1;
                sr -= row_delta;
                sc -= column_delta;
            }
            n
        } else {
            0
        };

        let total = 1 + contiguous_positive + contiguous_negative;
        let open_ends = u8::from(open_positive) + u8::from(open_negative);

        // potential_X = direct_total + skip_X: if >= 4, filling that gap creates five-in-a-row.
        let potential_pos = total + skip_positive;
        let potential_neg = total + skip_negative;
        // "Broken four" — opponent must fill the gap or we win next move.
        // Also covers the double-gap case (X X . P . X X): each gap alone yields potential=3,
        // but combined (skip_pos + skip_neg + total >= 5) filling P connects both sides to ≥5.
        let double_gap = skip_positive > 0 && skip_negative > 0
            && total + skip_positive + skip_negative >= 5;
        let gap_win_threat = (skip_positive > 0 && potential_pos >= 4)
            || (skip_negative > 0 && potential_neg >= 4)
            || double_gap;

        // Direct five or more: immediate win.
        if total >= 5 {
            return PatternScore::new(SCORE_FIVE, true);
        }
        // Direct open four: both ends free, unblockable.
        if total == 4 && open_ends == 2 {
            return PatternScore::new(SCORE_OPEN_FOUR, false);
        }
        // Direct closed four OR broken four (gap fill = win): one forced response.
        if total == 4 || gap_win_threat {
            return PatternScore::new(SCORE_FOUR, false);
        }
        // Broken three: filling a gap reaches 3 stones with at least one open end.
        let gap_three = (skip_positive > 0 && potential_pos == 3 && open_ends >= 1)
            || (skip_negative > 0 && potential_neg == 3 && open_ends >= 1);

        match (total, open_ends) {
            (3, 2) => PatternScore::new(SCORE_OPEN_THREE, false),
            (3, 1) => PatternScore::new(1_500, false),
            (2, 2) if gap_three => PatternScore::new(1_200, false),
            (2, 1) if gap_three => PatternScore::new(400, false),
            (2, 2) => PatternScore::new(250, false),
            (2, 1) => PatternScore::new(40, false),
            _ => PatternScore::new(total as i32 * 5, false),
        }
    }

    fn is_cell_color(&self, row: isize, column: isize, color: Piece) -> bool {
        if row < 0 || column < 0 {
            return false;
        }

        let row = row as usize;
        let column = column as usize;
        row < BOARD_SIZE && column < BOARD_SIZE && self.cells[row][column] == Some(color)
    }

    fn is_empty_cell(&self, row: isize, column: isize) -> bool {
        if row < 0 || column < 0 {
            return false;
        }

        let row = row as usize;
        let column = column as usize;
        row < BOARD_SIZE && column < BOARD_SIZE && self.cells[row][column].is_none()
    }
}

fn piece_index(piece: Piece) -> usize {
    match piece {
        Piece::Black => 0,
        Piece::White => 1,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PatternScore {
    score: i32,
    five_in_a_row: bool,
}

impl PatternScore {
    const fn new(score: i32, five_in_a_row: bool) -> Self {
        Self {
            score,
            five_in_a_row,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── place / undo ──────────────────────────────────────────────────────────

    #[test]
    fn place_out_of_bounds_returns_error() {
        let mut board = Board::new();
        let result = board.place(Position::new(BOARD_SIZE, 0), Piece::Black);
        assert_eq!(result, Err(BoardError::OutOfBounds(Position::new(BOARD_SIZE, 0))));
    }

    #[test]
    fn place_occupied_cell_returns_error() {
        let mut board = Board::new();
        board.place(Position::new(5, 5), Piece::Black).unwrap();
        let result = board.place(Position::new(5, 5), Piece::White);
        assert_eq!(result, Err(BoardError::Occupied(Position::new(5, 5))));
    }

    #[test]
    fn undo_removes_piece_and_allows_re_placement() {
        let mut board = Board::new();
        board.place(Position::new(3, 3), Piece::Black).unwrap();
        board.undo(Position::new(3, 3));
        assert!(board.is_empty_at(Position::new(3, 3)));
        // Re-placement should succeed.
        board.place(Position::new(3, 3), Piece::White).unwrap();
        assert_eq!(board.get(Position::new(3, 3)), Some(Piece::White));
    }

    #[test]
    fn undo_decrements_move_count() {
        let mut board = Board::new();
        board.place(Position::new(0, 0), Piece::Black).unwrap();
        assert_eq!(board.move_count(), 1);
        board.undo(Position::new(0, 0));
        assert_eq!(board.move_count(), 0);
    }

    #[test]
    fn undo_reverts_zobrist_hash() {
        let mut board = Board::new();
        let hash_before = board.hash;
        board.place(Position::new(5, 5), Piece::Black).unwrap();
        board.undo(Position::new(5, 5));
        assert_eq!(board.hash, hash_before);
    }

    // ── score_move: pattern types ─────────────────────────────────────────────

    #[test]
    fn score_move_open_five_horizontal_returns_one_million() {
        let mut board = Board::new();
        // Place 4 in a row, score the 5th position.
        for col in 0..4 {
            board.place(Position::new(0, col), Piece::Black).unwrap();
        }
        let score = board.score_move(Position::new(0, 4), Piece::Black);
        assert!(score >= 1_000_000, "five-in-a-row should score >= 1 000 000, got {score}");
    }

    #[test]
    fn score_move_open_four_both_ends_free() {
        // _ X X X X _ (columns 1-4, scoring column 5 creates open-four)
        // Actually score the middle-extension: place 3, score the 4th in open space.
        let mut board = Board::new();
        for col in 1..4 {
            board.place(Position::new(2, col), Piece::Black).unwrap();
        }
        // Column 4 extends to 4 stones; column 0 is open → open-four.
        let score = board.score_move(Position::new(2, 4), Piece::Black);
        assert!(score >= 100_000, "open four should score >= 100 000, got {score}");
    }

    #[test]
    fn score_move_closed_four_scores_as_must_block() {
        let mut board = Board::new();
        // Block one end with an opponent stone.
        board.place(Position::new(0, 0), Piece::White).unwrap();
        for col in 1..4 {
            board.place(Position::new(0, col), Piece::Black).unwrap();
        }
        // Scoring col 4: 4 stones total, only right end open → closed four.
        let score = board.score_move(Position::new(0, 4), Piece::Black);
        assert!(score >= 25_000, "closed four should score >= 25 000, got {score}");
        assert!(score < 100_000, "closed four should score < 100 000, got {score}");
    }

    #[test]
    fn score_move_open_three_scores_correctly() {
        let mut board = Board::new();
        for col in 2..4 {
            board.place(Position::new(1, col), Piece::Black).unwrap();
        }
        // 3rd stone at col 4: total=3, both ends open → open three = 8 000.
        let score = board.score_move(Position::new(1, 4), Piece::Black);
        assert!(score >= 8_000, "open three should score >= 8 000, got {score}");
    }

    #[test]
    fn score_move_two_in_a_row_open_both_ends() {
        let mut board = Board::new();
        board.place(Position::new(5, 5), Piece::Black).unwrap();
        // Score adjacent cell — total = 2, open ends = 2.
        let score = board.score_move(Position::new(5, 6), Piece::Black);
        assert!(score >= 250, "open two should score >= 250, got {score}");
    }

    // ── score_move: all 4 directions ──────────────────────────────────────────

    #[test]
    fn score_move_vertical_five_in_a_row() {
        let mut board = Board::new();
        for row in 0..4 {
            board.place(Position::new(row, 5), Piece::Black).unwrap();
        }
        let score = board.score_move(Position::new(4, 5), Piece::Black);
        assert!(score >= 1_000_000, "vertical five should score >= 1 000 000, got {score}");
    }

    #[test]
    fn score_move_diagonal_slash_five_in_a_row() {
        let mut board = Board::new();
        // Diagonal (/): row decreases, column increases.
        for i in 0..4 {
            board.place(Position::new(4 - i, i), Piece::Black).unwrap();
        }
        // (0, 4) completes the diagonal.
        let score = board.score_move(Position::new(0, 4), Piece::Black);
        assert!(score >= 1_000_000, "diagonal / five should score >= 1 000 000, got {score}");
    }

    #[test]
    fn score_move_diagonal_backslash_five_in_a_row() {
        let mut board = Board::new();
        // Diagonal (\): row and column both increase.
        for i in 0..4 {
            board.place(Position::new(i, i), Piece::Black).unwrap();
        }
        let score = board.score_move(Position::new(4, 4), Piece::Black);
        assert!(score >= 1_000_000, "diagonal \\ five should score >= 1 000 000, got {score}");
    }

    // ── candidate_positions ───────────────────────────────────────────────────

    #[test]
    fn empty_board_prefers_center() {
        let board = Board::new();
        assert_eq!(board.candidate_positions(), vec![Position::new(CENTER, CENTER)]);
    }

    #[test]
    fn placed_stone_not_included_in_candidates() {
        let mut board = Board::new();
        board.place(Position::new(9, 9), Piece::Black).unwrap();
        let candidates = board.candidate_positions();
        assert!(!candidates.contains(&Position::new(9, 9)));
    }

    #[test]
    fn candidates_only_within_two_steps_of_stones() {
        let mut board = Board::new();
        board.place(Position::new(9, 9), Piece::Black).unwrap();
        let candidates = board.candidate_positions();
        // (9,12) is 3 columns away → out of range.
        assert!(!candidates.contains(&Position::new(9, 12)));
        // (9,11) is exactly 2 away → in range.
        assert!(candidates.contains(&Position::new(9, 11)));
    }

    #[test]
    fn candidates_include_all_adjacent_cells() {
        let mut board = Board::new();
        board.place(Position::new(9, 9), Piece::Black).unwrap();
        let candidates = board.candidate_positions();
        // All 4 orthogonal neighbors at distance 1.
        assert!(candidates.contains(&Position::new(7, 7)));
        assert!(candidates.contains(&Position::new(11, 11)));
        assert!(candidates.contains(&Position::new(7, 11)));
        assert!(candidates.contains(&Position::new(11, 7)));
    }

    // ── winning_moves ─────────────────────────────────────────────────────────

    #[test]
    fn detects_winning_move() {
        let mut board = Board::new();
        for column in 0..4 {
            board.place(Position::new(0, column), Piece::Black).unwrap();
        }

        assert!(board.would_win(Position::new(0, 4), Piece::Black));
        assert_eq!(board.winning_moves(Piece::Black), vec![Position::new(0, 4)]);
    }

    #[test]
    fn winning_moves_returns_empty_when_no_threat() {
        let mut board = Board::new();
        board.place(Position::new(9, 9), Piece::Black).unwrap();
        assert!(board.winning_moves(Piece::Black).is_empty());
    }

    #[test]
    fn winning_moves_detects_both_open_ends_of_open_four() {
        let mut board = Board::new();
        for col in 2..6 {
            board.place(Position::new(5, col), Piece::Black).unwrap();
        }
        let wins = board.winning_moves(Piece::Black);
        // Both ends (col 1 and col 6) should be winning.
        assert!(wins.contains(&Position::new(5, 1)) || wins.contains(&Position::new(5, 6)));
        assert!(wins.len() >= 2, "open four should have >= 2 winning positions");
    }

    #[test]
    fn winning_moves_does_not_include_occupied_cells() {
        let mut board = Board::new();
        for col in 0..4 {
            board.place(Position::new(0, col), Piece::Black).unwrap();
        }
        // Place opponent stone at the winning position.
        board.place(Position::new(0, 4), Piece::White).unwrap();
        let wins = board.winning_moves(Piece::Black);
        assert!(!wins.contains(&Position::new(0, 4)));
    }

    // ── evaluate_for ──────────────────────────────────────────────────────────

    #[test]
    fn evaluate_for_higher_for_stronger_position() {
        let mut board_weak = Board::new();
        board_weak.place(Position::new(9, 9), Piece::Black).unwrap();

        let mut board_strong = Board::new();
        for col in 0..3 {
            board_strong.place(Position::new(5, col), Piece::Black).unwrap();
        }

        let score_weak = board_weak.evaluate_for(Piece::Black);
        let score_strong = board_strong.evaluate_for(Piece::Black);
        assert!(score_strong > score_weak, "three in a row should score higher than one stone");
    }

    #[test]
    fn evaluate_for_symmetric_on_mirrored_boards() {
        let mut board = Board::new();
        for col in 0..3 {
            board.place(Position::new(5, col), Piece::Black).unwrap();
        }
        // Mirrored: same structure for White.
        let mut board2 = Board::new();
        for col in 0..3 {
            board2.place(Position::new(5, col), Piece::White).unwrap();
        }
        let black_score = board.evaluate_for(Piece::Black);
        let white_score = board2.evaluate_for(Piece::White);
        assert_eq!(black_score, white_score, "mirrored boards should give equal scores");
    }

    // ── inspect ───────────────────────────────────────────────────────────────

    #[test]
    fn inspect_correct_candidate_count_after_placing_stone() {
        let mut board = Board::new();
        board.place(Position::new(9, 9), Piece::Black).unwrap();
        let insights = board.inspect(Piece::Black);
        // Candidates are the empty cells within 2 of (9,9) = 5x5 - 1 = 24 at most.
        assert!(insights.candidate_count > 0);
        assert!(insights.candidate_count <= 24);
    }

    #[test]
    fn inspect_detects_self_winning_moves() {
        let mut board = Board::new();
        for col in 0..4 {
            board.place(Position::new(0, col), Piece::Black).unwrap();
        }
        let insights = board.inspect(Piece::Black);
        assert!(insights.self_winning_moves >= 1);
    }

    #[test]
    fn inspect_detects_opponent_winning_moves() {
        let mut board = Board::new();
        for col in 0..4 {
            board.place(Position::new(0, col), Piece::White).unwrap();
        }
        let insights = board.inspect(Piece::Black);
        assert!(insights.opponent_winning_moves >= 1);
        assert_eq!(insights.self_winning_moves, 0);
    }

    #[test]
    fn inspect_returns_zero_winning_moves_on_quiet_board() {
        let mut board = Board::new();
        board.place(Position::new(9, 9), Piece::Black).unwrap();
        let insights = board.inspect(Piece::Black);
        assert_eq!(insights.self_winning_moves, 0);
        assert_eq!(insights.opponent_winning_moves, 0);
    }

    // ── candidate_generation ──────────────────────────────────────────────────

    #[test]
    fn candidate_generation_stays_local() {
        let mut board = Board::new();
        board.place(Position::new(9, 9), Piece::Black).unwrap();
        let candidates = board.candidate_positions();

        assert!(candidates.contains(&Position::new(7, 7)));
        assert!(candidates.contains(&Position::new(11, 11)));
        assert!(!candidates.contains(&Position::new(0, 0)));
    }

    #[test]
    fn scoring_rewards_center_on_empty_board() {
        let board = Board::new();
        assert!(board.score_move(Position::new(CENTER, CENTER), Piece::Black) > board.score_move(Position::new(0, 0), Piece::Black));
    }

    // Gap-pattern tests: scoring must see friendly stones across a single empty cell.

    #[test]
    fn broken_four_scores_as_must_block() {
        // Board row: _ X X _ P _ _ ... (P completes X X _ _ _ side, skip sees X X on the other)
        // Concrete: place Black at (0,1),(0,2) and (0,4),(0,5); score (0,3) = the gap between them.
        // After scoring (0,3): contiguous_neg=0, contiguous_pos=0, but skip sees 2 in each direction.
        // potential = 1+0+2 = 3 from positive side; from negative side = 3 — not quite 4.
        // Better: _ X X X _ P — skip_neg=3, potential_neg = 2+3 = 5 >= 4 → must-block.
        let mut board = Board::new();
        // Place three Black stones at columns 0,1,2 and score column 4 (gap at col 3).
        for col in 0..3 {
            board.place(Position::new(0, col), Piece::Black).unwrap();
        }
        let score = board.score_move(Position::new(0, 4), Piece::Black);
        assert!(score >= 25_000, "broken four (X X X _ P) should score >= 25 000, got {score}");
    }

    #[test]
    fn broken_four_with_single_skip_on_other_side() {
        // X X X _ P _ X — skip_neg=3 → potential_neg=4 ≥ 4, must-block regardless of positive side.
        let mut board = Board::new();
        for col in [0usize, 1, 2, 6] {
            board.place(Position::new(0, col), Piece::Black).unwrap();
        }
        let score = board.score_move(Position::new(0, 4), Piece::Black);
        assert!(score >= 25_000, "broken four (X X X _ P _ X) should score >= 25 000, got {score}");
    }

    #[test]
    fn plain_two_without_gap_not_inflated() {
        // Two adjacent stones with no gap connection should NOT reach must-block range.
        let mut board = Board::new();
        board.place(Position::new(5, 5), Piece::Black).unwrap();
        board.place(Position::new(5, 6), Piece::Black).unwrap();
        // Score a cell adjacent on the open side — no skip stones beyond the far open end.
        let score = board.score_move(Position::new(5, 4), Piece::Black);
        assert!(score < 25_000, "plain open two should score < 25 000, got {score}");
    }

    // ── score_move on occupied / out-of-bounds ────────────────────────────────

    #[test]
    fn score_move_occupied_cell_returns_min() {
        let mut board = Board::new();
        board.place(Position::new(5, 5), Piece::Black).unwrap();
        let score = board.score_move(Position::new(5, 5), Piece::Black);
        assert!(score < 0, "occupied cell should return a very negative score, got {score}");
    }

    #[test]
    fn score_move_out_of_bounds_returns_min() {
        let board = Board::new();
        let score = board.score_move(Position::new(BOARD_SIZE, 0), Piece::Black);
        assert!(score < 0, "out-of-bounds should return a very negative score, got {score}");
    }
}
