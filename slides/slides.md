---
theme: default
class: text-center
highlighter: shiki
lineNumbers: false
transition: slide-left
title: Gomoku Bot — Rust
mdc: true
---

# Gomoku Bot
## A Rust strategy engine for Connect-5

<div class="pt-12 text-gray-400">Press Space to advance</div>

---
layout: two-cols
---

# Project Structure

**Cargo workspace** — `rust-gomoku-bot/`

```
crates/
├── gomoku-core/        # Pure library (no I/O)
│   ├── board.rs        18×18 grid + scoring
│   ├── engine.rs       DecisionEngine
│   ├── strategy/
│   │   ├── router.rs   Dispatch logic
│   │   ├── tactical.rs Immediate win/block
│   │   ├── pattern.rs  Score-based pick
│   │   ├── frontier.rs Pruned candidate set
│   │   ├── vcf.rs      Forced-win search
│   │   ├── search.rs   Alpha-beta IDDFS
│   │   └── search_tt.rs PVS + TT
│   ├── transposition_table.rs
│   └── zobrist.rs
└── gomoku-cli/         # Binary + I/O
    ├── client.rs       SignalR WebSocket
    ├── runtime.rs      Event-driven loop
    ├── local_server.rs Offline server
    └── web_server.rs   Browser UI
```

::right::

# Run Modes

| Mode | Flag |
|---|---|
| Online (guest) | `--mode guest` |
| Online (registered) | `--mode registered` |
| Browser UI | `--ui` |
| Offline server | `--local-server` |
| Smoke test | `--demo` |

<br/>

**Strategy override:**

```bash
--strategy tactical|pattern|search|adaptive
```

`adaptive` = use the router (default)

---

# Data Flow — Bot Mode

```mermaid
flowchart LR
    WS["SignalR WebSocket\nclient.rs"]
    RT["Runtime\nruntime.rs"]
    DE["DecisionEngine\nengine.rs"]
    SR["StrategyRouter"]
    CMD["PlayMove\ncommand"]

    WS -->|HubEvent| RT
    RT -->|GameStarted\nMovePlayed| DE
    DE --> SR
    SR -->|DecisionPlan| DE
    DE -->|Position| RT
    RT --> CMD
    CMD -->|JSON frame| WS
```

---

# Data Flow — UI Mode

```mermaid
flowchart TD
    Browser -->|"POST /start"| SRV["web_server.rs (axum)"]
    SRV -->|spawn| B1["Runtime — Bot 1"]
    SRV -->|spawn| B2["Runtime — Bot 2"]
    B1 -->|"watch::Sender"| UI["UiState\nui_state.rs"]
    B2 -->|"watch::Sender"| UI
    UI -->|"WebSocket /ws"| Browser
    SRV -->|"GET /"| HTML["index.html"]
```

---
layout: center
---

# Board & Scoring

18 × 18 grid — Black plays first

### Static pattern values (`score_move`)

| Pattern | Score |
|---|---|
| Five in a row | **1 000 000** |
| Open four (≥ 2 threats) | **100 000** |
| Closed four | **25 000** |
| Strong three | **8 000** |
| Solid extension | **2 000** |

`candidate_positions()` — empty cells within **2 steps** of any stone (center on empty board)

`position_score(pos) = score_move(me, pos) + score_move(opp, pos) + center_bonus`

---
layout: center
---

# Strategy Overview

| Strategy | Router trigger | Mechanism |
|---|---|---|
| **Tactical** | win / blocking move exists | Immediate pattern → respond |
| **Pattern** | opening (< 4 moves) or default | Top-12 candidates by static score |
| **Frontier** | medium board (≤ 20 candidates) | Top-8 candidates by static score |
| **VCF** | forced-win lookup | Depth-first consecutive-fours search |
| **Search** | tight board (≤ 8 candidates) | IDDFS alpha-beta, depth ≤ 12 |
| **Search-TT** | explicit `--strategy` flag | PVS + transposition table, depth ≤ 16 |

---

# Strategy Router

```mermaid
flowchart TD
    A["board.inspect(my_color)"] --> B{"winning moves\nexist?"}
    B -->|yes| TAC["⚡ Tactical"]
    B -->|no| C{"move_count < 4?"}
    C -->|yes| PAT["📖 Pattern"]
    C -->|no| D{"best score\n≥ 50 000?"}
    D -->|yes| TACP["⚡ Tactical → 📖 Pattern"]
    D -->|no| E{"candidates\n≤ 8?"}
    E -->|yes| SRCH["🔍 Search → ⚡ Tactical"]
    E -->|no| F{"candidates\n≤ 20?"}
    F -->|yes| FRON["🗺 Frontier → 📖 Pattern"]
    F -->|no| G["📖 Pattern → ⚡ Tactical"]
```

---
layout: two-cols
---

# Tactical Strategy

**Reflex layer — microsecond cost**

```
1. winning_moves(me)  → play it  ✓ (score: INT_MAX/4)
2. winning_moves(opp) → block it ✗ (score: INT_MAX/5)
3. for each candidate:
     score = mine×2 - theirs
   → return best
```

**Score labels:**
- `≥ 100 000` → "open four pressure"
- `≥ 8 000` → "threat building"
- else → "local tactical pressure"

::right::

# Pattern Strategy

**Opening phase & default fallback**

```
candidates = candidate_positions()
sort by position_score DESC
take top 12
→ return highest scoring
```

**Score labels:**
- `≥ 8 000` → "strong pattern"
- `≥ 2 000` → "solid extension"
- else → "positional improvement"

**Frontier** is Pattern with top-8 only — used when the board has ≤ 20 candidates (midgame focus).

---

# VCF — Victory by Consecutive Fours

> Only plays moves with score **≥ 25 000** (a four or better). Defender has exactly **one forced reply** → near-linear tree → depth 40 in milliseconds.

```mermaid
flowchart TD
    A["filter: score_move ≥ 25 000"] --> B["place candidate"]
    B --> C{"immediate\n5-in-a-row?"}
    C -->|yes| WIN["✓ return move"]
    C -->|no| D{"threats ≥ 2?\n(open four)"}
    D -->|yes| E{"defender\nimmediate win?"}
    E -->|no| WIN
    E -->|yes| SKIP["skip"]
    D -->|"= 1 threat"| F["vcf_recurse\n(forced block)"]
    F -->|found| WIN
    F -->|not found| SKIP
```

Also inverts attacker/defender to **block opponent's VCF win**.

---
layout: two-cols
---

# Search — IDDFS Alpha-Beta

**Tight endgame (≤ 8 candidates)**

```
root candidates : top 15 by static score
inner candidates: top  8 per node
depths          : 1 → 12
deadline        : 4.5 s
poll deadline   : every 512 nodes
```

Only fully-searched depths are committed — a depth interrupted by the deadline is discarded.

A greedy depth-0 seed ensures a move is always returned.

```mermaid
flowchart LR
    D1[depth 1] --> D2[depth 2]
    D2 --> D3[depth 3]
    D3 --> DN["... N"]
    DN -->|deadline| BEST["best\ncomplete depth"]
```

::right::

# Search-TT — PVS + Transposition Table

**Explicit flag only — strongest engine**

Extra layers on top of Search:
- Depths up to **16**, inner limit **12**
- **TT probe**: skip re-search if depth-sufficient entry found
- **TT best move**: tried first before all other ordering
- **Killer heuristic**: 2 slots per ply → `store_killer` on β-cutoff
- **History heuristic**: `depth²` bonus on β-cutoff

**2-tier TT (32 MB, 1M slots):**

| Tier | Replacement policy |
|---|---|
| 0 — always-replace | freshest data, fast lookup |
| 1 — depth-preferred | keeps deep results longer |

---

# Move Ordering in Search-TT

```mermaid
flowchart LR
    A["candidate\nposition"] --> B{"would_win(me)?"}
    B -->|yes| P1["100 000 000\ninstant win"]
    B -->|no| C{"would_win(opp)?"}
    C -->|yes| P2["90 000 000\nblock loss"]
    C -->|no| D{"TT best\nmove?"}
    D -->|yes| P3["80 000 000\ncached best"]
    D -->|no| E{"killer\nslot 0 / 1?"}
    E -->|yes| P4["10 / 9 000 000\nrefutation"]
    E -->|no| F["static + defensive\n+ history score"]
```

---
layout: center
---

# Layered Strategy Architecture

```mermaid
flowchart TB
    subgraph "Layer 1 — Reflex (µs)"
        TAC["⚡ Tactical\nwin / block pattern"]
    end
    subgraph "Layer 2 — Opening (µs)"
        PAT["📖 Pattern\nstatic score top-12"]
    end
    subgraph "Layer 3 — Midgame (ms)"
        FRON["🗺 Frontier  top-8"]
        VCF["🎯 VCF  forced-win depth-40"]
    end
    subgraph "Layer 4 — Endgame (s)"
        SRCH["🔍 Search  IDDFS α-β depth 12"]
        TT["🧠 Search-TT  PVS + TT depth 16"]
    end

    TAC -.->|"no immediate move"| PAT
    PAT -.->|"board grows"| FRON & VCF
    FRON & VCF -.->|"positions narrow"| SRCH
    SRCH -.->|"explicit flag"| TT
```

> The router picks **the cheapest strategy that produces a good move** — tactical reflexes cost microseconds, deep search is reserved for tight positions where it matters most.
