# Spire-Slayer Implementation Plan

## Overview

Four layers remain: `sim/`, `metrics/`, `tui/`, and `input/`. They form a strict dependency DAG:

```
domain  ──►  metrics  ──►  sim  ──►  tui
  └──────────────────────► input ──► tui
```

No business logic in `tui/`; no I/O in `sim/` or `metrics/`. All layers communicate through `CombatState` and `RunState` values.

---

## Additional Cargo.toml Dependencies

```toml
crossterm = "0.28"   # promote from transitive dep to direct (already in Cargo.lock)
thiserror = "1"      # SimError in sim::apply
```

No async runtime needed. Flat-MC at budget=500 runs well under 1ms synchronously.

---

## Phase 1 — `metrics/`

Pure functions over domain types. No randomness, no mutation.

### `src/metrics/mod.rs`

```rust
pub mod combat;
pub mod deck;
pub mod card_pick;

pub use combat::{threat_score, survivability, kill_potential};
pub use deck::{deck_score, synergy_score};
pub use card_pick::{pick_score, CardAdvice};
```

### `src/metrics/combat.rs`

```rust
/// Total incoming damage this turn if all enemies act.
pub fn incoming_damage(state: &CombatState) -> u32

/// HP the player loses after block (clamped to 0).
pub fn net_damage_taken(state: &CombatState) -> u32

/// True if player dies this turn with no intervention.
pub fn is_lethal_turn(state: &CombatState) -> bool

/// 0.0–1.0: fraction of max_hp remaining.
pub fn survivability(state: &CombatState) -> f32

/// Expected total damage available from hand this turn.
pub fn kill_potential(state: &CombatState) -> u32

/// Incoming damage weighted by Vulnerable/Weak status.
pub fn threat_score(state: &CombatState) -> f32

/// Damage a card deals given current Strength/Weak.
pub fn effective_damage(card: &Card, player: &PlayerState, target: &EnemyState) -> u32

/// Block a card provides given current Dexterity/Frail.
pub fn effective_block(card: &Card, player: &PlayerState) -> u32
```

**Key rules:**
- Strength adds per hit on multi-hit attacks.
- Weak: `damage * 3 / 4` (integer, rounds down).
- Vulnerable: `damage * 3 / 2` (integer, rounds down).
- Dexterity adds flat block per card play; Frail: `block * 3 / 4`.

### `src/metrics/deck.rs`

```rust
/// Heuristic 0.0–1.0 deck rating for the current act.
pub fn deck_score(deck: &[Card], act: u8) -> f32

/// Count cards sharing a synergy axis (damage-amp, block-scaling, draw-engine, etc.).
pub fn synergy_score(deck: &[Card]) -> u32

pub fn mean_cost(deck: &[Card]) -> f32
pub fn attack_ratio(deck: &[Card]) -> f32
pub fn has_block_density(deck: &[Card], n: usize) -> bool
```

Synergy detection is axis-based (dominant `CardEffect` variant grouping), not name-based.
`deck_score` weights shift by act: act 1 values block density; act 2+ values synergy density.

### `src/metrics/card_pick.rs`

```rust
#[derive(Debug, Clone)]
pub struct CardAdvice {
    pub card_index: usize,  // index into offered slice
    pub score: f32,
    pub reason: String,     // one-liner explanation
}

/// Rank offered cards best-first against the current run state.
pub fn pick_score(offered: &[Card], run: &RunState) -> Vec<CardAdvice>

/// Score one card against an existing deck.
pub fn score_single(card: &Card, deck: &[Card], act: u8) -> f32
```

`score_single` factors: rarity weight, synergy delta, cost efficiency, deck dilution penalty.
`Passive(String)` effects score at flat 0.2 (relic interactions deferred to later phase).

---

## Phase 2 — `sim/`

### `src/sim/mod.rs`

```rust
pub mod apply;
pub mod policy;
pub mod playout;
pub mod mcts;

pub use playout::{playout, PlayoutResult};
pub use mcts::{best_play_sequence, PlayAdvice};
```

### `src/sim/apply.rs` ← most critical file

```rust
#[derive(Debug, thiserror::Error)]
pub enum SimError {
    #[error("card index {0} out of range")]
    InvalidCardIndex(usize),
    #[error("not enough energy: need {need}, have {have}")]
    NotEnoughEnergy { need: u8, have: u8 },
    #[error("card is not playable")]
    CardNotPlayable,
    #[error("invalid target index {0}")]
    InvalidTarget(usize),
}

/// Apply one card play, mutating state in place.
pub fn play_card(
    state: &mut CombatState,
    card_hand_idx: usize,
    target_idx: usize,
) -> Result<(), SimError>

/// Resolve enemy actions, reset block, draw new hand, advance turn counter.
/// Returns true if combat is over.
pub fn end_turn(state: &mut CombatState, rng: &mut impl rand::Rng) -> bool

/// Draw n cards; reshuffles discard into draw pile when empty.
pub fn draw_cards(state: &mut CombatState, n: usize, rng: &mut impl rand::Rng)

pub(crate) fn apply_effect(state: &mut CombatState, effect: &CardEffect, target_idx: usize)
```

**Key rules:**
- `play_card`: remove card from hand → spend energy → iterate effects → push to discard or exhaust pile.
- Effect order within a card is sequential (damage first, then draw, then buffs).
- Thorns damage reflects against the player inside `apply_effect` when relevant.
- Draw pile empty: shuffle `discard_pile` using provided RNG into `draw_pile`, clear discard.
- `end_turn`: resolve enemy intents → tick Poison → reset enemy block → reset player block → set `energy = energy_max` → increment `turn` → draw 5 cards.
- Never panics on bad input; all errors returned as `Err(SimError::...)`.

### `src/sim/policy.rs`

```rust
pub trait Policy: Send + Sync {
    fn select_action(&self, state: &CombatState) -> Option<Action>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Action {
    pub card_hand_idx: usize,
    pub target_idx: usize,
}

pub struct GreedyDamagePolicy;   // play by descending damage
pub struct SurvivalFirstPolicy;  // block-first when under lethal threat, else damage
pub struct MaxPlaysPolicy;       // lowest-cost cards first to maximise plays
pub struct SequentialPolicy;     // left-to-right; MCTS baseline
```

Target selection: lowest-HP living enemy (focus-fire). Implemented as a shared helper `select_target(state) -> usize`.
`SurvivalFirstPolicy` delegates to `metrics::combat::is_lethal_turn` to switch modes.

### `src/sim/playout.rs`

```rust
#[derive(Debug, Clone)]
pub struct PlayoutResult {
    pub damage_dealt: u32,
    pub block_gained: u32,
    pub player_hp_delta: i32,
    pub combat_over: bool,
    pub player_alive: bool,
    pub final_state: CombatState,
    pub actions: Vec<Action>,
}

#[derive(Debug, Clone)]
pub struct PlayoutStats {
    pub mean_damage_dealt: f32,
    pub mean_block_gained: f32,
    pub mean_player_hp_delta: f32,
    pub win_rate: f32,
    pub survival_rate: f32,
}

/// Execute one full turn using policy, then end_turn. Clones state; caller's state unchanged.
pub fn playout(state: CombatState, policy: &dyn Policy, rng: &mut impl rand::Rng) -> PlayoutResult

/// Run n playouts from the same initial state. Returns aggregate statistics.
pub fn playout_n(state: &CombatState, policy: &dyn Policy, n: u32, rng: &mut impl rand::Rng) -> PlayoutStats
```

### `src/sim/mcts.rs`

```rust
#[derive(Debug, Clone)]
pub struct PlayAdvice {
    pub actions: Vec<Action>,
    pub expected_damage: f32,
    pub expected_hp_retained: f32,
    pub rationale: String,
    pub simulation_count: u32,
}

/// Flat Monte Carlo: enumerate/sample play orderings, rank by composite score.
/// budget caps the number of simulations.
pub fn best_play_sequence(state: &CombatState, budget: u32, rng: &mut impl rand::Rng) -> PlayAdvice
```

**Algorithm:**
- For hand ≤ 5 playable cards: enumerate all permutations (max 5! = 120).
- For larger hands: random sampling up to `budget` iterations.
- Composite score: `1.0 * damage_dealt + 0.8 * block_gained + 2.0 * player_hp_delta`.
- `rationale` compares best sequence against `GreedyDamagePolicy` output; annotates key differences.

---

## Phase 3 — `input/`

### `src/input/event.rs`

```rust
#[derive(Debug, Clone)]
pub enum AppEvent {
    Key(crossterm::event::KeyEvent),
    StateUpdated(Box<CombatState>),
    CardPickConfirmed(usize),
    Quit,
    RunSim,
    Tick,
}

/// Spawn OS thread reading crossterm events → AppEvent channel.
pub fn spawn_event_loop(tick_rate_ms: u64) -> std::sync::mpsc::Receiver<AppEvent>
```

Uses `std::sync::mpsc` (no async runtime). Raw `KeyEvent`s forwarded; TUI layer interprets by mode.
Key bindings: `j`/`k` navigate, `Enter` confirm, `q` quit, `s` simulate, `e` edit state.

### `src/input/manual.rs`

```rust
pub enum InputField {
    PlayerHp, EnemyHp { index: usize }, EnemyIntent { index: usize },
    EnemyBlock { index: usize }, Energy, HandCard { index: usize },
    DrawPileSize, Turn,
}

pub struct ManualInputState {
    pub current_field: InputField,
    pub buffer: String,
    pub base: CombatState,
}

impl ManualInputState {
    pub fn new(base: CombatState) -> Self
    pub fn handle_char(&mut self, c: char)
    pub fn handle_backspace(&mut self)
    pub fn commit(&mut self) -> Result<(), String>  // parse buffer, write to base, advance field
    pub fn next_field(&mut self)                    // Tab: skip without committing
    pub fn is_complete(&self) -> bool
    pub fn build(self) -> CombatState
}
```

Intent shorthand parsed from text buffer: `"A12"` → `Attack(12)`, `"AM5x3"` → `AttackMulti{5,3}`, `"B"` → `Block`, `"?"` → `Unknown`.

---

## Phase 4 — `tui/`

### `src/tui/app.rs`

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppMode {
    CombatAdvice,
    CardPick,
    ManualInput,
    Simulating,
    Exiting,
}

pub struct App {
    pub mode: AppMode,
    pub combat: Option<CombatState>,
    pub run: Option<RunState>,
    pub play_advice: Option<PlayAdvice>,
    pub card_advice: Vec<CardAdvice>,
    pub input: Option<ManualInputState>,
    pub status_message: String,
    pub selected_row: usize,
}

impl App {
    pub fn new() -> Self                                          // starts in ManualInput mode
    pub fn handle_event(&mut self, event: AppEvent) -> bool      // false = quit
    pub fn run_simulation(&mut self, rng: &mut impl rand::Rng)
    pub fn load_combat(&mut self, state: CombatState)
    pub fn load_pick(&mut self, offered: Vec<Card>)
}

pub fn run_app() -> anyhow::Result<()>
```

`run_app`: init crossterm raw+alternate-screen → create `Terminal` → spawn event loop → event loop → restore terminal.
Simulation runs synchronously in the event loop (budget=500, < 1ms). Move to thread only if latency becomes observable.

### `src/tui/ui.rs`

Stateless rendering; takes `&App`, returns nothing (calls `frame.render_widget`).

```
┌─────────────────────────────────────────────────────────┐
│  COMBAT ADVICE   Floor 12 • Act 2 • Turn 3             │
├────────────────────┬────────────────────────────────────┤
│  HAND (4 cards)    │  ENEMIES                           │
│  > [Strike]  1e    │  Cultist  HP: 48/50  BLK: 0       │
│    [Defend]  1e    │  Intent:  Attack 9                 │
│    [Bash]    2e    │                                    │
│    [Iron W.] 1e    │  PLAYER  HP: 72/80  BLK: 0        │
├────────────────────┴────────────────────────────────────┤
│  RECOMMENDATION                                         │
│  Play: Bash → Strike → Defend → end turn               │
│  Expected: 21 dmg dealt, 5 block, -0 HP                │
│  Reason: Vulnerable from Bash amplifies Strike +50%    │
├─────────────────────────────────────────────────────────┤
│  [s]im  [e]dit  [q]uit          Threat: HIGH           │
└─────────────────────────────────────────────────────────┘
```

Colors: threat HIGH = red, MEDIUM = yellow, LOW = green. Block = blue. Recommended cards = bold cyan.

### `src/tui/widgets.rs`

Reusable, pure-function widget builders:

```rust
pub fn card_widget(card: &Card, highlight: bool) -> ratatui::widgets::Paragraph
pub fn enemy_row(enemy: &EnemyState, index: usize) -> ratatui::text::Line
pub fn hp_bar(current: u32, max: u32, width: u16) -> ratatui::widgets::Gauge
pub fn advice_spans(advice: &PlayAdvice, hand: &[Card]) -> ratatui::text::Text
pub fn intent_label(intent: &Intent) -> &'static str   // Block, Buff, Unknown, Escape
pub fn intent_label_owned(intent: &Intent) -> String   // Attack variants
```

---

## `src/main.rs` — Final Wiring

```rust
mod data;
mod domain;
mod input;
mod metrics;
mod sim;
mod tui;

fn main() -> anyhow::Result<()> {
    tui::app::run_app()
}
```

---

## Build Order and Milestones

### Milestone 1 — Simulation kernel

Files (in order):
1. `src/metrics/combat.rs`
2. `src/metrics/deck.rs`
3. `src/metrics/card_pick.rs`
4. `src/metrics/mod.rs`
5. `src/sim/apply.rs`

**Gate:** `cargo test` passes all `metrics/` and `sim::apply` unit tests.

### Milestone 2 — Full playout

1. `src/sim/policy.rs`
2. `src/sim/playout.rs`
3. `src/sim/mcts.rs`
4. `src/sim/mod.rs`

**Gate:** Integration test — starter deck combat → `best_play_sequence` → result non-empty, `expected_damage > 0`. Flat-MC at budget=500 completes in < 10ms.

### Milestone 3 — Input layer

1. `src/input/manual.rs`
2. `src/input/event.rs`
3. `src/input/mod.rs`

**Gate:** Unit tests for all `ManualInputState` field types and Intent parsing strings.

### Milestone 4 — TUI

1. `src/tui/widgets.rs`
2. `src/tui/ui.rs`
3. `src/tui/app.rs`
4. `src/tui/mod.rs`
5. `src/main.rs` — update to wire everything

**Gate:** `cargo run` shows TUI in manual-input mode; entering a combat state and pressing `s` shows advice. One integration test using `ratatui::backend::TestBackend` confirms render does not panic.

---

## Test Strategy

| Layer | Type | Key coverage |
|---|---|---|
| `metrics/combat` | Unit | Each buff modifier, lethal detection, multi-hit math |
| `metrics/deck` | Unit | Synergy grouping, empty deck, all-Status deck |
| `metrics/card_pick` | Unit | Sort order, reason non-empty, rarity weighting |
| `sim/apply` | Unit + integration | Each CardEffect, draw shuffle, end-turn sequence, SimError variants |
| `sim/policy` | Unit | Each policy selects expected card given crafted state |
| `sim/playout` | Integration | Full turn, combat-over detection, stat means |
| `sim/mcts` | Integration | Bash→Strike outscores Strike→Bash (Vulnerable amplification) |
| `input/manual` | Unit | All field types, invalid input rejection, Intent parsing |
| `input/event` | Integration | Key event forwarded correctly |
| `tui/app` | Integration | `AppEvent` transitions between modes |
| `tui/ui` | Integration | `TestBackend` render does not panic |

All tests are deterministic: RNG seeded with `rand::SeedableRng::seed_from_u64(42)` in test fixtures.
