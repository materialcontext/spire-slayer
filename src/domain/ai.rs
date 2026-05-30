use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::data::api::{SpireApiAiBranch, SpireApiMonster};

// ── Domain types ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RepeatConstraint {
    CanRepeatForever,
    CanRepeatXTimes,
    UseOnlyOnce,
    CannotRepeat,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RandomBranch {
    pub move_id: String,
    pub weight: f32,
    pub repeat: RepeatConstraint,
    pub max_times: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AiCondition {
    HpAtOrAboveHalf,
    HpBelowHalf,
    SlotIndex(usize),
    AlwaysTrue,
    /// Unrecognised condition — branch is skipped; `resolve_to_move` falls back
    /// to `branches.first()` so at least one branch always fires.
    AlwaysFalse,
    /// Move has been used fewer than `threshold` times total (Knowledge Demon,
    /// Test Subject phase transitions).
    MoveUsedLessThan(String, u32),
    /// Move has been used at least `threshold` times total.
    MoveUsedAtLeast(String, u32),
    /// Any non-self enemy slot has HP == 0 (Queen's `HasAmalgamDied`).
    AllyDead,
    /// All non-self enemy slots have HP > 0 (Queen's `!HasAmalgamDied`).
    AllyAlive,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConditionalBranch {
    pub move_id: String,
    pub condition: AiCondition,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AiStateKind {
    Move { move_id: String, next: Option<String> },
    Random { branches: Vec<RandomBranch> },
    Conditional { branches: Vec<ConditionalBranch> },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiState {
    pub id: String,
    pub kind: AiStateKind,
}

/// A power (buff/debuff) applied when an enemy executes a move.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MovePower {
    pub power_id: String,
    pub target: Option<String>,
    pub amount: i32,
}

/// Raw move data stored in the script — Intent-type-agnostic so we avoid
/// circular imports between domain/ai and domain/combat.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiMoveData {
    pub intent_str: String,
    pub damage: u32,
    pub hits: u32,
    pub block: u32,
    pub powers: Vec<MovePower>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnemyAiScript {
    pub states: HashMap<String, AiState>,
    pub moves: HashMap<String, AiMoveData>,
    pub initial_state_id: String,
    /// For sleeping monsters (initial Move state has `next: None`): the state ID
    /// to transition to when the wake condition fires (3 enemy turns elapsed or
    /// unblocked damage received). `None` for non-sleeping monsters.
    pub wake_state_id: Option<String>,
}

impl EnemyAiScript {
    /// Find the first Move-type state whose move_id matches.
    pub fn find_state_by_move_id(&self, move_id: &str) -> Option<&AiState> {
        self.states.values().find(|s| {
            matches!(&s.kind, AiStateKind::Move { move_id: mid, .. } if mid == move_id)
        })
    }

    /// The move ID the enemy will execute on the first turn.
    pub fn initial_move_id(&self) -> Option<&str> {
        self.states.get(&self.initial_state_id).and_then(|s| {
            if let AiStateKind::Move { move_id, .. } = &s.kind {
                Some(move_id.as_str())
            } else {
                None
            }
        })
    }
}

/// Mutable per-enemy AI runtime state, updated each turn.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AiRuntime {
    pub current_state_id: String,
    /// Move being executed this turn (used for repeat tracking and block lookup).
    pub last_move_id: Option<String>,
    pub consecutive_count: u32,
    pub used_moves: HashSet<String>,
    /// How many times each move has been used in total (for Knowledge Demon /
    /// Test Subject phase-counter conditions).
    pub move_use_counts: HashMap<String, u32>,
    /// Number of turns spent in the initial sleeping state (Lagavulin Matriarch).
    pub sleep_turns: u32,
    /// Set when the enemy takes unblocked HP damage; cleared after being read in
    /// `advance_enemy_ai` (used to trigger Lagavulin's wake-from-sleep).
    pub took_unblocked_damage: bool,
}

// ── Builder ───────────────────────────────────────────────────────────────────

/// Build an `EnemyAiScript` from API monster data.  Returns `None` if the
/// monster has no usable attack pattern.
///
/// State types are inferred structurally because the `state_type` field is
/// always `null` in the live API data:
///   • `move_id` present            → Move state
///   • branches with `weight`       → Random state
///   • branches with `condition`    → Conditional state
///   • no move_id, no branches      → stub/placeholder, skipped
///
/// Duplicate state IDs (e.g. Mawler's twin `RAND` entries — one stub, one
/// real) are handled naturally: stubs are skipped, so only the real entry is
/// inserted into the HashMap.
pub fn build_ai_script(monster: &SpireApiMonster) -> Option<EnemyAiScript> {
    let pattern = monster.attack_pattern.as_ref()?;
    let initial_move_id = pattern.initial_move.as_deref()?;

    // ── Move data map ──────────────────────────────────────────────────────
    let moves: HashMap<String, AiMoveData> = monster
        .moves
        .iter()
        .map(|m| {
            let damage = m.damage.as_ref().and_then(|d| d.normal).unwrap_or(0).max(0) as u32;
            let hits = m.damage.as_ref().and_then(|d| d.hit_count).unwrap_or(1).max(1) as u32;
            let block = m.block.unwrap_or(0).max(0) as u32;
            let intent_str = m.intent.clone().unwrap_or_else(|| "Unknown".into());
            let powers = m.powers.iter().map(|p| MovePower {
                power_id: p.power_id.clone(),
                target: p.target.clone(),
                amount: p.amount.unwrap_or(0),
            }).collect();
            (m.id.clone(), AiMoveData { intent_str, damage, hits, block, powers })
        })
        .collect();

    // ── State map (structural type inference) ─────────────────────────────
    let mut states: HashMap<String, AiState> = HashMap::new();
    for api_state in &pattern.states {
        let kind = if let Some(ref move_id) = api_state.move_id {
            // Has a move_id → Move state
            AiStateKind::Move {
                move_id: move_id.clone(),
                next: api_state.next.clone(),
            }
        } else {
            let branches = &api_state.branches;
            if branches.is_empty() {
                continue; // stub / placeholder — skip
            }
            let has_weight = branches.iter().any(|b| b.weight.is_some());
            let has_cond   = branches.iter().any(|b| b.condition.is_some());
            if has_weight && !has_cond {
                let parsed: Vec<RandomBranch> =
                    branches.iter().filter_map(parse_random_branch).collect();
                if parsed.is_empty() { continue; }
                AiStateKind::Random { branches: parsed }
            } else if has_cond {
                let parsed: Vec<ConditionalBranch> =
                    branches.iter().filter_map(parse_conditional_branch).collect();
                if parsed.is_empty() { continue; }
                AiStateKind::Conditional { branches: parsed }
            } else {
                continue;
            }
        };
        states.insert(api_state.id.clone(), AiState { id: api_state.id.clone(), kind });
    }

    // ── Initial state lookup ───────────────────────────────────────────────
    // Find the Move state whose move_id matches the pattern's initial_move.
    // Falls back to using initial_move_id as the state ID directly (for
    // monsters whose INIT state is a routing node rather than a Move state).
    let initial_state_id = states
        .values()
        .find(|s| matches!(&s.kind, AiStateKind::Move { move_id, .. } if move_id == initial_move_id))
        .map(|s| s.id.clone())
        .unwrap_or_else(|| {
            // Check for a routing state whose ID matches initial_move_id
            if states.contains_key(initial_move_id) {
                initial_move_id.to_string()
            } else {
                initial_move_id.to_string()
            }
        });

    // ── Wake state for sleeping monsters (Lagavulin Matriarch) ────────────
    // A monster starts "sleeping" when its initial state is a Move node with
    // `next: None` (loops on itself indefinitely).  When the wake condition
    // fires (`sleep_turns >= 3` or unblocked damage), the AI jumps to the
    // first non-sleeping Move state that has an outgoing `next` pointer —
    // i.e. the beginning of the awake attack cycle.
    let initial_is_sleeping = states
        .get(&initial_state_id)
        .map(|s| matches!(&s.kind, AiStateKind::Move { next: None, .. }))
        .unwrap_or(false);

    let wake_state_id = if initial_is_sleeping {
        // Iterate states in original JSON order to pick the first awake Move state.
        pattern.states.iter()
            .find(|api_s| {
                api_s.id != initial_state_id
                    && states.get(&api_s.id)
                        .map(|s| matches!(&s.kind, AiStateKind::Move { next: Some(_), .. }))
                        .unwrap_or(false)
            })
            .map(|s| s.id.clone())
    } else {
        None
    };

    Some(EnemyAiScript { states, moves, initial_state_id, wake_state_id })
}

fn parse_random_branch(b: &SpireApiAiBranch) -> Option<RandomBranch> {
    let move_id = b.move_id.clone()?;
    let weight = b.weight.unwrap_or(1.0).max(0.0);
    let repeat = match b.repeat.as_deref() {
        Some("CanRepeatForever") => RepeatConstraint::CanRepeatForever,
        Some("CanRepeatXTimes") => RepeatConstraint::CanRepeatXTimes,
        Some("UseOnlyOnce") => RepeatConstraint::UseOnlyOnce,
        _ => RepeatConstraint::CannotRepeat,
    };
    let max_times = b.max_times.unwrap_or(1).max(1) as u32;
    Some(RandomBranch { move_id, weight, repeat, max_times })
}

fn parse_conditional_branch(b: &SpireApiAiBranch) -> Option<ConditionalBranch> {
    let move_id = b.move_id.clone()?;
    let condition = b.condition.as_deref().map(parse_condition).unwrap_or(AiCondition::AlwaysTrue);
    Some(ConditionalBranch { move_id, condition })
}

fn parse_condition(s: &str) -> AiCondition {
    // HP threshold conditions
    if s.contains("CurrentHp") {
        if s.contains(">=") && s.contains("MaxHp") { return AiCondition::HpAtOrAboveHalf; }
        if s.contains('<') && s.contains("MaxHp")  { return AiCondition::HpBelowHalf; }
    }
    // Slot-based conditions
    if s.contains("SlotName") {
        if s.contains("\"first\"")  { return AiCondition::SlotIndex(0); }
        if s.contains("\"second\"") { return AiCondition::SlotIndex(1); }
        if s.contains("\"third\"")  { return AiCondition::SlotIndex(2); }
        if s.contains("\"fourth\"") { return AiCondition::SlotIndex(3); }
    }
    // Knowledge Demon: _curseOfKnowledgeCounter tracks total CURSE_OF_KNOWLEDGE uses
    if s.contains("_curseOfKnowledgeCounter") {
        if let Some(n) = parse_u32_from(s) {
            if s.contains("< ")  { return AiCondition::MoveUsedLessThan("CURSE_OF_KNOWLEDGE".into(), n); }
            if s.contains(">=") { return AiCondition::MoveUsedAtLeast("CURSE_OF_KNOWLEDGE".into(), n); }
        }
    }
    // Test Subject: Respawns tracks how many times RESPAWN has executed
    if s.contains("Respawns") {
        if let Some(n) = parse_u32_from(s) {
            if s.contains("< ")  { return AiCondition::MoveUsedLessThan("RESPAWN".into(), n); }
            if s.contains(">=") { return AiCondition::MoveUsedAtLeast("RESPAWN".into(), n); }
        }
    }
    // Queen: whether the Torch Head Amalgam ally is still alive
    if s == "!HasAmalgamDied" { return AiCondition::AllyAlive; }
    if s == "HasAmalgamDied"  { return AiCondition::AllyDead; }
    // Fabricator: assume CanFabricate is true by default (fewer than 4 allies is
    // almost always the case; simulating ally tracking is out of scope)
    if s == "CanFabricate"  { return AiCondition::AlwaysTrue; }
    if s == "!CanFabricate" { return AiCondition::AlwaysFalse; }
    // Anything unrecognised: skip this branch; fallback fires the first branch.
    AiCondition::AlwaysFalse
}

/// Extract the first `u32` literal from a condition expression string.
fn parse_u32_from(s: &str) -> Option<u32> {
    s.split_whitespace().filter_map(|w| w.parse::<u32>().ok()).next()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::api::{
        ApiMoveDamage, ApiMonsterMove, SpireApiAiBranch, SpireApiAiState, SpireApiAttackPattern,
        SpireApiMonster,
    };

    fn make_monster(pattern_type: &str, initial_move: &str, states: Vec<SpireApiAiState>, moves: Vec<ApiMonsterMove>) -> SpireApiMonster {
        SpireApiMonster {
            id: "test".into(),
            name: "Test".into(),
            monster_type: None,
            min_hp: Some(40),
            max_hp: Some(50),
            min_hp_ascension: None,
            max_hp_ascension: None,
            moves,
            innate_powers: vec![],
            attack_pattern: Some(SpireApiAttackPattern {
                pattern_type: Some(pattern_type.into()),
                initial_move: Some(initial_move.into()),
                states,
                description: None,
            }),
        }
    }

    fn move_state(id: &str, move_id: &str, next: Option<&str>) -> SpireApiAiState {
        SpireApiAiState {
            id: id.into(),
            state_type: None, // mirrors real data; structural inference used
            move_id: Some(move_id.into()),
            must_perform_once: None,
            next: next.map(String::from),
            branches: vec![],
        }
    }

    fn random_state(id: &str, branches: Vec<SpireApiAiBranch>) -> SpireApiAiState {
        SpireApiAiState {
            id: id.into(),
            state_type: None,
            move_id: None,
            must_perform_once: None,
            next: None,
            branches,
        }
    }

    fn cond_state(id: &str, branches: Vec<SpireApiAiBranch>) -> SpireApiAiState {
        SpireApiAiState {
            id: id.into(),
            state_type: None,
            move_id: None,
            must_perform_once: None,
            next: None,
            branches,
        }
    }

    fn branch(move_id: &str, weight: f32, repeat: &str) -> SpireApiAiBranch {
        SpireApiAiBranch {
            move_id: Some(move_id.into()),
            weight: Some(weight),
            repeat: Some(repeat.into()),
            max_times: None,
            condition: None,
        }
    }

    fn cond_branch(move_id: &str, condition: &str) -> SpireApiAiBranch {
        SpireApiAiBranch {
            move_id: Some(move_id.into()),
            weight: None,
            repeat: None,
            max_times: None,
            condition: Some(condition.into()),
        }
    }

    fn api_move(id: &str, damage: i32) -> ApiMonsterMove {
        ApiMonsterMove {
            id: id.into(),
            name: Some(id.into()),
            intent: Some("Attack".into()),
            damage: Some(ApiMoveDamage { normal: Some(damage), ascension: None, hit_count: Some(1) }),
            block: None,
            heal: None,
            powers: vec![],
        }
    }

    #[test]
    fn cycle_pattern_finds_initial_state() {
        let monster = make_monster(
            "cycle", "SWING_1",
            vec![
                move_state("SWING_1", "SWING_1", Some("SWING_2")),
                move_state("SWING_2", "SWING_2", Some("BIG_SWING")),
                move_state("BIG_SWING", "BIG_SWING", Some("SWING_1")),
            ],
            vec![api_move("SWING_1", 5), api_move("SWING_2", 5), api_move("BIG_SWING", 12)],
        );
        let script = build_ai_script(&monster).unwrap();
        assert_eq!(script.initial_state_id, "SWING_1");
        assert_eq!(script.initial_move_id(), Some("SWING_1"));
        assert_eq!(script.states.len(), 3);
        assert_eq!(script.moves.len(), 3);
        assert!(script.wake_state_id.is_none());
    }

    #[test]
    fn random_pattern_parses_branches() {
        let monster = make_monster(
            "random", "INIT",
            vec![
                move_state("INIT_MOVE", "INIT", Some("RAND")),
                move_state("A_MOVE", "A", Some("RAND")),
                move_state("B_MOVE", "B", Some("RAND")),
                random_state("RAND", vec![
                    branch("A", 0.6, "CannotRepeat"),
                    branch("B", 0.4, "CannotRepeat"),
                ]),
            ],
            vec![api_move("INIT", 8), api_move("A", 10), api_move("B", 6)],
        );
        let script = build_ai_script(&monster).unwrap();
        let rand_state = script.states.get("RAND").unwrap();
        if let AiStateKind::Random { branches } = &rand_state.kind {
            assert_eq!(branches.len(), 2);
            assert_eq!(branches[0].weight, 0.6);
        } else {
            panic!("expected Random state");
        }
    }

    #[test]
    fn stub_states_are_skipped_and_real_random_state_wins() {
        // Mirrors Mawler / Soul Nexus data: duplicate RAND ID, first entry is a
        // stub (no branches), second is the real random state.
        let monster = make_monster(
            "random", "CLAW",
            vec![
                move_state("CLAW_MOVE", "CLAW", Some("RAND")),
                // stub — should be skipped
                SpireApiAiState {
                    id: "RAND".into(), state_type: None, move_id: None,
                    must_perform_once: None, next: None, branches: vec![],
                },
                // real random state
                random_state("RAND", vec![
                    branch("CLAW", 1.0, "CannotRepeat"),
                    branch("BITE", 1.0, "CannotRepeat"),
                ]),
            ],
            vec![api_move("CLAW", 6), api_move("BITE", 8)],
        );
        let script = build_ai_script(&monster).unwrap();
        let rand = script.states.get("RAND").unwrap();
        assert!(matches!(rand.kind, AiStateKind::Random { .. }), "stub must not overwrite real state");
        if let AiStateKind::Random { branches } = &rand.kind {
            assert_eq!(branches.len(), 2);
        }
    }

    #[test]
    fn sleeping_monster_computes_wake_state() {
        // Mirrors Lagavulin: initial Move state has next=None, awake cycle is separate.
        let monster = make_monster(
            "cycle", "SLEEP",
            vec![
                move_state("SLEEP_MOVE", "SLEEP", None),         // sleeping state
                move_state("SLASH_MOVE", "SLASH", Some("STAB_MOVE")), // awake cycle start
                move_state("STAB_MOVE", "STAB", Some("SLASH_MOVE")),
            ],
            vec![api_move("SLEEP", 0), api_move("SLASH", 10), api_move("STAB", 8)],
        );
        let script = build_ai_script(&monster).unwrap();
        assert_eq!(script.initial_state_id, "SLEEP_MOVE");
        assert_eq!(script.wake_state_id.as_deref(), Some("SLASH_MOVE"));
    }

    #[test]
    fn parse_condition_hp_threshold() {
        assert_eq!(parse_condition("base.Creature.CurrentHp >= base.Creature.MaxHp / 2"), AiCondition::HpAtOrAboveHalf);
        assert_eq!(parse_condition("base.Creature.CurrentHp < base.Creature.MaxHp / 2"), AiCondition::HpBelowHalf);
    }

    #[test]
    fn parse_condition_slot() {
        assert_eq!(parse_condition("base.Creature.SlotName == \"first\""), AiCondition::SlotIndex(0));
        assert_eq!(parse_condition("base.Creature.SlotName == \"third\""), AiCondition::SlotIndex(2));
    }

    #[test]
    fn parse_condition_move_use_counters() {
        assert_eq!(
            parse_condition("_curseOfKnowledgeCounter < 3"),
            AiCondition::MoveUsedLessThan("CURSE_OF_KNOWLEDGE".into(), 3)
        );
        assert_eq!(
            parse_condition("_curseOfKnowledgeCounter >= 3"),
            AiCondition::MoveUsedAtLeast("CURSE_OF_KNOWLEDGE".into(), 3)
        );
        assert_eq!(
            parse_condition("Respawns < 2"),
            AiCondition::MoveUsedLessThan("RESPAWN".into(), 2)
        );
        assert_eq!(
            parse_condition("Respawns >= 2"),
            AiCondition::MoveUsedAtLeast("RESPAWN".into(), 2)
        );
    }

    #[test]
    fn parse_condition_ally_status() {
        assert_eq!(parse_condition("!HasAmalgamDied"), AiCondition::AllyAlive);
        assert_eq!(parse_condition("HasAmalgamDied"),  AiCondition::AllyDead);
    }

    #[test]
    fn parse_condition_fabricator() {
        // CanFabricate defaults to AlwaysTrue (assume fewer than 4 allies)
        assert_eq!(parse_condition("CanFabricate"),  AiCondition::AlwaysTrue);
        assert_eq!(parse_condition("!CanFabricate"), AiCondition::AlwaysFalse);
    }

    #[test]
    fn parse_condition_unknown_defaults_to_always_false() {
        assert_eq!(parse_condition("HasBeetleCharged"), AiCondition::AlwaysFalse);
        assert_eq!(parse_condition("some_random_flag"),  AiCondition::AlwaysFalse);
    }

    #[test]
    fn build_ai_script_returns_none_without_pattern() {
        let monster = SpireApiMonster {
            id: "x".into(), name: "X".into(), monster_type: None,
            min_hp: Some(10), max_hp: Some(10),
            min_hp_ascension: None, max_hp_ascension: None,
            moves: vec![], innate_powers: vec![], attack_pattern: None,
        };
        assert!(build_ai_script(&monster).is_none());
    }

    #[test]
    fn move_block_is_captured() {
        let mut m = api_move("DEFEND_MOVE", 0);
        m.intent = Some("Defend".into());
        m.block = Some(8);
        let monster = make_monster(
            "cycle", "DEFEND_MOVE",
            vec![move_state("DEFEND_MOVE", "DEFEND_MOVE", Some("DEFEND_MOVE"))],
            vec![m],
        );
        let script = build_ai_script(&monster).unwrap();
        assert_eq!(script.moves["DEFEND_MOVE"].block, 8);
    }

    #[test]
    fn conditional_state_parses_correctly() {
        let monster = make_monster(
            "conditional", "ATTACK",
            vec![
                move_state("ATTACK_MOVE", "ATTACK", Some("BRANCH")),
                cond_state("BRANCH", vec![
                    cond_branch("ATTACK", "base.Creature.CurrentHp >= base.Creature.MaxHp / 2"),
                    cond_branch("POWER", "base.Creature.CurrentHp < base.Creature.MaxHp / 2"),
                ]),
                move_state("POWER_MOVE", "POWER", Some("BRANCH")),
            ],
            vec![api_move("ATTACK", 8), api_move("POWER", 0)],
        );
        let script = build_ai_script(&monster).unwrap();
        let branch_state = script.states.get("BRANCH").unwrap();
        if let AiStateKind::Conditional { branches } = &branch_state.kind {
            assert_eq!(branches.len(), 2);
            assert_eq!(branches[0].condition, AiCondition::HpAtOrAboveHalf);
            assert_eq!(branches[1].condition, AiCondition::HpBelowHalf);
        } else {
            panic!("expected Conditional state");
        }
    }
}
