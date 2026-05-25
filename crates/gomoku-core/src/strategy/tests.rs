use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

use crate::board::{Board, Piece, Position};
use crate::game::GameSnapshot;

use super::{SearchStrategy, StrategyRouter, TacticalStrategy, VcfStrategy, VctStrategy};

fn snap(board: Board, color: Piece) -> GameSnapshot {
    GameSnapshot::new(board, color, color)
}

fn deadline_ms(ms: u64) -> Instant {
    Instant::now() + Duration::from_millis(ms)
}

// ── TacticalStrategy ─────────────────────────────────────────────────────────

#[test]
fn tactical_plays_winning_move() {
    let mut board = Board::new();
    for c in 0..4 {
        board.place(Position::new(5, c), Piece::Black).unwrap();
    }
    let (pos, _, _) = TacticalStrategy::analyze(&snap(board, Piece::Black)).unwrap();
    assert_eq!(pos, Position::new(5, 4));
}

#[test]
fn tactical_blocks_opponent_win() {
    let mut board = Board::new();
    for c in 0..4 {
        board.place(Position::new(5, c), Piece::White).unwrap();
    }
    let (pos, _, _) = TacticalStrategy::analyze(&snap(board, Piece::Black)).unwrap();
    assert_eq!(pos, Position::new(5, 4));
}

#[test]
fn tactical_blocks_vertical_win() {
    let mut board = Board::new();
    for r in 0..4 {
        board.place(Position::new(r, 5), Piece::White).unwrap();
    }
    let (pos, _, _) = TacticalStrategy::analyze(&snap(board, Piece::Black)).unwrap();
    assert_eq!(pos, Position::new(4, 5));
}

#[test]
fn tactical_blocks_diagonal_win() {
    let mut board = Board::new();
    for i in 0..4 {
        board.place(Position::new(3 + i, 3 + i), Piece::White).unwrap();
    }
    let (pos, _, _) = TacticalStrategy::analyze(&snap(board, Piece::Black)).unwrap();
    assert!(
        pos == Position::new(7, 7) || pos == Position::new(2, 2),
        "expected blocking move at (7,7) or (2,2), got {pos:?}"
    );
}

// ── StrategyRouter ────────────────────────────────────────────────────────────

#[test]
fn router_prefers_tactical_when_threats_exist() {
    let mut board = Board::new();
    for c in 0..4 {
        board.place(Position::new(5, c), Piece::Black).unwrap();
    }
    let plan = StrategyRouter::default().choose(&snap(board, Piece::Black)).unwrap();
    assert_eq!(plan.position, Position::new(5, 4));
    assert_eq!(plan.strategy.label(), "tactical");
}

#[test]
fn router_finds_blocking_move_via_tactical() {
    let mut board = Board::new();
    for c in 0..4 {
        board.place(Position::new(5, c), Piece::White).unwrap();
    }
    let plan = StrategyRouter::default().choose(&snap(board, Piece::Black)).unwrap();
    assert_eq!(plan.position, Position::new(5, 4));
}

// ── VcfStrategy ───────────────────────────────────────────────────────────────

#[test]
fn vcf_finds_immediate_five() {
    // Four in a row — VCF should find the fifth stone.
    let mut board = Board::new();
    for c in 1..=4 {
        board.place(Position::new(5, c), Piece::Black).unwrap();
    }
    let mv = VcfStrategy::find_win(&mut board, Piece::Black, 40);
    assert!(
        mv == Some(Position::new(5, 0)) || mv == Some(Position::new(5, 5)),
        "expected winning move at one end, got {mv:?}"
    );
}

#[test]
fn vcf_detects_open_four_double_threat() {
    // Black (5,1)-(5,3). Playing (5,4) creates open-four: threats at (5,0) and (5,5).
    let mut board = Board::new();
    for c in 1..=3 {
        board.place(Position::new(5, c), Piece::Black).unwrap();
    }
    let mv = VcfStrategy::find_win(&mut board, Piece::Black, 40);
    assert!(mv.is_some(), "VCF must find forced win via open-four double threat");
}

#[test]
fn vcf_two_step_forced_win() {
    // Step 1: (5,3) → row four, forced block.  Step 2: (11,0) → open-four (7,0)+(12,0).
    let mut board = Board::new();
    for c in 0..3 {
        board.place(Position::new(5, c), Piece::Black).unwrap();
    }
    for r in 8..=10 {
        board.place(Position::new(r, 0), Piece::Black).unwrap();
    }
    let mv = VcfStrategy::find_win(&mut board, Piece::Black, 40);
    assert!(mv.is_some(), "VCF must find a two-step forced win");
}

#[test]
fn vcf_returns_none_for_no_forced_win() {
    // Only two stones in a row — no forced win exists.
    let mut board = Board::new();
    board.place(Position::new(9, 9), Piece::Black).unwrap();
    board.place(Position::new(9, 10), Piece::Black).unwrap();
    let mv = VcfStrategy::find_win(&mut board, Piece::Black, 40);
    assert!(mv.is_none(), "two stones should not constitute a forced VCF win");
}

#[test]
fn vcf_choose_blocks_opponent_forced_win() {
    // White has a four — Black's VcfStrategy::choose must return the blocking move.
    let mut board = Board::new();
    for c in 0..4 {
        board.place(Position::new(5, c), Piece::White).unwrap();
    }
    let (mv, score, reason) = VcfStrategy::choose(&snap(board, Piece::Black)).unwrap();
    assert_eq!(mv, Position::new(5, 4), "must block White's winning threat");
    assert!(score >= 800_000, "blocking score should be high: {score}");
    assert!(reason.contains("block"), "reason should mention block: {reason}");
}

#[test]
fn vcf_choose_prefers_own_win_over_block() {
    // Both sides have a four — Black wins, doesn't just block.
    let mut board = Board::new();
    for c in 0..4 {
        board.place(Position::new(5, c), Piece::Black).unwrap();
    }
    for c in 0..4 {
        board.place(Position::new(6, c), Piece::White).unwrap();
    }
    let (_, score, reason) = VcfStrategy::choose(&snap(board, Piece::Black)).unwrap();
    assert!(score >= 900_000, "own win should score ≥ 900 000, got {score}");
    assert!(reason.contains("win"), "reason should mention win: {reason}");
}

// ── VctStrategy ───────────────────────────────────────────────────────────────

#[test]
fn vct_detects_double_four_fork() {
    // Playing (5,5) creates horizontal four (5,5)-(5,8) AND vertical four (5,5)-(8,5).
    // Both have two open ends → four winning threats total → unblockable.
    let mut board = Board::new();
    for c in 6..=8 {
        board.place(Position::new(5, c), Piece::Black).unwrap();
    }
    for r in 6..=8 {
        board.place(Position::new(r, 5), Piece::Black).unwrap();
    }
    let mv = VctStrategy::find_win(&mut board, Piece::Black, 30);
    assert_eq!(mv, Some(Position::new(5, 5)), "VCT must find double-four fork at (5,5)");
}

#[test]
fn vct_finds_vcf_win_via_fast_path() {
    // VCT subsumes VCF: a straightforward four-in-a-row should still be found.
    let mut board = Board::new();
    for c in 1..=4 {
        board.place(Position::new(5, c), Piece::Black).unwrap();
    }
    let mv = VctStrategy::find_win(&mut board, Piece::Black, 30);
    assert!(mv.is_some(), "VCT fast-path must find the VCF win");
}

// ── Gap-pattern scoring (regression) ─────────────────────────────────────────

#[test]
fn gap_broken_four_scores_as_must_block() {
    // X X X . P  — filling P completes a virtual four through the gap.
    let mut board = Board::new();
    for c in 0..3 {
        board.place(Position::new(5, c), Piece::White).unwrap();
    }
    let score = board.score_move(Position::new(5, 4), Piece::White);
    assert!(
        score >= 25_000,
        "X X X . P must score ≥ 25 000 (gap broken-four), got {score}"
    );
}

#[test]
fn gap_broken_four_symmetric() {
    // P . X X X — same pattern mirrored; filling P should also score ≥ 25 000.
    let mut board = Board::new();
    for c in 2..=4 {
        board.place(Position::new(5, c), Piece::White).unwrap();
    }
    let score = board.score_move(Position::new(5, 0), Piece::White);
    assert!(
        score >= 25_000,
        "P . X X X must score ≥ 25 000 (gap broken-four), got {score}"
    );
}

#[test]
fn gap_broken_four_double_side() {
    // X X . P . X X — potential on both sides through separate gaps.
    let mut board = Board::new();
    for c in [0usize, 1, 5, 6] {
        board.place(Position::new(5, c), Piece::White).unwrap();
    }
    let score = board.score_move(Position::new(5, 3), Piece::White);
    assert!(
        score >= 25_000,
        "X X . P . X X must score ≥ 25 000, got {score}"
    );
}

#[test]
fn gap_two_stones_no_inflation() {
    // X X . P — only two stones, not a broken four; should score below must-block threshold.
    let mut board = Board::new();
    for c in 0..2 {
        board.place(Position::new(5, c), Piece::White).unwrap();
    }
    let score = board.score_move(Position::new(5, 3), Piece::White);
    assert!(
        score < 25_000,
        "X X . P must NOT reach must-block threshold, got {score}"
    );
}

// ── Open-three missed block regression ───────────────────────────────────────

/// Reproduces the game where White (AdaptiveV2) failed to block Black's open
/// three at turn 8 (moves logged from a real game).
///
/// Initial board: 0:Black:8:8;1:White:9:9;2:Black:8:9;3:White:9:8;
/// Then moves 1–7 before White's failing move 8.
///
/// Run with: cargo test -p gomoku-core open_three_missed -- --nocapture
#[test]
fn open_three_missed_block_diagnosis() {
    use crate::board::Move;

    let mut board = Board::new();

    // Initial board stones
    for (r, c, color) in [
        (8, 8, Piece::Black),
        (9, 9, Piece::White),
        (8, 9, Piece::Black),
        (9, 8, Piece::White),
    ] {
        board.apply_move(Move { row: r, column: c, color }).unwrap();
    }

    // Moves 1–7 from the game log
    for (r, c, color) in [
        (9, 10, Piece::Black),  // 1
        (8, 10, Piece::White),  // 2
        (10,  8, Piece::Black), // 3
        (7, 11, Piece::White),  // 4
        (9,  7, Piece::Black),  // 5
        (10, 11, Piece::White), // 6
        (8,  6, Piece::Black),  // 7  ← Black's diagonal now 3-in-a-row
    ] {
        board.apply_move(Move { row: r, column: c, color }).unwrap();
    }

    // It's White's turn (move 8)
    let snapshot = GameSnapshot::new(board.clone(), Piece::White, Piece::White);
    let candidates = board.candidate_positions();

    // ── Key threat squares ───────────────────────────────────────────────────
    let threat_lo = Position::new(7, 5);   // extend Black's diagonal downward
    let threat_hi = Position::new(11, 9);  // extend Black's diagonal upward
    let white_four = Position::new(6, 12); // White's (7,11)-(8,10)-(9,9) diagonal ext.

    let score_threat_lo_black = board.score_move(threat_lo, Piece::Black);
    let score_threat_hi_black = board.score_move(threat_hi, Piece::Black);
    let score_white_four       = board.score_move(white_four, Piece::White);

    println!("\n=== Board state (move 8 — White to play) ===");
    println!("Black stones (diagonal threat): (8,6) (9,7) (10,8)");
    println!("White stones: (9,9) (9,8) (8,10) (7,11) (10,11)");

    println!("\n── Threat squares ──");
    println!("  score_move((7, 5), Black)  = {score_threat_lo_black}  (want ≥ 100 000 → open four)");
    println!("  score_move((11,9), Black)  = {score_threat_hi_black}  (want ≥ 100 000 → open four)");
    println!("  score_move((6,12), White)  = {score_white_four}       (≥ 25 000 flips guard)");

    // ── AdaptiveV2 guard condition ───────────────────────────────────────────
    let best_self = candidates.iter()
        .map(|&p| board.score_move(p, Piece::White))
        .max()
        .unwrap_or(0);
    let open_four_threats: Vec<Position> = candidates.iter().copied()
        .filter(|&p| board.score_move(p, Piece::Black) >= 100_000)
        .collect();

    println!("\n── AdaptiveV2 `block open three` guard ──");
    println!("  best_self (White) = {best_self}");
    println!("  guard passes (best_self < 100 000): {}", best_self < 100_000);
    println!("  open-four threat squares: {:?}", open_four_threats);

    // ── Top-10 candidates by White's own score ───────────────────────────────
    let mut ranked: Vec<(Position, i32, i32)> = candidates.iter().copied()
        .map(|p| (p, board.score_move(p, Piece::White), board.score_move(p, Piece::Black)))
        .collect();
    ranked.sort_by_key(|(_, mine, _)| -mine);

    println!("\n── Top-10 candidates for White (sorted by White score) ──");
    println!("  {:>8}  {:>12}  {:>12}", "pos", "white_score", "black_score");
    for (pos, mine, theirs) in ranked.iter().take(10) {
        println!("  ({:2},{:2})  {:>12}  {:>12}", pos.row, pos.column, mine, theirs);
    }

    // ── What TacticalStrategy picks ──────────────────────────────────────────
    let tactical = TacticalStrategy::analyze(&snapshot);
    println!("\n── TacticalStrategy pick ──");
    match tactical {
        Some((pos, score, ref reason)) => println!("  ({},{}) score={} reason={}", pos.row, pos.column, score, reason),
        None => println!("  None"),
    }

    // ── What SearchV2 picks with 5-second budget ─────────────────────────────
    let cancel = AtomicBool::new(false);
    let mut strategy = SearchStrategy::default();
    let result = strategy.search_timed(
        &snapshot,
        Instant::now() + Duration::from_secs(5),
        &cancel,
        |pos, score, reason, depth, _| {
            println!("  [depth {depth}] ({},{}) score={score} reason={reason}", pos.row, pos.column);
        },
    );

    println!("\n── SearchV2 final pick (5 s budget) ──");
    match result {
        Some((pos, score, ref reason)) => println!("  ({},{}) score={} reason={}", pos.row, pos.column, score, reason),
        None => println!("  None"),
    }

    // ── Regression assertions ────────────────────────────────────────────────
    // Scoring: both ends of Black's open three must register as open-four threats.
    assert!(
        score_threat_lo_black >= 100_000,
        "(7,5) must score ≥ 100 000 as Black (open four), got {score_threat_lo_black}"
    );
    assert!(
        score_threat_hi_black >= 100_000,
        "(11,9) must score ≥ 100 000 as Black (open four), got {score_threat_hi_black}"
    );
    // TacticalStrategy (used by router and AdaptiveV2 guard) must block the open four.
    // This was the primary bug: White's closed four at (6,12)=25 063 was above the old
    // guard threshold of 25 000, causing the block to be skipped.
    let tactical_pick = tactical.map(|(p, _, _)| p);
    assert!(
        tactical_pick == Some(threat_lo) || tactical_pick == Some(threat_hi),
        "TacticalStrategy must block (7,5) or (11,9), but picked {:?}",
        tactical_pick
    );
    // AdaptiveV2 guard threshold: best_self (25 063) must be < 100 000 (fixed threshold).
    assert!(
        best_self < 100_000,
        "best_self {best_self} should be < 100 000 so the open-four block fires"
    );
}

// ── Open-four missed (6,6 vs 7,6 / 11,6) regression ─────────────────────────

/// Reproduces move 10 where Black played (6,6) instead of the open-four at
/// (7,6) or (11,6) in column 6.
///
/// Initial board: Black(8,8) White(8,10) Black(8,6)
/// Moves 1-9 before Black's failing move 10.
///
/// Run with: cargo test -p gomoku-core open_four_missed -- --nocapture
#[test]
fn open_four_missed_col6_diagnosis() {
    use crate::board::Move;

    let mut board = Board::new();

    // Initial board stones
    for (r, c, color) in [
        (8, 8, Piece::Black),
        (8, 10, Piece::White),
        (8, 6, Piece::Black),
    ] {
        board.apply_move(Move { row: r, column: c, color }).unwrap();
    }

    // Moves 1-9
    for (r, c, color) in [
        (8, 7, Piece::White),  // 1
        (9, 5, Piece::Black),  // 2
        (7, 7, Piece::White),  // 3
        (9, 7, Piece::Black),  // 4
        (6, 7, Piece::White),  // 5
        (9, 6, Piece::Black),  // 6
        (9, 8, Piece::White),  // 7
        (10, 6, Piece::Black), // 8
        (7, 9, Piece::White),  // 9
    ] {
        board.apply_move(Move { row: r, column: c, color }).unwrap();
    }

    // It's Black's turn (move 10)
    let snapshot = GameSnapshot::new(board.clone(), Piece::Black, Piece::Black);
    let candidates = board.candidate_positions();

    println!("\n=== Board state (move 10 — Black to play) ===");
    println!("Black stones: (8,8) (8,6) (9,5) (9,7) (9,6) (10,6)");
    println!("White stones: (8,10) (8,7) (7,7) (6,7) (9,8) (7,9)");
    println!("move_count = {}", board.move_count());

    // Key positions
    let p_76  = Position::new(7, 6);
    let p_116 = Position::new(11, 6);
    let p_66  = Position::new(6, 6);   // what the bot actually played

    println!("\n── Key position scores (Black) ──");
    for pos in [p_76, p_116, p_66] {
        let mine   = board.score_move(pos, Piece::Black);
        let theirs = board.score_move(pos, Piece::White);
        let ps     = super::position_score(&snapshot, pos);
        let in_cand = candidates.contains(&pos);
        println!("  ({:2},{:2})  black={:8}  white={:8}  position_score={:8}  candidate={}", pos.row, pos.column, mine, theirs, ps, in_cand);
    }

    // Router insight
    let insight = board.inspect(Piece::Black);
    println!("\n── Board insights (Black) ──");
    println!("  candidate_count       = {}", insight.candidate_count);
    println!("  self_winning_moves    = {}", insight.self_winning_moves);
    println!("  opponent_winning_moves= {}", insight.opponent_winning_moves);
    println!("  best_self_score       = {}", insight.best_self_score);
    println!("  best_opponent_score   = {}", insight.best_opponent_score);

    // Top candidates by position_score (how search_v2 sorts root)
    let mut ranked: Vec<(Position, i32, i32, i32)> = candidates.iter().copied()
        .map(|p| (p, board.score_move(p, Piece::Black), board.score_move(p, Piece::White), super::position_score(&snapshot, p)))
        .collect();
    ranked.sort_by_key(|(_, _, _, ps)| -*ps);

    println!("\n── Top-15 candidates (sorted by position_score) ──");
    println!("  {:>8}  {:>10}  {:>10}  {:>14}", "pos", "black", "white", "position_score");
    for (pos, mine, theirs, ps) in ranked.iter().take(15) {
        println!("  ({:2},{:2})  {:>10}  {:>10}  {:>14}", pos.row, pos.column, mine, theirs, ps);
    }

    // TacticalStrategy
    let tactical = TacticalStrategy::analyze(&snapshot);
    println!("\n── TacticalStrategy pick ──");
    match tactical {
        Some((pos, score, ref reason)) => println!("  ({},{}) score={} reason={}", pos.row, pos.column, score, reason),
        None => println!("  None"),
    }

    // StrategyRouter
    let router_plan = StrategyRouter::default().choose(&snapshot);
    println!("\n── StrategyRouter pick ──");
    match router_plan {
        Some(ref plan) => println!("  ({},{}) strategy={} score={} reason={}", plan.position.row, plan.position.column, plan.strategy.label(), plan.score, plan.reason),
        None => println!("  None"),
    }

    // VCF candidate order (row-major, unfiltered)
    println!("\n── VCF candidate order (row-major, score ≥ 25 000) ──");
    let vcf_candidates: Vec<(Position, i32)> = candidates.iter().copied()
        .filter(|&p| board.score_move(p, Piece::Black) >= 25_000)
        .map(|p| (p, board.score_move(p, Piece::Black)))
        .collect();
    for (p, s) in &vcf_candidates {
        println!("  ({:2},{:2})  black={}", p.row, p.column, s);
    }

    // What winning_moves returns after placing each key candidate
    println!("\n── winning_moves(Black) after placing each key candidate ──");
    for pos in [p_76, p_116, p_66] {
        let mut b = board.clone();
        b.place(pos, Piece::Black).unwrap();
        let wins = b.winning_moves(Piece::Black);
        println!("  after ({},{})  winning_moves = {:?}", pos.row, pos.column, wins);
    }

    // VCF result for individual starting moves
    println!("\n── VCF find_win for key starting moves ──");
    for pos in [p_66, p_76, p_116] {
        let mut b = board.clone();
        b.place(pos, Piece::Black).unwrap();
        let wins_after = b.winning_moves(Piece::Black);
        // Only recurse if this move creates a threat
        let vcf_from_here = if wins_after.is_empty() {
            "no threat created".to_string()
        } else {
            format!("threats={:?}", wins_after)
        };
        println!("  ({:2},{:2}) → {}", pos.row, pos.column, vcf_from_here);
    }

    // Full VCF call on original board
    println!("\n── VcfStrategy::find_win (full, no deadline) ──");
    {
        let mut b = board.clone();
        let vcf_mv = VcfStrategy::find_win(&mut b, Piece::Black, 40);
        println!("  result = {:?}", vcf_mv);
    }

    // SearchV2 with progress
    println!("\n── SearchV2 IDDFS (2 s budget) ──");
    let cancel = AtomicBool::new(false);
    let mut strategy = SearchStrategy::default();
    let result = strategy.search_timed(
        &snapshot,
        Instant::now() + Duration::from_secs(2),
        &cancel,
        |pos, score, reason, depth, _| {
            println!("  [depth {depth:2}] ({},{}) score={score} reason={reason}", pos.row, pos.column);
        },
    );
    println!("\n── SearchV2 final pick ──");
    match result {
        Some((pos, score, ref reason)) => println!("  ({},{}) score={} reason={}", pos.row, pos.column, score, reason),
        None => println!("  None"),
    }
}

// ── Move 5 White: missed blocking Black's open-three at (9,6) ────────────────

/// Move 5 (White to play). White played (6,7) — building a closed three in
/// column 7 — while Black's (9,5)–(9,7) gap already sets up an open three at
/// (9,6). The test diagnoses why the block was skipped.
///
/// Run with: cargo test -p gomoku-core move5_white_missed_block -- --nocapture
#[test]
fn move5_white_missed_block_diagnosis() {
    use crate::board::Move;

    let mut board = Board::new();

    // Initial board
    for (r, c, color) in [
        (8, 8, Piece::Black),
        (8, 10, Piece::White),
        (8, 6, Piece::Black),
    ] {
        board.apply_move(Move { row: r, column: c, color }).unwrap();
    }

    // Moves 1-4
    for (r, c, color) in [
        (8, 7, Piece::White), // 1
        (9, 5, Piece::Black), // 2
        (7, 7, Piece::White), // 3
        (9, 7, Piece::Black), // 4
    ] {
        board.apply_move(Move { row: r, column: c, color }).unwrap();
    }

    // Move 5: White to play
    let snapshot = GameSnapshot::new(board.clone(), Piece::White, Piece::White);
    let candidates = board.candidate_positions();

    println!("\n=== Move 5 — White to play ===");
    println!("White stones: (8,10) (8,7) (7,7)");
    println!("Black stones: (8,8) (8,6) (9,5) (9,7)");
    println!("move_count = {}", board.move_count());

    // Key positions
    let p_actual = Position::new(6, 7);  // what White actually played
    let p_96     = Position::new(9, 6);  // gap in Black's (9,5)–(9,7)
    let p_106    = Position::new(10, 6); // also mentioned by user
    let p_79     = Position::new(7, 9);  // also mentioned by user

    println!("\n── Key position scores ──");
    println!("  {:>8}  {:>10}  {:>10}  {:>14}  candidate", "pos", "white", "black", "position_score");
    for pos in [p_actual, p_96, p_106, p_79] {
        let mine   = board.score_move(pos, Piece::White);
        let theirs = board.score_move(pos, Piece::Black);
        let ps     = super::position_score(&snapshot, pos);
        println!("  ({:2},{:2})  {:>10}  {:>10}  {:>14}  {}",
            pos.row, pos.column, mine, theirs, ps, candidates.contains(&pos));
    }

    // What score Black gets at (9,6) and what it creates
    {
        let mut b = board.clone();
        b.place(Position::new(9, 6), Piece::Black).unwrap();
        let wins = b.winning_moves(Piece::Black);
        println!("\n── After Black plays (9,6) — Black's winning moves: {:?}", wins);
        println!("   score_move((9,6), Black) = {}", board.score_move(Position::new(9, 6), Piece::Black));
        println!("   (open-three threshold = 8 000, open-four threshold = 100 000)");
    }

    // Board insights
    let insight = board.inspect(Piece::White);
    println!("\n── Board insights (White) ──");
    println!("  candidate_count        = {}", insight.candidate_count);
    println!("  self_winning_moves     = {}", insight.self_winning_moves);
    println!("  opponent_winning_moves = {}", insight.opponent_winning_moves);
    println!("  best_self_score        = {}", insight.best_self_score);
    println!("  best_opponent_score    = {}", insight.best_opponent_score);

    // Top candidates by position_score
    let mut ranked: Vec<(Position, i32, i32, i32)> = candidates.iter().copied()
        .map(|p| (p, board.score_move(p, Piece::White), board.score_move(p, Piece::Black), super::position_score(&snapshot, p)))
        .collect();
    ranked.sort_by_key(|(_, _, _, ps)| -*ps);
    println!("\n── Top-15 candidates (sorted by position_score) ──");
    println!("  {:>8}  {:>10}  {:>10}  {:>14}", "pos", "white", "black", "position_score");
    for (pos, w, b_score, ps) in ranked.iter().take(15) {
        let marker = if *pos == p_actual { " ← played" } else if *pos == p_96 { " ← block?" } else { "" };
        println!("  ({:2},{:2})  {:>10}  {:>10}  {:>14}{}", pos.row, pos.column, w, b_score, ps, marker);
    }

    // Tactical
    let tactical = TacticalStrategy::analyze(&snapshot);
    println!("\n── TacticalStrategy pick ──");
    match tactical {
        Some((pos, score, ref reason)) => println!("  ({},{}) score={} reason={}", pos.row, pos.column, score, reason),
        None => println!("  None"),
    }

    // Router
    let router_plan = StrategyRouter::default().choose(&snapshot);
    println!("\n── StrategyRouter pick ──");
    match router_plan {
        Some(ref plan) => println!("  ({},{}) strategy={} score={} reason={}", plan.position.row, plan.position.column, plan.strategy.label(), plan.score, plan.reason),
        None => println!("  None"),
    }

    // TacticalStrategy scoring formula for each key move
    println!("\n── TacticalStrategy formula: mine*2 - theirs ──");
    for pos in [p_actual, p_96, p_106, p_79] {
        let mine   = board.score_move(pos, Piece::White);
        let theirs = board.score_move(pos, Piece::Black);
        let tactical_score = mine * 2 - theirs;
        let ps = super::position_score(&snapshot, pos);
        println!("  ({:2},{:2})  mine={:6}  theirs={:6}  tactical={:8}  position_score={:8}",
            pos.row, pos.column, mine, theirs, tactical_score, ps);
    }

    // evaluate_for after each key White move (what the depth-0 leaf sees)
    println!("\n── evaluate_for(Black) - evaluate_for(White) at depth-0 leaf after White's move ──");
    println!("  (this is what depth=1 uses to rank root candidates)");
    for pos in [p_actual, p_96] {
        let mut b = board.clone();
        b.place(pos, Piece::White).unwrap();
        let eval_black = b.evaluate_for(Piece::Black);
        let eval_white = b.evaluate_for(Piece::White);
        // pvs_v2 at depth=0 with color=Black returns eval_black - eval_white
        // then negated at root => White sees -(eval_black - eval_white)
        let leaf_from_whites_perspective = eval_white - eval_black;
        println!("  after White ({},{}): eval_black={} eval_white={} → White's leaf score ≈ {}",
            pos.row, pos.column, eval_black, eval_white, leaf_from_whites_perspective);
        // Show best White candidate after this move (explains eval_white inflation)
        let mut top_white: Vec<(Position, i32)> = b.candidate_positions().iter()
            .map(|&p| (p, b.score_move(p, Piece::White)))
            .collect();
        top_white.sort_by_key(|(_, s)| -*s);
        println!("    top-3 White candidates: {:?}", &top_white.iter().take(3).collect::<Vec<_>>());
    }

    // SearchV2 with very short budget (depth 1 only)
    println!("\n── SearchV2 with 50 ms budget (depth 1 only) ──");
    {
        let cancel = AtomicBool::new(false);
        let mut strat = SearchStrategy::default();
        let r = strat.search_timed(&snapshot, Instant::now() + Duration::from_millis(50), &cancel,
            |pos, score, reason, depth, _| {
                println!("  [depth {:2}] ({},{}) score={} reason={}", depth, pos.row, pos.column, score, reason);
            });
        println!("  result: {:?}", r.map(|(p, s, r)| (p.row, p.column, s, r)));
    }

    // SearchV2 (2 s)
    println!("\n── SearchV2 IDDFS (2 s) ──");
    let cancel = AtomicBool::new(false);
    let mut strategy = SearchStrategy::default();
    let result = strategy.search_timed(
        &snapshot,
        Instant::now() + Duration::from_secs(2),
        &cancel,
        |pos, score, reason, depth, _| {
            println!("  [depth {:2}] ({},{}) score={} reason={}", depth, pos.row, pos.column, score, reason);
        },
    );
    println!("\n── SearchV2 final pick ──");
    match result {
        Some((pos, score, ref reason)) => println!("  ({},{}) score={} reason={}", pos.row, pos.column, score, reason),
        None => println!("  None"),
    }
}

// ── PatternStrategy ───────────────────────────────────────────────────────────

#[test]
fn pattern_returns_center_on_empty_board() {
    use crate::board::CENTER;
    use super::PatternStrategy;
    let board = crate::board::Board::new();
    let snapshot = snap(board, Piece::Black);
    let (pos, _, _) = PatternStrategy::choose(&snapshot).unwrap();
    // On an empty board the only candidate is the center.
    assert_eq!(pos, crate::board::Position::new(CENTER, CENTER));
}

#[test]
fn pattern_does_not_return_occupied_cell() {
    use super::PatternStrategy;
    let mut board = crate::board::Board::new();
    board.place(crate::board::Position::new(9, 9), Piece::Black).unwrap();
    let snapshot = snap(board.clone(), Piece::White);
    let (pos, _, _) = PatternStrategy::choose(&snapshot).unwrap();
    assert!(board.is_empty_at(pos), "pattern must not return an occupied cell");
}

#[test]
fn pattern_prefers_extension_of_existing_stones() {
    use super::PatternStrategy;
    // Place two Black stones in a row; the best extension should score higher than
    // an isolated cell far away.
    let mut board = crate::board::Board::new();
    board.place(crate::board::Position::new(9, 8), Piece::Black).unwrap();
    board.place(crate::board::Position::new(9, 9), Piece::Black).unwrap();
    let snapshot = snap(board, Piece::Black);
    let (_, score, _) = PatternStrategy::choose(&snapshot).unwrap();
    // Score for extending a 2-in-a-row should be at least 250 (open-two minimum).
    assert!(score >= 250, "pattern score for extending two in a row should be >= 250, got {score}");
}

// ── TacticalStrategy (additional) ────────────────────────────────────────────

#[test]
fn tactical_returns_none_on_empty_board() {
    // Empty board has no candidates near any stone; TacticalStrategy may return
    // a positional move but cannot find a win/block.  The important invariant is
    // it does not panic.
    let board = crate::board::Board::new();
    // An empty board only has center as candidate; no threat exists.
    let snapshot = snap(board, Piece::Black);
    // We only verify it doesn't panic; it may return Some for positional play.
    let _ = TacticalStrategy::analyze(&snapshot);
}

#[test]
fn tactical_plays_own_five_in_a_row_over_block() {
    // Both players have a four; Black should win rather than block.
    let mut board = crate::board::Board::new();
    for c in 0..4 {
        board.place(crate::board::Position::new(5, c), Piece::Black).unwrap();
    }
    for c in 0..4 {
        board.place(crate::board::Position::new(6, c), Piece::White).unwrap();
    }
    let (pos, _, reason) = TacticalStrategy::analyze(&snap(board, Piece::Black)).unwrap();
    // Black's winning move is (5,4).
    assert_eq!(pos, crate::board::Position::new(5, 4));
    assert!(reason.contains("winning"), "reason should mention winning, got: {reason}");
}

#[test]
fn tactical_blocks_opponent_open_four_when_no_own_open_four() {
    // White has an open four; Black has no open four → must block.
    let mut board = crate::board::Board::new();
    for c in 2..6 {
        board.place(crate::board::Position::new(5, c), Piece::White).unwrap();
    }
    let (_, _, reason) = TacticalStrategy::analyze(&snap(board, Piece::Black)).unwrap();
    assert!(
        reason.contains("block") || reason.contains("open four"),
        "reason should indicate blocking, got: {reason}"
    );
}

// ── VcfStrategy (additional) ──────────────────────────────────────────────────

#[test]
fn vcf_does_not_panic_at_max_depth() {
    // Exercises the depth-limit guard in vcf_recurse.
    let mut board = crate::board::Board::new();
    board.place(crate::board::Position::new(9, 9), Piece::Black).unwrap();
    // max_depth = 0 must return None without panicking.
    let mv = VcfStrategy::find_win(&mut board, Piece::Black, 0);
    assert!(mv.is_none());
}

#[test]
fn vcf_find_win_returns_none_for_isolated_stone() {
    let mut board = crate::board::Board::new();
    board.place(crate::board::Position::new(9, 9), Piece::Black).unwrap();
    assert!(VcfStrategy::find_win(&mut board, Piece::Black, 40).is_none());
}

// ── VctStrategy (additional) ──────────────────────────────────────────────────

#[test]
fn vct_returns_none_for_quiet_position() {
    let mut board = crate::board::Board::new();
    board.place(crate::board::Position::new(9, 9), Piece::Black).unwrap();
    board.place(crate::board::Position::new(9, 10), Piece::Black).unwrap();
    let mv = VctStrategy::find_win(&mut board, Piece::Black, 30);
    assert!(mv.is_none(), "two stones should not be a VCT win");
}

#[test]
fn vct_subsumes_vcf_for_four_in_a_row() {
    // VCT fast-path: four-in-a-row is a VCF win, VCT must find it too.
    let mut board = crate::board::Board::new();
    for c in 1..=4 {
        board.place(crate::board::Position::new(7, c), Piece::Black).unwrap();
    }
    let mv = VctStrategy::find_win(&mut board, Piece::Black, 30);
    assert!(mv.is_some(), "VCT must find an immediate VCF win (four-in-a-row)");
}

#[test]
fn vct_find_win_handles_zero_max_depth() {
    // With max_depth=0, VCT may still find a VCF win (fast path is unconstrained),
    // but must not panic.
    let mut board = crate::board::Board::new();
    board.place(crate::board::Position::new(9, 9), Piece::Black).unwrap();
    let _ = VctStrategy::find_win(&mut board, Piece::Black, 0);
}

// ── StrategyRouter (additional) ───────────────────────────────────────────────

#[test]
fn router_uses_pattern_in_opening_phase() {
    // Fewer than 4 moves → router should choose pattern strategy.
    let mut board = crate::board::Board::new();
    board.place(crate::board::Position::new(9, 9), Piece::Black).unwrap();
    board.place(crate::board::Position::new(9, 10), Piece::White).unwrap();
    // 2 moves < 4 → opening phase.
    let plan = StrategyRouter::default().choose(&snap(board, Piece::Black)).unwrap();
    assert_eq!(plan.strategy.label(), "pattern", "router must pick pattern in opening (< 4 moves)");
}

#[test]
fn router_blocks_opponent_win_via_tactical() {
    let mut board = crate::board::Board::new();
    for c in 0..4 {
        board.place(crate::board::Position::new(5, c), Piece::White).unwrap();
    }
    let plan = StrategyRouter::default().choose(&snap(board, Piece::Black)).unwrap();
    assert_eq!(plan.position, crate::board::Position::new(5, 4));
}

#[test]
fn router_uses_search_on_narrow_board() {
    // Force a narrow board: place enough stones that candidate_count <= 8 but no threat.
    // A cross-pattern with stones only adjacent leaves very few candidates.
    let mut board = crate::board::Board::new();
    // Place a dense cluster so candidates are few and no immediate threat.
    // Use a 3x3 block in the corner offset — 9 placed stones with tight neighbors.
    let positions = [
        (1,1),(1,3),(1,5),
        (3,1),(3,3),(3,5),
        (5,1),(5,3),(5,5),
    ];
    for (i, &(r, c)) in positions.iter().enumerate() {
        let piece = if i % 2 == 0 { Piece::Black } else { Piece::White };
        board.place(crate::board::Position::new(r, c), piece).unwrap();
    }
    let insight = board.inspect(Piece::Black);
    // Only run the router assertion when candidate_count is actually <= 8.
    if insight.candidate_count <= 8 && insight.self_winning_moves == 0 && insight.opponent_winning_moves == 0
        && insight.best_self_score < 50_000 && insight.best_opponent_score < 50_000 {
        let plan = StrategyRouter::default().choose(&snap(board, Piece::Black));
        // In the narrow-board branch the router calls search::choose.
        assert!(plan.is_some(), "router must return a move even on a narrow board");
    }
    // If the candidate count condition wasn't met, the test is a no-op (not a failure).
}

// ── SearchStrategy (additional) ───────────────────────────────────────────────

#[test]
fn search_returns_move_on_near_empty_board() {
    let mut board = crate::board::Board::new();
    board.place(crate::board::Position::new(9, 9), Piece::Black).unwrap();
    let cancel = std::sync::atomic::AtomicBool::new(false);
    let mut strategy = SearchStrategy::default();
    let result = strategy.search_timed(&snap(board, Piece::White), deadline_ms(500), &cancel, |_, _, _, _, _| {});
    assert!(result.is_some(), "search must return a move on a near-empty board");
}

#[test]
fn search_respects_deadline_and_returns_before_overrun() {
    let mut board = crate::board::Board::new();
    // Mid-game position with several stones.
    for c in 3..7 {
        board.place(crate::board::Position::new(9, c), Piece::Black).unwrap();
        board.place(crate::board::Position::new(10, c), Piece::White).unwrap();
    }
    let cancel = std::sync::atomic::AtomicBool::new(false);
    let mut strategy = SearchStrategy::default();
    let start = std::time::Instant::now();
    let _result = strategy.search_timed(&snap(board, Piece::Black), deadline_ms(200), &cancel, |_, _, _, _, _| {});
    // Allow 500 ms grace period beyond the 200 ms deadline.
    assert!(
        start.elapsed() < std::time::Duration::from_millis(700),
        "search must not massively overrun deadline, took {:?}",
        start.elapsed()
    );
}

#[test]
fn search_on_depth_callback_called_at_least_once_for_solved_position() {
    // Four-in-a-row is immediately solved by VCF pre-check; callback should
    // NOT be called (the VCF path bypasses IDDFS), but search must return.
    let mut board = crate::board::Board::new();
    for c in 0..4 {
        board.place(crate::board::Position::new(5, c), Piece::Black).unwrap();
    }
    let cancel = std::sync::atomic::AtomicBool::new(false);
    let mut strategy = SearchStrategy::default();
    let result = strategy.search_timed(
        &snap(board, Piece::Black),
        deadline_ms(500),
        &cancel,
        |_, _, _, _, _| {},
    );
    assert!(result.is_some(), "search must return a move for a solved position");
    let (pos, _, _) = result.unwrap();
    assert_eq!(pos, crate::board::Position::new(5, 4));
}

// ── SearchStrategy ──────────────────────────────────────────────────────────

#[test]
fn search_vcf_precheck_finds_win() {
    // VCF pre-check fires before IDDFS begins; result should arrive well within budget.
    let mut board = Board::new();
    for c in 0..4 {
        board.place(Position::new(5, c), Piece::Black).unwrap();
    }
    let cancel = AtomicBool::new(false);
    let start = Instant::now();
    let mut strategy = SearchStrategy::default();
    let (pos, _, _) = strategy
        .search_timed(&snap(board, Piece::Black), deadline_ms(500), &cancel, |_, _, _, _, _| {})
        .unwrap();
    assert_eq!(pos, Position::new(5, 4));
    assert!(
        start.elapsed() < Duration::from_millis(300),
        "VCF pre-check should return fast, took {:?}",
        start.elapsed()
    );
}

#[test]
fn search_blocks_opponent_win() {
    let mut board = Board::new();
    for c in 0..4 {
        board.place(Position::new(5, c), Piece::White).unwrap();
    }
    let cancel = AtomicBool::new(false);
    let mut strategy = SearchStrategy::default();
    let (pos, _, _) = strategy
        .search_timed(&snap(board, Piece::Black), deadline_ms(500), &cancel, |_, _, _, _, _| {})
        .unwrap();
    assert_eq!(pos, Position::new(5, 4));
}

#[test]
fn search_persistent_tt_reuses_cache() {
    // Calling search_timed twice on the same strategy should reuse TT entries
    // and return a consistent result, not panic or produce garbage.
    let mut board = Board::new();
    for c in 3..7 {
        board.place(Position::new(9, c), Piece::Black).unwrap();
    }
    for c in 3..7 {
        board.place(Position::new(10, c), Piece::White).unwrap();
    }
    let cancel = AtomicBool::new(false);
    let mut strategy = SearchStrategy::default();
    let snap = snap(board, Piece::Black);

    let first = strategy.search_timed(&snap, deadline_ms(200), &cancel, |_, _, _, _, _| {});
    let second = strategy.search_timed(&snap, deadline_ms(200), &cancel, |_, _, _, _, _| {});

    assert!(first.is_some() && second.is_some(), "both calls must return a move");
    assert_eq!(
        first.unwrap().0,
        second.unwrap().0,
        "same position should yield same best move across calls"
    );
}

// ── Search timing ─────────────────────────────────────────────────────────────

/// For a mid-game position with no forced win, the search must use most of the
/// time budget (depth > 1) and return before the deadline + a small grace period.
#[test]
fn search_uses_most_of_time_budget() {
    // Move-5 board state — no immediate VCF win for either side.
    use crate::board::Move;
    let mut board = Board::new();
    for (r, c, color) in [
        (8, 8, Piece::Black), (8, 10, Piece::White), (8, 6, Piece::Black),
        (8, 7, Piece::White), (9, 5, Piece::Black), (7, 7, Piece::White), (9, 7, Piece::Black),
    ] {
        board.apply_move(Move { row: r, column: c, color }).unwrap();
    }

    let budget_ms = 500u64;
    let start = Instant::now();
    let cancel = AtomicBool::new(false);
    let mut strategy = SearchStrategy::default();
    let mut max_depth_reached = 0usize;
    let result = strategy.search_timed(
        &snap(board, Piece::White),
        Instant::now() + Duration::from_millis(budget_ms),
        &cancel,
        |_, _, _, depth, _| { max_depth_reached = max_depth_reached.max(depth); },
    );
    let elapsed = start.elapsed().as_millis() as u64;

    assert!(result.is_some(), "must return a move");
    assert!(
        max_depth_reached >= 2,
        "must reach at least depth 2, only reached depth {max_depth_reached} in {elapsed}ms"
    );
    assert!(
        elapsed >= budget_ms * 4 / 5,
        "must use at least 80% of the {budget_ms}ms budget (used {elapsed}ms, depth {max_depth_reached})"
    );
    assert!(
        elapsed <= budget_ms + 200,
        "must not overrun deadline by more than 200ms (budget {budget_ms}ms, elapsed {elapsed}ms)"
    );
}

/// The VCF pre-check must be bounded by the main deadline so short budgets are
/// not blown on VCF alone.  With a 60ms budget on a complex mid-game position,
/// the search must return in under 60ms + a small grace period.
#[test]
fn search_vcf_precheck_respects_main_deadline() {
    use crate::board::Move;
    let mut board = Board::new();
    for (r, c, color) in [
        (8, 8, Piece::Black), (8, 10, Piece::White), (8, 6, Piece::Black),
        (8, 7, Piece::White), (9, 5, Piece::Black), (7, 7, Piece::White), (9, 7, Piece::Black),
    ] {
        board.apply_move(Move { row: r, column: c, color }).unwrap();
    }

    let budget_ms = 60u64;
    let start = Instant::now();
    let cancel = AtomicBool::new(false);
    let mut strategy = SearchStrategy::default();
    let result = strategy.search_timed(
        &snap(board, Piece::White),
        Instant::now() + Duration::from_millis(budget_ms),
        &cancel,
        |_, _, _, _, _| {},
    );
    let elapsed = start.elapsed().as_millis() as u64;

    assert!(result.is_some(), "must return a move even with tight budget");
    // Without the fix, VCF pre-check alone could consume 200ms, blowing the 60ms budget.
    assert!(
        elapsed <= budget_ms + 100,
        "VCF pre-check must not overrun main deadline (budget {budget_ms}ms, elapsed {elapsed}ms)"
    );
}

// ── Move 5 White: must block at least one of Black's open-three threats ───────

/// At move 5, Black has multiple open-three threats (score ≥ SCORE_OPEN_THREE)
/// on row 9, the anti-diagonal, and the main diagonal.  White played (6,7) —
/// a half-open three in column 7 — which defends none of them.
///
/// This test:
///   1. Enumerates ALL Black positions scoring ≥ SCORE_OPEN_THREE.
///   2. Prints diagnostic info (scores, strategy picks, position_score ranking).
///   3. Asserts that SearchStrategy's final pick is one of those threat squares.
///
/// The assertion is expected to FAIL with the current bot, exposing the root
/// cause (why (6,7) outranks the defensive squares at the depth used).
///
/// Run with: cargo test -p gomoku-core move5_white_defends -- --nocapture
#[test]
fn move5_white_defends_black_open_three_threats() {
    use crate::board::{Move, SCORE_OPEN_THREE};

    let mut board = Board::new();

    for (r, c, color) in [
        (8, 8, Piece::Black),
        (8, 10, Piece::White),
        (8, 6, Piece::Black),
    ] {
        board.apply_move(Move { row: r, column: c, color }).unwrap();
    }

    for (r, c, color) in [
        (8, 7, Piece::White), // move 1
        (9, 5, Piece::Black), // move 2
        (7, 7, Piece::White), // move 3
        (9, 7, Piece::Black), // move 4
    ] {
        board.apply_move(Move { row: r, column: c, color }).unwrap();
    }

    let snapshot = GameSnapshot::new(board.clone(), Piece::White, Piece::White);
    let candidates = board.candidate_positions();

    // ── 1. All Black open-three (or better) forming positions ────────────────
    let mut black_threats: Vec<(Position, i32)> = candidates.iter().copied()
        .map(|p| (p, board.score_move(p, Piece::Black)))
        .filter(|(_, s)| *s >= SCORE_OPEN_THREE)
        .collect();
    black_threats.sort_by_key(|(_, s)| -*s);
    let threat_positions: Vec<Position> = black_threats.iter().map(|(p, _)| *p).collect();

    println!("\n=== Move 5 — White to play ===");
    println!("Black: (8,8) (8,6) (9,5) (9,7)   White: (8,10) (8,7) (7,7)");

    println!("\n── Black open-three threats (score ≥ {SCORE_OPEN_THREE}) ──");
    println!("  {:>7}  {:>12}  {:>12}", "pos", "black_score", "white_score");
    for (pos, bs) in &black_threats {
        let ws = board.score_move(*pos, Piece::White);
        println!("  ({:2},{:2})  {:>12}  {:>12}", pos.row, pos.column, bs, ws);
    }
    println!("  {} threat squares total", threat_positions.len());

    // ── 2. position_score ranking — shows whether threats outrank (6,7) ──────
    let mut ranked: Vec<(Position, i32, i32, i32)> = candidates.iter().copied()
        .map(|p| (p,
            board.score_move(p, Piece::White),
            board.score_move(p, Piece::Black),
            super::position_score(&snapshot, p)))
        .collect();
    ranked.sort_by_key(|(_, _, _, ps)| -*ps);

    println!("\n── Top-15 candidates by position_score ──");
    println!("  {:>7}  {:>10}  {:>10}  {:>14}", "pos", "white", "black", "pos_score");
    for (pos, w, b_s, ps) in ranked.iter().take(15) {
        let tag = if threat_positions.contains(pos) { " ← Black threat" }
                  else if *pos == Position::new(6, 7) { " ← bot played (6,7)" }
                  else { "" };
        println!("  ({:2},{:2})  {:>10}  {:>10}  {:>14}{}", pos.row, pos.column, w, b_s, ps, tag);
    }

    // ── 3. Strategy picks ────────────────────────────────────────────────────
    let tactical = TacticalStrategy::analyze(&snapshot);
    println!("\n── TacticalStrategy ──");
    match tactical {
        Some((pos, score, ref reason)) =>
            println!("  ({},{}) score={} reason={} — defensive={}", pos.row, pos.column, score, reason, threat_positions.contains(&pos)),
        None => println!("  None"),
    }

    let router = StrategyRouter::default().choose(&snapshot);
    println!("\n── StrategyRouter ──");
    match router {
        Some(ref plan) =>
            println!("  ({},{}) strategy={} score={} — defensive={}", plan.position.row, plan.position.column, plan.strategy.label(), plan.score, threat_positions.contains(&plan.position)),
        None => println!("  None"),
    }

    println!("\n── SearchStrategy (500 ms, depth-by-depth) ──");
    let cancel = AtomicBool::new(false);
    let mut strategy = SearchStrategy::default();
    let result = strategy.search_timed(
        &snapshot,
        Instant::now() + Duration::from_millis(500),
        &cancel,
        |pos, score, reason, depth, _| {
            let tag = if threat_positions.contains(&pos) { " ← defensive" } else { "" };
            println!("  [depth {:2}] ({},{}) score={:8} reason={}{}", depth, pos.row, pos.column, score, reason, tag);
        },
    );

    let white_pick = result.map(|(p, _, _)| p).expect("SearchStrategy must return a move");
    println!("\n── Final pick: ({},{}) — defensive={} ──",
        white_pick.row, white_pick.column, threat_positions.contains(&white_pick));

    // ── 4. Assertion ─────────────────────────────────────────────────────────
    assert!(
        !threat_positions.is_empty(),
        "there must be at least one Black open-three threat at move 5"
    );
    assert!(
        threat_positions.contains(&white_pick),
        "White must block one of Black's {} open-three threats at move 5\n\
         White played: ({},{})\n\
         Threat squares: {:?}",
        threat_positions.len(),
        white_pick.row, white_pick.column,
        threat_positions,
    );
}
