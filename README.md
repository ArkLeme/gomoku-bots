# Rust Gomoku Bot

Rust implementation of a Puissance 5 / Gomoku WebSocket bot for the [Codebusters Connect-5](https://api-connect5.dev.codebusters.cloud) platform.

## Requirements

- **Rust 1.75+** — install via [rustup.rs](https://rustup.rs)
- Network access to the game API (or use `--local-server` for offline play)

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

Verify:

```bash
rustc --version   # rustc 1.75.0 or later
cargo --version
```

## Quick start

```bash
cargo test                        # run all tests
cargo run -p gomoku-cli -- --demo # offline engine smoke test
```

## Run against the server

Guest mode (no account needed):

```bash
cargo run -p gomoku-cli -- \
  --mode guest \
  --bot-name rust-gomoku-bot \
  --room-name rust-room \
  --create-room
```

Registered mode:

```bash
cargo run -p gomoku-cli -- \
  --mode registered \
  --username <username> \
  --password <password> \
  --room-name rust-room \
  --create-room
```

Key flags:

| Flag | Default | Description |
|------|---------|-------------|
| `--strategy <name>` | `adaptive` | Strategy to use (see below) |
| `--move-time-seconds <s>` | `5` | Time budget per move |
| `--base-url <url>` | game server | API base URL |
| `--create-room` | off | Host the room before joining |
| `--initial-board-moves-history <h>` | — | Pre-load a board position |
| `--demo` | off | Local smoke test, no network |
| `--debug-websocket` | off | Log raw SignalR frames |

## Browser UI mode

Serves a web interface at `http://localhost:8080` to configure and watch two bots play:

```bash
cargo run -p gomoku-cli -- --ui [--ui-port 8080]
```

## Local server mode

Start a self-contained SignalR-compatible game server for offline bot-vs-bot testing:

```bash
# Terminal 1 — start the hub
cargo run -p gomoku-cli -- --local-server --local-server-port 8081

# Terminal 2 — bot 1 (room owner)
cargo run -p gomoku-cli -- \
  --mode guest --bot-name bot-1 --room-name test-room --create-room \
  --base-url http://localhost:8081

# Terminal 3 — bot 2
cargo run -p gomoku-cli -- \
  --mode guest --bot-name bot-2 --room-name test-room \
  --base-url http://localhost:8081
```

## Strategies

| `--strategy` | Description |
|---|---|
| `adaptive` *(default)* | Fast win/block/open-four checks, then PVS + TT + VCF deep search |
| `search` | PVS + transposition table + VCF at leaf nodes, full time budget |
| `vcf` | Forced-four (VCF) sequences only; falls back to greedy |
| `vct` | Victory by Consecutive Threats — VCF extended with open-three forks |
| `tactical` | Immediate win/block only, no lookahead |
| `pattern` | Greedy positional heuristic, good for opening moves |

`adaptive` is the recommended default: it applies fast heuristic checks for common
cases (immediate wins, open-four threats) and delegates the rest to `search`.

## Architecture

The project is a Cargo workspace at the repo root with two crates:

```
gomoku-bots/
├── Cargo.toml              workspace root
├── crates/
│   ├── gomoku-core/        pure library — board, scoring, strategies, protocol types
│   └── gomoku-cli/         binary — SignalR client, runtime loop, UI server, local server
└── slides/                 presentation (Slidev) — package.json + slides.md
```

### Core data flow (bot mode)

```
SignalRClient  →  HubEvent
                    └── Runtime
                          ├── maintains board + turn state
                          ├── on GameStarted / MovePlayed → try_play_next_move()
                          └── timed_select() → SearchStrategy → PlayMove
```

### Strategy selection (`adaptive`)

Priority order inside `timed_select` for `--strategy adaptive`:

1. Immediate five-in-a-row → play it
2. Opponent five-in-a-row → block it
3. Move count < 4 → `PatternStrategy` (well-studied opening moves)
4. Opponent open-four threat and we have no counter-four → block it
5. Otherwise → `SearchStrategy` with full time budget

`StrategyRouter` (in `gomoku-core`) implements a similar priority tree and is used
by the router-aware path of `search` at narrow board states (≤ 8 candidates).

### Search (`search` / `SearchStrategy`)

- **IDDFS** (iterative-deepening depth-first search) within the time budget
- **PVS** (Principal Variation Search) — zero-window re-search reduces node count
- **Zobrist transposition table** — persists across depths; warm cache across turns
- **Killer + history heuristics** — better move ordering
- **VCF pre-check** — 200 ms budget to find forced-win sequences before IDDFS
- **VCF at leaf nodes** — quiescence extension to avoid horizon-effect blunders

### Board (`gomoku-core/src/board.rs`)

- 18 × 18 grid, Black plays first
- `score_move()` — directional pattern matching: five = 1 000 000, open-four = 100 000, closed-four = 25 000, open-three = 8 000, …
- `candidate_positions()` — empty cells within 2 steps of any placed stone
- `inspect()` — returns `BoardInsights` used by the strategy router

## Verification

```bash
cargo test
cargo run -p gomoku-cli -- --demo
```

For transport changes, also run a short live smoke test with guest mode.

## Slides

The `slides/` folder contains a Slidev presentation about the bot:

```bash
cd slides
npm install
npx slidev slides.md
```
