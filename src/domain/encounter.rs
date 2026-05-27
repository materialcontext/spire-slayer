use crate::data::api::{SpireApiEncounter, SpireApiMonster};
use crate::domain::ai::build_ai_script;
use crate::domain::catalog::ironclad;
use crate::domain::combat::{CombatState, EnemyState, Intent, PlayerState};
use crate::domain::effect::BuffType;

/// Normalize the API `act` string to a short canonical form: "1", "2", "3", "boss", "other".
pub fn normalize_act(act: &str) -> &'static str {
    match act.to_lowercase().trim_matches(|c: char| !c.is_alphanumeric()) {
        s if s.contains('1') || s == "act1" || s == "one" => "1",
        s if s.contains('2') || s == "act2" || s == "two" => "2",
        s if s.contains('3') || s == "act3" || s == "three" => "3",
        s if s.contains("boss") || s.contains("4") => "boss",
        _ => "other",
    }
}

pub fn encounters_for_act<'a>(
    encounters: &'a [SpireApiEncounter],
    act_filter: &str,
) -> Vec<&'a SpireApiEncounter> {
    if act_filter == "all" {
        return encounters.iter().collect();
    }
    encounters
        .iter()
        .filter(|e| {
            e.act
                .as_deref()
                .map(|a| normalize_act(a) == act_filter)
                .unwrap_or(false)
        })
        .collect()
}

pub fn map_intent(intent_str: &str, damage: u32, hits: u32) -> Intent {
    let s = intent_str.to_lowercase();
    // Multi-hit attacks
    if (s.contains("attack") || s.contains("multi")) && hits > 1 {
        return Intent::AttackMulti { damage, hits };
    }
    if s.contains("attack") && damage > 0 {
        return Intent::Attack(damage);
    }
    if s.contains("attack") {
        // Attack with no damage value listed — treat as unknown damage
        return Intent::Attack(0);
    }
    if s.contains("defend") || s.contains("block") || s == "defend" {
        return Intent::Block;
    }
    if s.contains("strong_debuff") || s.contains("debuff") {
        return Intent::DebuffPlayer;
    }
    if s.contains("buff") || s.contains("magic") || s.contains("ritual") {
        return Intent::Buff;
    }
    if s.contains("escape") || s == "run" {
        return Intent::Escape;
    }
    Intent::Unknown
}

pub fn monster_to_enemy(monster: &SpireApiMonster) -> EnemyState {
    let hp = monster.max_hp.unwrap_or(50).max(1) as u32;

    let intent = monster
        .moves
        .first()
        .map(|m| {
            let intent_str = m.intent.as_deref().unwrap_or("unknown");
            let damage = m
                .damage
                .as_ref()
                .and_then(|d| d.normal)
                .unwrap_or(0)
                .max(0) as u32;
            let hits = m
                .damage
                .as_ref()
                .and_then(|d| d.hit_count)
                .unwrap_or(1)
                .max(1) as u32;
            map_intent(intent_str, damage, hits)
        })
        .unwrap_or(Intent::Unknown);

    let mut enemy = EnemyState::new(&monster.name, hp, intent);

    for power in &monster.innate_powers {
        if let Some(buff) = map_power_id(&power.power_id) {
            enemy.buffs.insert(buff, power.amount.unwrap_or(0) as i32);
        }
    }

    // Wire up the AI move script if the monster has one
    if let Some(script) = build_ai_script(monster) {
        let initial_move_id = script.initial_move_id().map(String::from);
        let initial_state_id = script.initial_state_id.clone();
        enemy.ai_script = Some(script);
        enemy.ai_runtime.current_state_id = initial_state_id;
        if let Some(mid) = initial_move_id {
            enemy.ai_runtime.last_move_id = Some(mid.clone());
            enemy.ai_runtime.used_moves.insert(mid);
        }
    }

    enemy
}

fn map_power_id(id: &str) -> Option<BuffType> {
    match id.to_lowercase().as_str() {
        "strength" => Some(BuffType::Strength),
        "dexterity" => Some(BuffType::Dexterity),
        "vulnerable" => Some(BuffType::Vulnerable),
        "weak" => Some(BuffType::Weak),
        "frail" => Some(BuffType::Frail),
        "poison" => Some(BuffType::Poison),
        "thorns" | "sharp_hide" => Some(BuffType::Thorns),
        "metallicize" => Some(BuffType::Metallicize),
        _ => None,
    }
}

/// Build a CombatState from an encounter, cross-referencing full monster data.
/// Enemies start at max HP; user can press `e` to adjust for mid-fight state.
pub fn encounter_to_combat(
    encounter: &SpireApiEncounter,
    all_monsters: &[SpireApiMonster],
) -> CombatState {
    let player = PlayerState::new(80, 80);

    let enemies: Vec<EnemyState> = encounter
        .monsters
        .iter()
        .filter_map(|em| all_monsters.iter().find(|m| m.id == em.id))
        .map(monster_to_enemy)
        .collect();

    let enemies = if enemies.is_empty() {
        // Fallback: one Unknown enemy per entry in the encounter monster list
        encounter
            .monsters
            .iter()
            .map(|em| EnemyState::new(&em.name, 50, Intent::Unknown))
            .collect()
    } else {
        enemies
    };

    let deck = ironclad::starter_deck();
    let mut state = CombatState::new(player, enemies, deck);
    let hand: Vec<_> = state.draw_pile.drain(..5.min(state.draw_pile.len())).collect();
    state.hand = hand;
    state
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_act_variants() {
        assert_eq!(normalize_act("1"), "1");
        assert_eq!(normalize_act("act1"), "1");
        assert_eq!(normalize_act("Act 1"), "1");
        assert_eq!(normalize_act("2"), "2");
        assert_eq!(normalize_act("boss"), "boss");
        assert_eq!(normalize_act("Boss"), "boss");
    }

    #[test]
    fn map_intent_attack() {
        assert_eq!(map_intent("Attack", 9, 1), Intent::Attack(9));
    }

    #[test]
    fn map_intent_multi() {
        assert_eq!(
            map_intent("Attack", 5, 3),
            Intent::AttackMulti { damage: 5, hits: 3 }
        );
    }

    #[test]
    fn map_intent_block() {
        assert_eq!(map_intent("Defend", 0, 1), Intent::Block);
        assert_eq!(map_intent("Block", 0, 1), Intent::Block);
    }

    #[test]
    fn map_intent_buff() {
        assert_eq!(map_intent("Buff", 0, 1), Intent::Buff);
        assert_eq!(map_intent("Magic", 0, 1), Intent::Buff);
    }

    #[test]
    fn map_intent_debuff() {
        assert_eq!(map_intent("Debuff", 0, 1), Intent::DebuffPlayer);
        assert_eq!(map_intent("Strong_Debuff", 0, 1), Intent::DebuffPlayer);
    }

    #[test]
    fn map_intent_escape() {
        assert_eq!(map_intent("Escape", 0, 1), Intent::Escape);
    }

    #[test]
    fn map_intent_unknown() {
        assert_eq!(map_intent("Sleep", 0, 1), Intent::Unknown);
    }

    #[test]
    fn monster_to_enemy_basic() {
        use crate::data::api::ApiMonsterMove;
        let monster = SpireApiMonster {
            id: "cultist".into(),
            name: "Cultist".into(),
            monster_type: None,
            min_hp: Some(40),
            max_hp: Some(48),
            min_hp_ascension: None,
            max_hp_ascension: None,
            moves: vec![ApiMonsterMove {
                id: "incantation".into(),
                name: Some("Incantation".into()),
                intent: Some("Buff".into()),
                damage: None,
                block: None,
                heal: None,
                powers: vec![],
            }],
            innate_powers: vec![],
            attack_pattern: None,
        };
        let enemy = monster_to_enemy(&monster);
        assert_eq!(enemy.name, "Cultist");
        assert_eq!(enemy.hp, 48);
        assert_eq!(enemy.intent, Intent::Buff);
    }

    #[test]
    fn encounter_to_combat_uses_max_hp() {
        use crate::data::api::{ApiEncounterMonster, ApiMonsterMove};
        let monster = SpireApiMonster {
            id: "jaw_worm".into(),
            name: "Jaw Worm".into(),
            monster_type: None,
            min_hp: Some(40),
            max_hp: Some(44),
            min_hp_ascension: None,
            max_hp_ascension: None,
            moves: vec![ApiMonsterMove {
                id: "chomp".into(),
                name: Some("Chomp".into()),
                intent: Some("Attack".into()),
                damage: Some(crate::data::api::ApiMoveDamage {
                    normal: Some(11),
                    ascension: None,
                    hit_count: Some(1),
                }),
                block: None,
                heal: None,
                powers: vec![],
            }],
            innate_powers: vec![],
            attack_pattern: None,
        };
        let encounter = SpireApiEncounter {
            id: "jaw_worm_fight".into(),
            name: "Jaw Worm".into(),
            room_type: Some("normal".into()),
            is_weak: Some(false),
            act: Some("1".into()),
            tags: vec![],
            monsters: vec![ApiEncounterMonster {
                id: "jaw_worm".into(),
                name: "Jaw Worm".into(),
            }],
            loss_text: None,
        };
        let combat = encounter_to_combat(&encounter, &[monster]);
        assert_eq!(combat.enemies.len(), 1);
        assert_eq!(combat.enemies[0].hp, 44);
        assert_eq!(combat.enemies[0].intent, Intent::Attack(11));
    }

    #[test]
    fn encounter_fallback_when_monster_not_found() {
        use crate::data::api::ApiEncounterMonster;
        let encounter = SpireApiEncounter {
            id: "mystery".into(),
            name: "Mystery Fight".into(),
            room_type: None,
            is_weak: None,
            act: Some("2".into()),
            tags: vec![],
            monsters: vec![ApiEncounterMonster {
                id: "unknown_monster".into(),
                name: "Unknown".into(),
            }],
            loss_text: None,
        };
        let combat = encounter_to_combat(&encounter, &[]);
        assert_eq!(combat.enemies.len(), 1);
        assert_eq!(combat.enemies[0].name, "Unknown");
    }

    #[test]
    fn encounters_for_act_filter() {
        let make = |act: &str| SpireApiEncounter {
            id: act.to_string(),
            name: act.to_string(),
            room_type: None,
            is_weak: None,
            act: Some(act.to_string()),
            tags: vec![],
            monsters: vec![],
            loss_text: None,
        };
        let all = vec![make("1"), make("1"), make("2"), make("boss")];
        assert_eq!(encounters_for_act(&all, "1").len(), 2);
        assert_eq!(encounters_for_act(&all, "2").len(), 1);
        assert_eq!(encounters_for_act(&all, "all").len(), 4);
    }
}
