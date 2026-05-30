use rand::Rng;
use rand::seq::SliceRandom;

use crate::data::api::{SpireApiEncounter, SpireApiMonster};
use crate::domain::ai::build_ai_script;
use crate::domain::card::Card;
use crate::domain::catalog::ironclad;
use crate::domain::combat::{CombatState, EnemyState, Intent, PlayerState};
use crate::domain::effect::BuffType;

/// Normalize the API `act` string to a sub-act canonical name.
///
/// The game randomly assigns one of two sub-acts per act at run start.
/// Act 1: "overgrowth" | "underdocks"
/// Act 2: "hive"
/// Act 3: "glory"
pub fn normalize_act(act: &str) -> &'static str {
    let s = act.to_lowercase();
    if s.contains("overgrowth") { return "overgrowth"; }
    if s.contains("underdock") { return "underdocks"; }
    if s.contains("hive") { return "hive"; }
    if s.contains("glory") { return "glory"; }
    if s.contains("boss") { return "boss"; }
    "other"
}

pub fn encounters_for_act<'a>(
    encounters: &'a [SpireApiEncounter],
    sub_act: &str,
) -> Vec<&'a SpireApiEncounter> {
    if sub_act == "all" {
        return encounters.iter().collect();
    }
    // "boss" across all sub-acts
    if sub_act == "boss" {
        return encounters
            .iter()
            .filter(|e| e.room_type.as_deref() == Some("Boss"))
            .collect();
    }
    encounters
        .iter()
        .filter(|e| {
            e.act
                .as_deref()
                .map(|a| normalize_act(a) == sub_act)
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
            enemy.ai_runtime.used_moves.insert(mid.clone());
            // Override initial intent with the correct first move
            if let Some(ref script) = enemy.ai_script {
                if let Some(data) = script.moves.get(&mid) {
                    enemy.intent = map_intent(&data.intent_str, data.damage, data.hits);
                }
            }
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
        "plating" | "metallicize" => Some(BuffType::Plating),
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

/// Like `encounter_to_combat` but uses the caller-supplied deck, HP, and relics.
///
/// Shuffles the deck and deals a starting hand (size adjusted by relics).
/// Applies start-of-combat relic effects before returning.
fn scale_intent_damage(intent: Intent, factor: f32) -> Intent {
    match intent {
        Intent::Attack(d) => Intent::Attack(((d as f32 * factor).round() as u32).max(d)),
        Intent::AttackMulti { damage, hits } => Intent::AttackMulti {
            damage: ((damage as f32 * factor).round() as u32).max(damage),
            hits,
        },
        other => other,
    }
}

pub fn encounter_to_combat_with_deck(
    enc: &SpireApiEncounter,
    all_monsters: &[SpireApiMonster],
    deck: &[Card],
    hp: u32,
    max_hp: u32,
    relics: &[String],
    ascension: u8,
    rng: &mut impl Rng,
) -> CombatState {
    let mut state = encounter_to_combat(enc, all_monsters);
    state.player.hp = hp.max(1);
    state.player.max_hp = max_hp;
    state.player.block = 0;
    state.player.buffs.clear();
    state.relics = relics.iter().cloned().collect();
    state.is_elite = enc.room_type.as_deref()
        .map(|rt| rt.eq_ignore_ascii_case("Elite"))
        .unwrap_or(false);

    apply_start_of_combat_relics(&mut state);

    let mut draw = deck.to_vec();
    draw.shuffle(rng);
    let draw_count = (state.hand_size as usize).min(draw.len());
    let hand: Vec<_> = draw.drain(..draw_count).collect();
    state.hand = hand;
    state.draw_pile = draw;
    state.discard_pile.clear();
    state.energy = state.energy_max;

    // A8+: enemies have more HP; A9+: enemies deal more damage
    if ascension >= 8 {
        for (i, enc_monster) in enc.monsters.iter().enumerate() {
            if let Some(enemy) = state.enemies.get_mut(i) {
                let monster_data = all_monsters.iter().find(|m| m.id == enc_monster.id);
                let asc_hp = monster_data
                    .and_then(|m| m.max_hp_ascension)
                    .map(|h| h.max(1) as u32)
                    .unwrap_or_else(|| ((enemy.max_hp as f32 * 1.2).round() as u32).max(1));
                enemy.hp = asc_hp.min(enemy.hp * 2); // cap at 2× to avoid absurd values
                enemy.max_hp = asc_hp;
            }
        }
    }
    if ascension >= 9 {
        for enemy in &mut state.enemies {
            enemy.intent = scale_intent_damage(enemy.intent.clone(), 1.15);
        }
    }

    state
}

/// Apply all start-of-combat relic effects to a freshly built CombatState.
pub fn apply_start_of_combat_relics(state: &mut CombatState) {
    use crate::domain::effect::BuffType;

    let apply_all_enemies = |state: &mut CombatState, buff: BuffType, stacks: i32| {
        for enemy in &mut state.enemies {
            if enemy.is_alive() {
                *enemy.buffs.entry(buff.clone()).or_insert(0) += stacks;
            }
        }
    };

    // ── Regent's Stars resource ────────────────────────────────────────────
    // DIVINE_RIGHT: Regent's starter relic — gain 3 Stars at start of combat
    if state.has_relic("DIVINE_RIGHT") { state.stars += 3; }
    // DIVINE_DESTINY: upgraded variant — gain 6 Stars
    if state.has_relic("DIVINE_DESTINY") { state.stars += 6; }

    // ── Opening block ──────────────────────────────────────────────────────
    if state.has_relic("ANCHOR")      { state.player.block += 10; }
    if state.has_relic("FAKE_ANCHOR") { state.player.block += 4; }

    // ── Opening hand size bonuses ──────────────────────────────────────────
    if state.has_relic("BAG_OF_PREPARATION") { state.hand_size = (state.hand_size + 2).min(10); }
    if state.has_relic("RING_OF_THE_SNAKE")  { state.hand_size = (state.hand_size + 2).min(10); }
    if state.has_relic("BIG_MUSHROOM")       { state.hand_size = (state.hand_size + 2).min(10); }
    if state.has_relic("BOOMING_CONCH") && state.is_elite {
        state.hand_size = (state.hand_size + 2).min(10);
    }

    // ── Energy bonuses ─────────────────────────────────────────────────────
    // LANTERN: +1 energy at combat start
    if state.has_relic("LANTERN") { state.energy_max += 1; }
    // PHILOSOPHERS_STONE: +1 energy per turn modelled as +1 energy_max
    if state.has_relic("PHILOSOPHERS_STONE") { state.energy_max += 1; }

    // ── Debuffs on all enemies ─────────────────────────────────────────────
    if state.has_relic("BAG_OF_MARBLES")     { apply_all_enemies(state, BuffType::Vulnerable, 1); }
    if state.has_relic("RED_MASK")           { apply_all_enemies(state, BuffType::Weak, 1); }
    if state.has_relic("TWISTED_FUNNEL")     { apply_all_enemies(state, BuffType::Poison, 4); }
    if state.has_relic("PHILOSOPHERS_STONE") { apply_all_enemies(state, BuffType::Strength, 1); }

    // ── Player buffs ───────────────────────────────────────────────────────
    if state.has_relic("BRONZE_SCALES") {
        *state.player.buffs.entry(BuffType::Thorns).or_insert(0) += 3;
    }
    if state.has_relic("SWORD_OF_JADE") {
        *state.player.buffs.entry(BuffType::Strength).or_insert(0) += 3;
    }
    if state.has_relic("GORGET") {
        *state.player.buffs.entry(BuffType::Plating).or_insert(0) += 4;
    }
    if state.has_relic("SLING_OF_COURAGE") && state.is_elite {
        *state.player.buffs.entry(BuffType::Strength).or_insert(0) += 2;
    }
    if state.has_relic("EMBER_TEA") {
        *state.player.buffs.entry(BuffType::Strength).or_insert(0) += 2;
    }

    // ── Direct damage to all enemies ───────────────────────────────────────
    if state.has_relic("FESTIVE_POPPER") {
        for enemy in &mut state.enemies {
            if enemy.is_alive() {
                // Bypass block — Festive Popper deals direct HP damage
                enemy.hp = enemy.hp.saturating_sub(9);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_act_variants() {
        assert_eq!(normalize_act("Act 1 - Overgrowth"), "overgrowth");
        assert_eq!(normalize_act("act 1 - overgrowth"), "overgrowth");
        assert_eq!(normalize_act("Act 1 - Underdocks"), "underdocks");
        assert_eq!(normalize_act("Act 2 - Hive"), "hive");
        assert_eq!(normalize_act("Act 3 - Glory"), "glory");
        assert_eq!(normalize_act("boss"), "boss");
        assert_eq!(normalize_act("Boss"), "boss");
        assert_eq!(normalize_act("unknown"), "other");
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
        let make = |act: &str, room_type: &str| SpireApiEncounter {
            id: act.to_string(),
            name: act.to_string(),
            room_type: Some(room_type.to_string()),
            is_weak: None,
            act: Some(act.to_string()),
            tags: vec![],
            monsters: vec![],
            loss_text: None,
        };
        let all = vec![
            make("Act 1 - Overgrowth", "Monster"),
            make("Act 1 - Overgrowth", "Monster"),
            make("Act 1 - Underdocks", "Monster"),
            make("Act 2 - Hive", "Monster"),
        ];
        assert_eq!(encounters_for_act(&all, "overgrowth").len(), 2);
        assert_eq!(encounters_for_act(&all, "underdocks").len(), 1);
        assert_eq!(encounters_for_act(&all, "hive").len(), 1);
        assert_eq!(encounters_for_act(&all, "all").len(), 4);
    }
}
