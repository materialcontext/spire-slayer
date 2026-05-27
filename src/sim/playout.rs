use rand::Rng;

use super::apply;
use super::policy::{Action, Policy};
use crate::domain::combat::CombatState;

// ── Result types ───────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct PlayoutResult {
    /// Total damage dealt to all enemies this turn.
    pub damage_dealt: u32,
    /// Block the player accumulated before the enemy phase.
    pub block_gained: u32,
    /// HP change to the player across the full turn (negative = net damage taken).
    pub player_hp_delta: i32,
    /// True if combat ended (won or lost) after the enemy phase.
    pub combat_over: bool,
    pub player_alive: bool,
    /// State snapshot at end of the turn.
    pub final_state: CombatState,
    /// Ordered sequence of actions taken this turn.
    pub actions: Vec<Action>,
}

#[derive(Debug, Clone)]
pub struct PlayoutStats {
    pub mean_damage_dealt: f32,
    pub mean_block_gained: f32,
    pub mean_player_hp_delta: f32,
    /// Fraction of playouts that killed all enemies.
    pub win_rate: f32,
    /// Fraction of playouts where the player survived the enemy phase.
    pub survival_rate: f32,
}

// ── Core playout ───────────────────────────────────────────────────────────

/// Execute one full turn using `policy`, then run the enemy phase.
///
/// Takes ownership of `state` so the caller must clone if they need to
/// preserve the original.
pub fn playout(mut state: CombatState, policy: &dyn Policy, rng: &mut impl Rng) -> PlayoutResult {
    let initial_hp = state.player.hp;
    let initial_enemy_hp: u32 = state.enemies.iter().map(|e| e.hp).sum();
    let mut actions: Vec<Action> = Vec::new();

    // Player phase: play cards until the policy passes or combat ends
    loop {
        if state.is_over() {
            break;
        }
        match policy.select_action(&state) {
            None => break,
            Some(action) => {
                if apply::play_card(
                    &mut state,
                    action.card_hand_idx,
                    action.target_idx,
                    rng,
                )
                .is_ok()
                {
                    actions.push(action);
                } else {
                    // Policy returned an action that failed — stop to avoid
                    // infinite loops on a bad policy.
                    break;
                }
            }
        }
    }

    // Snapshot block before enemies absorb it
    let block_gained = state.player.block;

    // Enemy phase (no-op if combat is already over)
    if !state.is_over() {
        apply::end_turn(&mut state, rng);
    }

    let final_enemy_hp: u32 = state.enemies.iter().map(|e| e.hp).sum();

    PlayoutResult {
        damage_dealt: initial_enemy_hp.saturating_sub(final_enemy_hp),
        block_gained,
        player_hp_delta: state.player.hp as i32 - initial_hp as i32,
        combat_over: state.is_over(),
        player_alive: state.player.is_alive(),
        final_state: state,
        actions,
    }
}

/// Run `n` independent playouts from the same initial state.
///
/// Returns aggregate statistics. Useful for estimating win probability across
/// different draw orders.
pub fn playout_n(
    state: &CombatState,
    policy: &dyn Policy,
    n: u32,
    rng: &mut impl Rng,
) -> PlayoutStats {
    assert!(n > 0, "playout_n requires n > 0");

    let mut total_damage = 0u64;
    let mut total_block = 0u64;
    let mut total_hp_delta = 0i64;
    let mut wins = 0u32;
    let mut survivals = 0u32;

    for _ in 0..n {
        let result = playout(state.clone(), policy, rng);
        total_damage += result.damage_dealt as u64;
        total_block += result.block_gained as u64;
        total_hp_delta += result.player_hp_delta as i64;
        if result.player_alive && result.combat_over && result.damage_dealt >= state.enemies.iter().map(|e| e.hp).sum::<u32>() {
            wins += 1;
        }
        if result.player_alive {
            survivals += 1;
        }
    }

    let f = n as f32;
    PlayoutStats {
        mean_damage_dealt: total_damage as f32 / f,
        mean_block_gained: total_block as f32 / f,
        mean_player_hp_delta: total_hp_delta as f32 / f,
        win_rate: wins as f32 / f,
        survival_rate: survivals as f32 / f,
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::card::{Card, CardType, Rarity};
    use crate::domain::combat::{CombatState, EnemyState, Intent, PlayerState};
    use crate::domain::effect::CardEffect;
    use crate::sim::policy::{GreedyDamagePolicy, SequentialPolicy};
    use rand::SeedableRng;
    use rand::rngs::StdRng;

    fn rng() -> StdRng {
        StdRng::seed_from_u64(42)
    }

    fn strike() -> Card {
        Card::new(1, "Strike", 1, CardType::Attack, Rarity::Basic, vec![CardEffect::Damage(6)])
    }

    fn defend() -> Card {
        Card::new(2, "Defend", 1, CardType::Skill, Rarity::Basic, vec![CardEffect::Block(5)])
    }

    fn make_state(cards_in_hand: Vec<Card>, enemy_hp: u32, enemy_intent: Intent) -> CombatState {
        let player = PlayerState::new(80, 80);
        let enemy = EnemyState::new("Enemy", enemy_hp, enemy_intent);
        let mut state = CombatState::new(player, vec![enemy], vec![]);
        state.hand = cards_in_hand;
        state
    }

    #[test]
    fn playout_deals_damage() {
        let state = make_state(vec![strike()], 50, Intent::Attack(5));
        let result = playout(state, &GreedyDamagePolicy, &mut rng());
        assert_eq!(result.damage_dealt, 6);
    }

    #[test]
    fn playout_tracks_block_gained() {
        let state = make_state(vec![defend()], 50, Intent::Attack(5));
        let result = playout(state, &SequentialPolicy, &mut rng());
        assert_eq!(result.block_gained, 5);
    }

    #[test]
    fn playout_player_takes_net_damage_after_block() {
        // Defend gives 5 block, enemy does 5 → no damage taken
        let state = make_state(vec![defend()], 50, Intent::Attack(5));
        let result = playout(state, &SequentialPolicy, &mut rng());
        assert_eq!(result.player_hp_delta, 0); // 5 block absorbs 5 attack
    }

    #[test]
    fn playout_combat_over_when_enemy_killed() {
        let state = make_state(vec![strike()], 6, Intent::Attack(5));
        let result = playout(state, &GreedyDamagePolicy, &mut rng());
        assert!(result.combat_over);
        assert!(result.player_alive);
    }

    #[test]
    fn playout_actions_recorded() {
        let state = make_state(vec![strike(), defend()], 50, Intent::Attack(5));
        let result = playout(state, &GreedyDamagePolicy, &mut rng());
        // GreedyDamage will play Strike (6 dmg) then Defend (0 dmg, but still playable)
        assert!(!result.actions.is_empty());
    }

    #[test]
    fn playout_empty_hand_returns_zero_damage() {
        let state = make_state(vec![], 50, Intent::Attack(9));
        let result = playout(state, &GreedyDamagePolicy, &mut rng());
        assert_eq!(result.damage_dealt, 0);
        assert_eq!(result.block_gained, 0);
        // Player takes 9 damage from enemy attack
        assert_eq!(result.player_hp_delta, -9);
    }

    #[test]
    fn playout_n_aggregates_correctly() {
        let state = make_state(vec![strike()], 50, Intent::Attack(5));
        let stats = playout_n(&state, &GreedyDamagePolicy, 10, &mut rng());
        // All playouts with same hand have deterministic damage
        assert!((stats.mean_damage_dealt - 6.0).abs() < f32::EPSILON);
        // Player survives all runs (80 HP - 5 dmg = 75)
        assert!((stats.survival_rate - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn playout_n_win_rate_one_shot_enemy() {
        // Strike deals 6; enemy has 6 HP — kill every time
        let state = make_state(vec![strike()], 6, Intent::Attack(5));
        let stats = playout_n(&state, &GreedyDamagePolicy, 20, &mut rng());
        assert!((stats.win_rate - 1.0).abs() < f32::EPSILON);
    }
}
