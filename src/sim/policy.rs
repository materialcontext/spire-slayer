use std::cmp::Ordering;
use crate::domain::card::Card;
use crate::domain::combat::{CombatState, Intent};
use crate::domain::effect::{BuffType, CardEffect, OrbType};

// ── Action ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Action {
    /// Index into `CombatState::hand` at the time the action is taken.
    pub card_hand_idx: usize,
    /// Index into `CombatState::enemies` (ignored for AoE cards).
    pub target_idx: usize,
}

// ── Target selection ───────────────────────────────────────────────────────

/// Choose the best target given current state.
///
/// Default: lowest-HP living enemy (focus-fire to reduce incoming attacks).
/// Turn-1 exception: when all living enemies share the same name, prefer the
/// self-buffing one so it cannot compound its power before being killed.
pub fn select_target(state: &CombatState) -> usize {
    let living: Vec<(usize, &_)> = state
        .enemies
        .iter()
        .enumerate()
        .filter(|(_, e)| e.is_alive())
        .collect();

    if living.is_empty() {
        return 0;
    }

    if state.turn == 1 && living.len() > 1 {
        let all_same_type = living.windows(2).all(|w| w[0].1.name == w[1].1.name);
        if all_same_type {
            if let Some(&(i, _)) = living.iter().find(|(_, e)| e.intent == Intent::Buff) {
                return i;
            }
        }
    }

    living
        .iter()
        .min_by_key(|(_, e)| e.hp)
        .map(|&(i, _)| i)
        .unwrap_or(0)
}

// ── Policy trait ───────────────────────────────────────────────────────────

/// A pluggable play-order strategy.
///
/// Returns `None` to end the player's turn (no action selected).
pub trait Policy: Send + Sync {
    fn select_action(&self, state: &CombatState) -> Option<Action>;
}

// ── Card value scoring ─────────────────────────────────────────────────────

fn living_enemy_count(state: &CombatState) -> usize {
    state.enemies.iter().filter(|e| e.is_alive()).count()
}

fn player_focus(state: &CombatState) -> f32 {
    *state.player.buffs.get(&BuffType::Focus).unwrap_or(&0) as f32
}

fn orb_channel_value(orb: &OrbType, state: &CombatState) -> f32 {
    let f = player_focus(state);
    let n = living_enemy_count(state) as f32;
    match orb {
        // Passive: 3+f dmg per turn (~2 turns); evoke: 8+f dmg (prob ~40%)
        OrbType::Lightning => (3.0 + f) * 2.0 + (8.0 + f) * 0.4,
        // Passive: 2+f block per turn (~2 turns); evoke: 5+f block (prob ~40%)
        OrbType::Frost => ((2.0 + f) * 2.0 + (5.0 + f) * 0.4) * 0.85,
        // Grows by 6+f each passive; eventually evokes to lowest-HP enemy
        OrbType::Dark(val) => *val as f32 * 0.5 + f,
        // Passive: +1 energy/turn (~2 turns ≈ 10); evoke: +2 energy (prob ~40% ≈ 4)
        OrbType::Plasma => 10.0 + 4.0,
        // Passive: val dmg to all per turn (~2 turns); evoke: 2×val to all (prob ~40%)
        OrbType::Glass(val) => *val as f32 * n * 2.0 + *val as f32 * 2.0 * n * 0.4,
    }
}

fn orb_evoke_value(orb: &OrbType, state: &CombatState) -> f32 {
    let f = player_focus(state);
    let n = living_enemy_count(state) as f32;
    match orb {
        OrbType::Lightning => 8.0 + f,
        OrbType::Frost     => (5.0 + f) * 0.85,
        OrbType::Dark(val) => *val as f32 + f,
        OrbType::Plasma    => 10.0,
        OrbType::Glass(val)=> (*val as f32 * 2.0 + f) * n,
    }
}

fn buff_enemy_value(buff: &BuffType, stacks: i32) -> f32 {
    let s = stacks as f32;
    match buff {
        BuffType::Vulnerable => s * 6.0,
        BuffType::Weak       => s * 4.0,
        BuffType::Frail      => s * 3.0,
        BuffType::Poison     => s * 2.5,
        _                    => s * 1.0,
    }
}

fn buff_self_value(buff: &BuffType, stacks: i32, state: &CombatState) -> f32 {
    let s = stacks as f32;
    match buff {
        BuffType::Strength    => s * 3.0,
        BuffType::Dexterity   => s * 2.0,
        BuffType::Intangible  => s * 8.0,
        BuffType::NoxiousFumes=> s * 5.0,
        BuffType::Focus       => s * 5.0,
        BuffType::Barricade   => 4.0 + state.player.block as f32 * 0.5,
        BuffType::Plating     => s * 1.5,
        BuffType::Thorns      => s * 1.0,
        _                     => s * 1.0,
    }
}

/// Approximate HP-equivalent value of playing a card in the current state.
///
/// Each effect type is assigned a weight that reflects how much it contributes
/// to winning the combat in expected-damage-dealt or damage-prevented units.
pub fn card_play_value(card: &Card, state: &CombatState) -> f32 {
    let n = living_enemy_count(state) as f32;
    let mut score = 0.0f32;

    for effect in &card.effects {
        let delta: f32 = match effect {
            CardEffect::Damage(d)                   => *d as f32,
            CardEffect::DamageAll(d)                => *d as f32 * n,
            CardEffect::DamageMulti { base, hits }  => *base as f32 * *hits as f32,
            CardEffect::DamageEqBlock               => state.player.block as f32,
            // Block is slightly below equivalent damage in value
            CardEffect::Block(b)                    => *b as f32 * 0.85,
            // Draw and energy have high tempo value
            CardEffect::Draw(n)                     => *n as f32 * 4.0,
            CardEffect::GainEnergy(e)               => *e as f32 * 5.0,
            // HP loss is a real cost
            CardEffect::LoseHp(h)                   => -(*h as f32),
            CardEffect::GainStars(n)                => *n as f32 * 2.0,
            CardEffect::ApplyToEnemy { buff, stacks }
                => buff_enemy_value(buff, *stacks),
            CardEffect::ApplyToAllEnemies { buff, stacks }
                => buff_enemy_value(buff, *stacks) * n,
            CardEffect::ApplyToSelf { buff, stacks }
                => buff_self_value(buff, *stacks, state),
            CardEffect::ChannelOrb(orb)             => orb_channel_value(orb, state),
            CardEffect::EvokeOrb(count) => {
                let take = if *count == u32::MAX {
                    state.orbs.len()
                } else {
                    (*count as usize).min(state.orbs.len())
                };
                state.orbs.iter().rev().take(take)
                    .map(|orb| orb_evoke_value(orb, state))
                    .sum()
            }
            CardEffect::Passive(_) => 1.0,
        };
        score += delta;
    }

    score
}

// ── GreedyDamagePolicy ─────────────────────────────────────────────────────

/// Plays the highest-value card using a comprehensive scoring function.
///
/// Scores each playable card by its approximate HP-equivalent contribution,
/// accounting for damage, block, draw, energy, debuffs, self-buffs, and orbs.
///
/// Lethal exception: when combined incoming attacks would kill the player this
/// turn, prefer a kill-shot on the threatening enemy (removes future attacks)
/// or, if none is available, the highest-block card to survive.
pub struct GreedyDamagePolicy;

impl Policy for GreedyDamagePolicy {
    fn select_action(&self, state: &CombatState) -> Option<Action> {
        let target = select_target(state);

        let playable = |(_, c): &(usize, &Card)| -> bool {
            c.is_playable(state.energy) && state.stars >= c.star_cost as u32
        };

        // Sum up all incoming attack damage this turn.
        let incoming: u32 = state.enemies.iter()
            .filter(|e| e.is_alive())
            .map(|e| match e.intent {
                Intent::Attack(d)                    => d,
                Intent::AttackMulti { damage, hits } => damage * hits,
                _                                    => 0,
            })
            .sum();

        let lethal_threat = incoming > 0
            && incoming >= state.player.hp + state.player.block;

        if lethal_threat {
            // Prefer killing the threatening enemy to prevent future attacks.
            let threat_hp = state.enemies.get(target).map(|e| e.hp).unwrap_or(u32::MAX);
            let kill_shot = state.hand.iter().enumerate()
                .filter(|t| playable(t) && t.1.base_damage() >= threat_hp)
                .max_by_key(|(_, c)| c.base_damage());
            if let Some((i, _)) = kill_shot {
                return Some(Action { card_hand_idx: i, target_idx: target });
            }
            // No kill available — play highest-block card to survive.
            let best_block = state.hand.iter().enumerate()
                .filter(|t| playable(t) && t.1.base_block() > 0)
                .max_by_key(|(_, c)| c.base_block());
            if let Some((i, _)) = best_block {
                return Some(Action { card_hand_idx: i, target_idx: target });
            }
        }

        state
            .hand
            .iter()
            .enumerate()
            .filter(|t| playable(t))
            .max_by(|(_, a), (_, b)| {
                card_play_value(a, state)
                    .partial_cmp(&card_play_value(b, state))
                    .unwrap_or(Ordering::Equal)
            })
            .map(|(i, _)| Action { card_hand_idx: i, target_idx: target })
    }
}

// ── SequentialPolicy ───────────────────────────────────────────────────────

/// Plays cards left-to-right in hand order. Used as the MCTS rollout baseline.
#[allow(dead_code)]
pub struct SequentialPolicy;

impl Policy for SequentialPolicy {
    fn select_action(&self, state: &CombatState) -> Option<Action> {
        let target = select_target(state);
        state
            .hand
            .iter()
            .enumerate()
            .find(|(_, c)| c.is_playable(state.energy) && state.stars >= c.star_cost as u32)
            .map(|(i, _)| Action { card_hand_idx: i, target_idx: target })
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::card::{Card, CardType, Rarity};
    use crate::domain::combat::{CombatState, EnemyState, Intent, PlayerState};
    use crate::domain::effect::CardEffect;

    fn strike() -> Card {
        Card::new(1, "Strike", 1, CardType::Attack, Rarity::Basic, vec![CardEffect::Damage(6)])
    }

    fn heavy_strike() -> Card {
        Card::new(2, "Heavy Strike", 2, CardType::Attack, Rarity::Common, vec![CardEffect::Damage(14)])
    }

    fn defend() -> Card {
        Card::new(3, "Defend", 1, CardType::Skill, Rarity::Basic, vec![CardEffect::Block(5)])
    }

    fn state_with_hand(cards: Vec<Card>) -> CombatState {
        let player = PlayerState::new(80, 80);
        let enemy = EnemyState::new("Cultist", 50, Intent::Attack(9));
        let mut state = CombatState::new(player, vec![enemy], vec![]);
        state.hand = cards;
        state
    }

    #[test]
    fn greedy_picks_highest_damage() {
        let state = state_with_hand(vec![strike(), heavy_strike(), defend()]);
        let action = GreedyDamagePolicy.select_action(&state).unwrap();
        // heavy_strike scores 14 (index 1), strike scores 6, defend scores ~4.25
        assert_eq!(action.card_hand_idx, 1);
    }

    #[test]
    fn card_value_draw_beats_small_damage() {
        // A draw-2 card (value 8) should score higher than Strike (value 6)
        let draw_card = Card::new(4, "Acrobatics", 1, crate::domain::card::CardType::Skill, Rarity::Common,
            vec![CardEffect::Draw(2)]);
        let state = state_with_hand(vec![strike(), draw_card.clone()]);
        let va = card_play_value(&strike(), &state);
        let vb = card_play_value(&draw_card, &state);
        assert!(vb > va, "draw-2 ({vb}) should outscore strike ({va})");
    }

    #[test]
    fn card_value_energy_gain_is_high() {
        // Energy gain (5 per energy ≈ 10 for 2 energy) should score well
        let energy_card = Card::new(5, "Offering", 0, crate::domain::card::CardType::Skill, Rarity::Uncommon,
            vec![CardEffect::GainEnergy(2), CardEffect::LoseHp(3)]);
        let state = state_with_hand(vec![energy_card.clone()]);
        let v = card_play_value(&energy_card, &state);
        // GainEnergy(2)=10, LoseHp(3)=-3 → 7.0
        assert!((v - 7.0).abs() < 0.01, "expected 7.0 got {v}");
    }

    #[test]
    fn greedy_returns_none_when_no_energy() {
        let mut state = state_with_hand(vec![strike()]);
        state.energy = 0;
        assert!(GreedyDamagePolicy.select_action(&state).is_none());
    }

    #[test]
    fn sequential_picks_first_playable() {
        let state = state_with_hand(vec![strike(), heavy_strike()]);
        let action = SequentialPolicy.select_action(&state).unwrap();
        assert_eq!(action.card_hand_idx, 0);
    }

    #[test]
    fn greedy_blocks_when_lethal_threat() {
        // Enemy attacks for 15; player has 10 HP and 0 block → lethal.
        // Hand has Strike (6 dmg) and Defend (5 block). Should pick Defend.
        let player = PlayerState::new(10, 80);
        let enemy = EnemyState::new("Cultist", 50, Intent::Attack(15));
        let mut state = CombatState::new(player, vec![enemy], vec![]);
        state.hand = vec![strike(), defend()];
        let action = GreedyDamagePolicy.select_action(&state).unwrap();
        assert_eq!(action.card_hand_idx, 1, "should pick Defend (idx 1) to survive lethal");
    }

    #[test]
    fn greedy_attacks_when_threat_not_lethal() {
        // Enemy attacks for 5; player has 10 HP and 0 block → not lethal.
        // Should still pick highest damage card.
        let player = PlayerState::new(10, 80);
        let enemy = EnemyState::new("Cultist", 50, Intent::Attack(5));
        let mut state = CombatState::new(player, vec![enemy], vec![]);
        state.hand = vec![defend(), heavy_strike()];
        let action = GreedyDamagePolicy.select_action(&state).unwrap();
        assert_eq!(action.card_hand_idx, 1, "should pick Heavy Strike (idx 1) when not lethal");
    }

    #[test]
    fn select_target_picks_lowest_hp_enemy() {
        let player = PlayerState::new(80, 80);
        let strong = EnemyState::new("Cultist", 50, Intent::Unknown);
        let weak = EnemyState::new("Louse", 10, Intent::Unknown);
        let state = CombatState::new(player, vec![strong, weak], vec![]);
        assert_eq!(select_target(&state), 1); // weak is at index 1
    }
}
