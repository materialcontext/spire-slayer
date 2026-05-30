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
    /// Unrecognised condition expression — branch is skipped unless it is the
    /// only/last fallback (handled by the `.or_else(branches.first())` in
    /// `resolve_to_move`).
    AlwaysFalse,
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
}

// ── Builder ───────────────────────────────────────────────────────────────────

/// Build an `EnemyAiScript` from API monster data. Returns `None` if the
/// monster has no usable attack pattern.
pub fn build_ai_script(monster: &SpireApiMonster) -> Option<EnemyAiScript> {
    let pattern = monster.attack_pattern.as_ref()?;
    let initial_move_id = pattern.initial_move.as_deref()?;

    // Move data map
    let moves: HashMap<String, AiMoveData> = monster
        .moves
        .iter()
        .map(|m| {
            let damage = m.damage.as_ref().and_then(|d| d.normal).unwrap_or(0).max(0) as u32;
            let hits = m
                .damage
                .as_ref()
                .and_then(|d| d.hit_count)
                .unwrap_or(1)
                .max(1) as u32;
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

    // State map (later entries overwrite earlier ones with same ID)
    let mut states: HashMap<String, AiState> = HashMap::new();
    for api_state in &pattern.states {
        let kind = match api_state.state_type.as_deref() {
            Some("move") => {
                let Some(move_id) = api_state.move_id.clone() else { continue };
                AiStateKind::Move {
                    move_id,
                    next: api_state.next.clone(),
                }
            }
            Some("random") => AiStateKind::Random {
                branches: api_state
                    .branches
                    .iter()
                    .filter_map(parse_random_branch)
                    .collect(),
            },
            Some("conditional") => AiStateKind::Conditional {
                branches: api_state
                    .branches
                    .iter()
                    .filter_map(parse_conditional_branch)
                    .collect(),
            },
            _ => continue,
        };
        states.insert(api_state.id.clone(), AiState { id: api_state.id.clone(), kind });
    }

    // Find initial state: first Move-state whose move_id == initial_move_id
    let initial_state_id = states
        .values()
        .find(|s| matches!(&s.kind, AiStateKind::Move { move_id, .. } if move_id == initial_move_id))
        .map(|s| s.id.clone())
        .unwrap_or_else(|| initial_move_id.to_string());

    Some(EnemyAiScript { states, moves, initial_state_id })
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
        if s.contains(">=") && s.contains("MaxHp") {
            return AiCondition::HpAtOrAboveHalf;
        }
        if s.contains('<') && s.contains("MaxHp") {
            return AiCondition::HpBelowHalf;
        }
    }
    // Slot-based conditions
    if s.contains("SlotName") {
        if s.contains("\"first\"") { return AiCondition::SlotIndex(0); }
        if s.contains("\"second\"") { return AiCondition::SlotIndex(1); }
        if s.contains("\"third\"") { return AiCondition::SlotIndex(2); }
        if s.contains("\"fourth\"") { return AiCondition::SlotIndex(3); }
    }
    // Anything unrecognised (custom flags, ally counts, etc.) is treated as
    // never-matching so that real conditions further down the list can still
    // win. The `resolve_to_move` fallback ensures at least one branch fires.
    AiCondition::AlwaysFalse
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
            state_type: Some("move".into()),
            move_id: Some(move_id.into()),
            must_perform_once: None,
            next: next.map(String::from),
            branches: vec![],
        }
    }

    fn random_state(id: &str, branches: Vec<SpireApiAiBranch>) -> SpireApiAiState {
        SpireApiAiState {
            id: id.into(),
            state_type: Some("random".into()),
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
    fn parse_condition_unknown_defaults_to_always_false() {
        assert_eq!(parse_condition("CanFabricate"), AiCondition::AlwaysFalse);
        assert_eq!(parse_condition("HasBeetleCharged"), AiCondition::AlwaysFalse);
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
}
