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
    /// HP lost by the player at the 10th/50th/90th percentile (higher = worse).
    /// Positive values mean damage taken; 0 means no damage at that percentile.
    pub hp_loss_p10: f32,
    pub hp_loss_p50: f32,
    pub hp_loss_p90: f32,
}

/// Result of a complete multi-turn combat simulation.
#[derive(Debug, Clone)]
pub struct CombatResult {
    /// HP change to the player over the full fight (negative = net damage taken).
    pub player_hp_delta: i32,
    /// Number of turns elapsed when combat ended.
    pub turns: u32,
    pub player_alive: bool,
    pub combat_won: bool,
    /// State snapshot at the end of combat.
    pub final_state: CombatState,
}

// Safety cap; prevents infinite loops against unkillable enemies.
const MAX_TURNS: u32 = 50;

/// Simulate a full combat to completion (win, loss, or turn limit).
///
/// If the state has no hand yet (e.g. freshly constructed), an opening hand
/// is drawn before the first turn. Otherwise plays from the existing hand.
pub fn run_combat(
    mut state: CombatState,
    policy: &dyn Policy,
    rng: &mut impl Rng,
) -> CombatResult {
    let initial_hp = state.player.hp;

    if state.hand.is_empty() {
        let n = state.hand_size as usize;
        apply::draw_cards(&mut state, n, rng);
    }

    loop {
        // Player phase: play cards until policy passes or combat ends
        loop {
            if state.is_over() { break; }
            match policy.select_action(&state) {
                None => break,
                Some(action) => {
                    if apply::play_card(
                        &mut state,
                        action.card_hand_idx,
                        action.target_idx,
                        rng,
                    ).is_err() {
                        break;
                    }
                }
            }
        }

        if state.is_over() || state.turn >= MAX_TURNS {
            break;
        }

        apply::end_turn(&mut state, rng);

        if state.is_over() {
            break;
        }
    }

    CombatResult {
        player_hp_delta: state.player.hp as i32 - initial_hp as i32,
        turns: state.turn,
        player_alive: state.player.is_alive(),
        combat_won: state.is_won(),
        final_state: state,
    }
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

fn percentile(sorted: &[f32], p: f32) -> f32 {
    if sorted.is_empty() {
        return 0.0;
    }
    let idx = ((p / 100.0) * (sorted.len() - 1) as f32).round() as usize;
    sorted[idx.min(sorted.len() - 1)]
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
    let mut hp_losses: Vec<f32> = Vec::with_capacity(n as usize);

    for _ in 0..n {
        let result = playout(state.clone(), policy, rng);
        total_damage += result.damage_dealt as u64;
        total_block += result.block_gained as u64;
        total_hp_delta += result.player_hp_delta as i64;
        hp_losses.push((-result.player_hp_delta).max(0) as f32);
        if result.player_alive && result.combat_over && result.damage_dealt >= state.enemies.iter().map(|e| e.hp).sum::<u32>() {
            wins += 1;
        }
        if result.player_alive {
            survivals += 1;
        }
    }

    hp_losses.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

    let f = n as f32;
    PlayoutStats {
        mean_damage_dealt: total_damage as f32 / f,
        mean_block_gained: total_block as f32 / f,
        mean_player_hp_delta: total_hp_delta as f32 / f,
        win_rate: wins as f32 / f,
        survival_rate: survivals as f32 / f,
        hp_loss_p10: percentile(&hp_losses, 10.0),
        hp_loss_p50: percentile(&hp_losses, 50.0),
        hp_loss_p90: percentile(&hp_losses, 90.0),
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

    // ── run_combat ─────────────────────────────────────────────────────────

    fn make_state_with_draw(hand: Vec<Card>, draw: Vec<Card>, enemy_hp: u32, enemy_intent: Intent) -> CombatState {
        let player = PlayerState::new(80, 80);
        let enemy = EnemyState::new("Enemy", enemy_hp, enemy_intent);
        let mut state = CombatState::new(player, vec![enemy], draw);
        state.hand = hand;
        state
    }

    #[test]
    fn run_combat_one_shot_kill() {
        // Player kills on turn 1; combat_won and alive
        let state = make_state(vec![strike()], 6, Intent::Attack(5));
        let result = run_combat(state, &GreedyDamagePolicy, &mut rng());
        assert!(result.combat_won);
        assert!(result.player_alive);
        assert_eq!(result.turns, 1);
    }

    #[test]
    fn run_combat_multi_turn_kill() {
        // Turn 1: 1 Strike (6 dmg) → 12 HP left. Turn 2: draws 3 more Strikes → kills.
        // Fight spans at least 2 turns.
        let draw_pile = vec![strike(), strike()];
        let state = make_state_with_draw(vec![strike()], draw_pile, 18, Intent::Attack(3));
        let result = run_combat(state, &GreedyDamagePolicy, &mut rng());
        assert!(result.combat_won);
        assert!(result.player_alive);
        assert!(result.turns >= 2);
    }

    #[test]
    fn run_combat_player_dies() {
        // Enemy hits for 80 each turn and player has no cards
        let state = make_state(vec![], 999, Intent::Attack(80));
        let result = run_combat(state, &GreedyDamagePolicy, &mut rng());
        assert!(!result.player_alive);
        assert!(!result.combat_won);
    }

    #[test]
    fn run_combat_hp_delta_accumulates() {
        // Enemy hits for 5 per turn; player draws nothing, fight goes to turn limit
        let state = make_state(vec![], 999, Intent::Attack(5));
        let result = run_combat(state, &GreedyDamagePolicy, &mut rng());
        // HP delta must be negative (took damage over multiple turns)
        assert!(result.player_hp_delta < 0);
    }

    #[test]
    fn run_combat_draws_opening_hand_when_empty() {
        // State has no hand but a draw pile — run_combat should deal it
        let player = PlayerState::new(80, 80);
        let enemy = EnemyState::new("Enemy", 6, Intent::Attack(0));
        let draw = vec![strike()];
        let state = CombatState::new(player, vec![enemy], draw);
        assert!(state.hand.is_empty());
        let result = run_combat(state, &GreedyDamagePolicy, &mut rng());
        assert!(result.combat_won);
    }

    // TODO: REMOVE — diagnostic trace of a real Act 1 fight (Axe Raider)
    #[test]
    #[ignore]
    fn trace_axe_raider_fight() {
        use crate::data::api::load_monsters;
        use crate::domain::catalog::ironclad;
        use crate::domain::encounter::monster_to_enemy;
        use crate::domain::effect::BuffType;
        use crate::sim::apply;
        use crate::sim::policy::GreedyDamagePolicy;

        let monsters = load_monsters();
        let axe_raider = monsters.iter().find(|m| m.id == "AXE_RUBY_RAIDER")
            .expect("AXE_RUBY_RAIDER not found in data");

        let deck = ironclad::starter_deck();
        let player = crate::domain::combat::PlayerState::new(80, 80);
        let enemy = monster_to_enemy(axe_raider);
        let mut state = CombatState::new(player, vec![enemy], deck);
        let initial_hp = state.player.hp;
        let mut rng = StdRng::seed_from_u64(42);

        // Deal opening hand
        let n = state.hand_size as usize;
        apply::draw_cards(&mut state, n, &mut rng);

        println!("\n=== Axe Raider fight (seed 42, Ironclad starter deck) ===");
        println!("Enemy: {} | HP: {} | Initial intent: {:?}",
            state.enemies[0].name, state.enemies[0].hp, state.enemies[0].intent);
        println!("Player HP: {}", state.player.hp);

        loop {
            let turn = state.turn;
            let enemy = &state.enemies[0];
            println!("\n--- Turn {} ---", turn);
            println!("  Enemy intent: {:?}  block: {}  HP: {}", enemy.intent, enemy.block, enemy.hp);
            let strength = enemy.buffs.get(&BuffType::Strength).copied().unwrap_or(0);
            if strength != 0 { println!("  Enemy Strength: {}", strength); }
            println!("  Player HP: {}  block: {}  energy: {}", state.player.hp, state.player.block, state.energy);
            println!("  Hand: [{}]", state.hand.iter().map(|c| c.name.as_str()).collect::<Vec<_>>().join(", "));

            // Player phase
            let mut played: Vec<String> = Vec::new();
            loop {
                if state.is_over() { break; }
                match GreedyDamagePolicy.select_action(&state) {
                    None => break,
                    Some(action) => {
                        let name = state.hand[action.card_hand_idx].name.clone();
                        if apply::play_card(&mut state, action.card_hand_idx, action.target_idx, &mut rng).is_ok() {
                            played.push(name);
                        } else {
                            break;
                        }
                    }
                }
            }
            println!("  Played: [{}]  → enemy HP now: {}", played.join(", "), state.enemies[0].hp);

            if state.is_over() || state.turn >= MAX_TURNS { break; }

            // Enemy phase
            apply::end_turn(&mut state, &mut rng);

            println!("  After enemy phase → player HP: {}  (next intent: {:?})",
                state.player.hp, state.enemies[0].intent);

            if state.is_over() { break; }
        }

        println!("\n=== Result ===");
        println!("  Won: {}  Player alive: {}  Turns: {}",
            state.is_won(), state.player.is_alive(), state.turn);
        println!("  Player HP delta: {}", state.player.hp as i32 - initial_hp as i32);
    }
}
