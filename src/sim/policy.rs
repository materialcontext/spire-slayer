use crate::domain::combat::{CombatState, Intent};

// ── Action ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
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

// ── GreedyDamagePolicy ─────────────────────────────────────────────────────

/// Always plays the highest base-damage playable card.
///
/// Exception: when the combined incoming attack damage from all living enemies
/// would kill the player (HP + current block ≤ incoming), and the player has
/// a playable block card, prefer the highest-block card instead. This prevents
/// the policy from ignoring lethal threats while mindlessly attacking.
pub struct GreedyDamagePolicy;

impl Policy for GreedyDamagePolicy {
    fn select_action(&self, state: &CombatState) -> Option<Action> {
        let target = select_target(state);

        // Sum up all incoming attack damage this turn.
        let incoming: u32 = state.enemies.iter()
            .filter(|e| e.is_alive())
            .map(|e| match e.intent {
                Intent::Attack(d)               => d,
                Intent::AttackMulti { damage, hits } => damage * hits,
                _                               => 0,
            })
            .sum();

        let lethal_threat = incoming > 0
            && incoming >= state.player.hp + state.player.block;

        if lethal_threat {
            // Try to play the highest-block card to survive.
            let best_block = state.hand.iter().enumerate()
                .filter(|(_, c)| c.is_playable(state.energy) && state.stars >= c.star_cost as u32 && c.base_block() > 0)
                .max_by_key(|(_, c)| c.base_block());
            if let Some((i, _)) = best_block {
                return Some(Action { card_hand_idx: i, target_idx: target });
            }
        }

        state
            .hand
            .iter()
            .enumerate()
            .filter(|(_, c)| c.is_playable(state.energy) && state.stars >= c.star_cost as u32)
            .max_by_key(|(_, c)| c.base_damage())
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
        // heavy_strike does 14 damage (index 1)
        assert_eq!(action.card_hand_idx, 1);
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
