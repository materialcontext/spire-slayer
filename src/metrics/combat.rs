use crate::domain::card::Card;
use crate::domain::combat::{CombatState, EnemyState, PlayerState};
use crate::domain::effect::{BuffType, CardEffect};

// ── Internal helpers ───────────────────────────────────────────────────────

fn strength_adjusted(base: u32, strength: i32) -> u32 {
    if strength >= 0 {
        base + strength as u32
    } else {
        base.saturating_sub((-strength) as u32)
    }
}

fn apply_modifiers(raw: u32, weak: bool, vulnerable: bool) -> u32 {
    let after_weak = if weak { raw * 3 / 4 } else { raw };
    if vulnerable { after_weak * 3 / 2 } else { after_weak }
}

// ── Public API ─────────────────────────────────────────────────────────────

/// Damage a card deals to a single target, accounting for Strength, Weak, and Vulnerable.
pub fn effective_damage(card: &Card, player: &PlayerState, target: &EnemyState) -> u32 {
    let strength = player.buff(&BuffType::Strength);
    let weak = player.buff(&BuffType::Weak) > 0;
    let vulnerable = target.buff(&BuffType::Vulnerable) > 0;

    card.effects
        .iter()
        .map(|e| match e {
            CardEffect::Damage(d) => {
                apply_modifiers(strength_adjusted(*d, strength), weak, vulnerable)
            }
            CardEffect::DamageAll(d) => {
                apply_modifiers(strength_adjusted(*d, strength), weak, vulnerable)
            }
            CardEffect::DamageMulti { base, hits } => {
                apply_modifiers(strength_adjusted(*base, strength), weak, vulnerable) * hits
            }
            _ => 0,
        })
        .sum()
}

/// Block a card provides, accounting for Dexterity and Frail.
pub fn effective_block(card: &Card, player: &PlayerState) -> u32 {
    let dex = player.buff(&BuffType::Dexterity).max(0) as u32;
    let frail = player.buff(&BuffType::Frail) > 0;

    let raw: u32 = card
        .effects
        .iter()
        .map(|e| match e {
            CardEffect::Block(b) => b + dex,
            _ => 0,
        })
        .sum();

    if frail { raw * 3 / 4 } else { raw }
}

/// Total damage from all living enemies this turn, accounting for enemy Weak and player Vulnerable.
pub fn incoming_damage(state: &CombatState) -> u32 {
    let player_vulnerable = state.player.buff(&BuffType::Vulnerable) > 0;

    state
        .enemies
        .iter()
        .filter(|e| e.is_alive())
        .map(|e| {
            use crate::domain::combat::Intent;
            let raw = match &e.intent {
                Intent::Attack(d) => *d,
                Intent::AttackMulti { damage, hits } => damage * hits,
                _ => 0,
            };
            let enemy_weak = e.buff(&BuffType::Weak) > 0;
            let after_weak = if enemy_weak { raw * 3 / 4 } else { raw };
            if player_vulnerable { after_weak * 3 / 2 } else { after_weak }
        })
        .sum()
}

/// HP the player will lose this turn after their current block absorbs incoming damage.
pub fn net_damage_taken(state: &CombatState) -> u32 {
    incoming_damage(state).saturating_sub(state.player.block)
}

/// Whether the player dies this turn with no further cards played.
pub fn is_lethal_turn(state: &CombatState) -> bool {
    net_damage_taken(state) >= state.player.hp
}

/// Fraction of max HP the player currently has (0.0–1.0).
pub fn survivability(state: &CombatState) -> f32 {
    if state.player.max_hp == 0 {
        return 0.0;
    }
    state.player.hp as f32 / state.player.max_hp as f32
}

/// Total damage available from cards in hand against the primary living enemy.
pub fn kill_potential(state: &CombatState) -> u32 {
    let Some(target) = state.enemies.iter().find(|e| e.is_alive()) else {
        return 0;
    };
    state
        .hand
        .iter()
        .map(|c| effective_damage(c, &state.player, target))
        .sum()
}

/// Ratio of incoming damage to player max HP; higher = more threatening.
pub fn threat_score(state: &CombatState) -> f32 {
    if state.player.max_hp == 0 {
        return 0.0;
    }
    incoming_damage(state) as f32 / state.player.max_hp as f32
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::card::{Card, CardType, Rarity};
    use crate::domain::combat::{CombatState, EnemyState, Intent, PlayerState};
    use crate::domain::effect::{BuffType, CardEffect};

    fn strike() -> Card {
        Card::new(1, "Strike", 1, CardType::Attack, Rarity::Basic, vec![CardEffect::Damage(6)])
    }

    fn twin_strike() -> Card {
        Card::new(
            2,
            "Twin Strike",
            1,
            CardType::Attack,
            Rarity::Common,
            vec![CardEffect::DamageMulti { base: 5, hits: 2 }],
        )
    }

    fn defend() -> Card {
        Card::new(3, "Defend", 1, CardType::Skill, Rarity::Basic, vec![CardEffect::Block(5)])
    }

    fn player() -> PlayerState {
        PlayerState::new(80, 80)
    }

    fn enemy() -> EnemyState {
        EnemyState::new("Cultist", 50, Intent::Attack(9))
    }

    #[test]
    fn effective_damage_base() {
        assert_eq!(effective_damage(&strike(), &player(), &enemy()), 6);
    }

    #[test]
    fn effective_damage_with_strength() {
        let mut p = player();
        *p.buffs.entry(BuffType::Strength).or_insert(0) = 3;
        assert_eq!(effective_damage(&strike(), &p, &enemy()), 9);
    }

    #[test]
    fn effective_damage_weak_rounds_down() {
        let mut p = player();
        *p.buffs.entry(BuffType::Weak).or_insert(0) = 2;
        // 6 * 3/4 = 4 (rounds down)
        assert_eq!(effective_damage(&strike(), &p, &enemy()), 4);
    }

    #[test]
    fn effective_damage_vulnerable_rounds_down() {
        let mut e = enemy();
        *e.buffs.entry(BuffType::Vulnerable).or_insert(0) = 2;
        // 6 * 3/2 = 9
        assert_eq!(effective_damage(&strike(), &player(), &e), 9);
    }

    #[test]
    fn effective_damage_multi_hit_applies_strength_per_hit() {
        let mut p = player();
        *p.buffs.entry(BuffType::Strength).or_insert(0) = 2;
        // (5+2)*2 = 14
        assert_eq!(effective_damage(&twin_strike(), &p, &enemy()), 14);
    }

    #[test]
    fn effective_block_base() {
        assert_eq!(effective_block(&defend(), &player()), 5);
    }

    #[test]
    fn effective_block_with_dexterity() {
        let mut p = player();
        *p.buffs.entry(BuffType::Dexterity).or_insert(0) = 2;
        assert_eq!(effective_block(&defend(), &p), 7);
    }

    #[test]
    fn effective_block_frail_rounds_down() {
        let mut p = player();
        *p.buffs.entry(BuffType::Frail).or_insert(0) = 2;
        // 5 * 3/4 = 3
        assert_eq!(effective_block(&defend(), &p), 3);
    }

    #[test]
    fn incoming_damage_single_enemy() {
        let p = PlayerState::new(80, 80);
        let e = EnemyState::new("Cultist", 50, Intent::Attack(9));
        let state = CombatState::new(p, vec![e], vec![]);
        assert_eq!(incoming_damage(&state), 9);
    }

    #[test]
    fn incoming_damage_with_player_vulnerable() {
        let mut p = PlayerState::new(80, 80);
        *p.buffs.entry(BuffType::Vulnerable).or_insert(0) = 2;
        let e = EnemyState::new("Cultist", 50, Intent::Attack(9));
        let state = CombatState::new(p, vec![e], vec![]);
        // 9 * 3/2 = 13
        assert_eq!(incoming_damage(&state), 13);
    }

    #[test]
    fn is_lethal_turn_detects_death() {
        let p = PlayerState::new(5, 80);
        let e = EnemyState::new("Cultist", 50, Intent::Attack(9));
        let state = CombatState::new(p, vec![e], vec![]);
        assert!(is_lethal_turn(&state));
    }

    #[test]
    fn is_not_lethal_with_enough_block() {
        let mut p = PlayerState::new(5, 80);
        p.block = 10;
        let e = EnemyState::new("Cultist", 50, Intent::Attack(9));
        let state = CombatState::new(p, vec![e], vec![]);
        assert!(!is_lethal_turn(&state));
    }

    #[test]
    fn survivability_full_hp() {
        let p = PlayerState::new(80, 80);
        let state = CombatState::new(p, vec![], vec![]);
        assert!((survivability(&state) - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn threat_score_proportional() {
        let p = PlayerState::new(80, 80);
        let e = EnemyState::new("Cultist", 50, Intent::Attack(8));
        let state = CombatState::new(p, vec![e], vec![]);
        assert!((threat_score(&state) - 0.1).abs() < 0.001);
    }
}
