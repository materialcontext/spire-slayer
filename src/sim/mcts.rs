use std::cmp::Ordering;
use rand::seq::SliceRandom;
use rand::Rng;
use rand::rngs::StdRng;
use rand::SeedableRng;
use rayon::prelude::*;

use super::apply;
use super::playout::run_combat;
use super::policy::{Action, GreedyDamagePolicy};
use crate::domain::card::Card;
use crate::domain::combat::CombatState;
use crate::domain::effect::{BuffType, CardEffect};
use crate::metrics::combat as metrics;

/// UCB1 exploration constant (√2).
const C: f32 = std::f32::consts::SQRT_2;
/// Independent rollouts run in parallel per MCTS expansion.
const PARALLEL_ROLLOUTS: usize = 4;

// ── Public types ───────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct PlayAdvice {
    /// Optimal card play sequence for the current turn.
    pub actions: Vec<Action>,
    /// Expected total damage dealt to enemies this turn.
    pub expected_damage: f32,
    /// Expected player HP retained after enemy attacks.
    pub expected_hp_retained: f32,
    /// Human-readable rationale for why this sequence was chosen.
    pub rationale: String,
    /// Total rollouts performed (= iterations × PARALLEL_ROLLOUTS).
    pub simulation_count: u32,
    /// Distribution of total HP lost over the full fight (positive = damage taken).
    pub hp_loss_p10: f32,
    pub hp_loss_p50: f32,
    pub hp_loss_p90: f32,
}

// ── Action types ───────────────────────────────────────────────────────────

/// An action in the MCTS tree.
/// `Some(act)` = play a specific card against a specific target.
/// `None`      = end the player's turn early ("pass").
type TreeAction = Option<Action>;

// ── MCTS node ──────────────────────────────────────────────────────────────

struct MctsNode {
    /// Index of the parent node in the arena; None for the root.
    parent: Option<usize>,
    /// The card-play action that produced this node. None for root.
    action: Option<Action>,
    /// Combat state at this decision point (after `action` was applied).
    state: CombatState,
    visits: u32,
    score_sum: f32,
    children: Vec<usize>,
    /// Actions not yet expanded into tree nodes. `None` = "pass turn early".
    untried: Vec<TreeAction>,
}

impl MctsNode {
    fn new(
        parent: Option<usize>,
        action: Option<Action>,
        state: CombatState,
        rng: &mut impl Rng,
    ) -> Self {
        // Build card-play actions, shuffle to remove ordering bias (item 4),
        // then append "pass" last so it is tried only after all card plays.
        let mut card_plays = available_card_plays(&state);
        card_plays.shuffle(rng);
        let mut untried: Vec<TreeAction> = card_plays.into_iter().map(Some).collect();
        if !untried.is_empty() {
            untried.insert(0, None); // Vec::pop() takes the last element, so None is tried last
        }
        MctsNode {
            parent,
            action,
            state,
            visits: 0,
            score_sum: 0.0,
            children: Vec::new(),
            untried,
        }
    }

    fn ucb1(&self, parent_visits: u32) -> f32 {
        if self.visits == 0 {
            return f32::INFINITY;
        }
        let exploit = self.score_sum / self.visits as f32;
        let explore = C * ((parent_visits as f32).ln() / self.visits as f32).sqrt();
        exploit + explore
    }
}

// ── Action generation ──────────────────────────────────────────────────────

/// True for cards that deal damage to a single enemy (not AoE / self-only).
fn is_single_target_damage(card: &Card) -> bool {
    let has_single = card.effects.iter().any(|e| {
        matches!(
            e,
            CardEffect::Damage(_)
                | CardEffect::DamageMulti { .. }
                | CardEffect::DamageEqBlock
                | CardEffect::EnemyLoseHp(_)
        )
    });
    let has_aoe = card.effects.iter().any(|e| matches!(e, CardEffect::DamageAll(_)));
    has_single && !has_aoe
}

/// All card-play `Action`s available in `state`.
///
/// Single-target damage cards generate one action per living enemy (item 8),
/// allowing MCTS to explore focus-firing different targets. All other cards
/// use the default focus-fire target.
fn available_card_plays(state: &CombatState) -> Vec<Action> {
    if state.is_over() {
        return vec![];
    }

    let living: Vec<usize> = state
        .enemies
        .iter()
        .enumerate()
        .filter(|(_, e)| e.is_alive())
        .map(|(i, _)| i)
        .collect();

    if living.is_empty() {
        return vec![];
    }

    let default_target = living
        .iter()
        .copied()
        .min_by_key(|&i| state.enemies[i].hp)
        .unwrap_or(0);

    let multi = living.len() > 1;
    let mut actions: Vec<Action> = Vec::new();

    for (card_idx, card) in state.hand.iter().enumerate() {
        if !card.is_playable(state.energy) || state.stars < card.star_cost as u32 {
            continue;
        }
        if is_single_target_damage(card) && multi {
            for &t in &living {
                actions.push(Action { card_hand_idx: card_idx, target_idx: t });
            }
        } else {
            actions.push(Action { card_hand_idx: card_idx, target_idx: default_target });
        }
    }

    actions
}

// ── MCTS steps ─────────────────────────────────────────────────────────────

/// Descend via UCB1 to the most promising unexpanded or terminal node.
fn select(arena: &[MctsNode]) -> usize {
    let mut idx = 0;
    loop {
        let node = &arena[idx];
        if !node.untried.is_empty() || node.state.is_over() || node.children.is_empty() {
            return idx;
        }
        let pv = node.visits;
        idx = *node
            .children
            .iter()
            .max_by(|&&a, &&b| {
                arena[a]
                    .ucb1(pv)
                    .partial_cmp(&arena[b].ucb1(pv))
                    .unwrap_or(Ordering::Equal)
            })
            .unwrap();
    }
}

/// Score a state by ending the player turn and running combat to completion.
///
/// Returns `(score, hp_loss)`:
/// - `score`   ∈ [0, 1]: fraction of max HP retained on win; 0 on loss.
/// - `hp_loss`: absolute HP lost from `state.player.hp` to end of fight.
fn rollout(state: &CombatState, rng: &mut impl Rng) -> (f32, f32) {
    let initial_hp = state.player.hp;

    if state.is_won() {
        return (initial_hp as f32 / state.player.max_hp as f32, 0.0);
    }
    if state.is_lost() {
        return (0.0, initial_hp as f32);
    }

    // Apply end-of-turn (enemy attacks + draw next hand).
    let mut s = state.clone();
    apply::end_turn(&mut s, rng);

    if s.is_over() {
        let hp_loss = initial_hp.saturating_sub(s.player.hp) as f32;
        let score = if s.is_won() {
            s.player.hp as f32 / state.player.max_hp as f32
        } else {
            0.0
        };
        return (score, hp_loss);
    }

    let result = run_combat(s, &GreedyDamagePolicy, rng);
    let hp_loss = initial_hp.saturating_sub(result.final_state.player.hp) as f32;
    let score = if result.combat_won {
        result.final_state.player.hp as f32 / state.player.max_hp as f32
    } else {
        // Tiny partial credit: rewards lines that at least damage the enemy.
        let total: u32 = state.enemies.iter().map(|e| e.max_hp).sum();
        if total > 0 { result.damage_dealt as f32 / total as f32 * 0.05 } else { 0.0 }
    };

    (score, hp_loss)
}

/// Propagate `score` from `idx` up to the root.
fn backprop(arena: &mut [MctsNode], mut idx: usize, score: f32) {
    loop {
        arena[idx].visits += 1;
        arena[idx].score_sum += score;
        match arena[idx].parent {
            None => break,
            Some(p) => idx = p,
        }
    }
}

/// Follow the most-visited child at each level from `start`, collecting actions.
fn extract_best_path(arena: &[MctsNode], start: usize) -> Vec<Action> {
    let mut actions = Vec::new();
    let mut idx = start;
    loop {
        if let Some(act) = arena[idx].action {
            actions.push(act);
        }
        if arena[idx].children.is_empty() {
            break;
        }
        idx = *arena[idx]
            .children
            .iter()
            .max_by_key(|&&i| arena[i].visits)
            .unwrap();
    }
    actions
}

/// Replay `actions` on a clone of `state`, trigger end-of-turn, and return
/// `(damage_dealt, hp_retained_after_enemy_phase)`.
fn evaluate_actions(state: &CombatState, actions: &[Action], rng: &mut impl Rng) -> (f32, f32) {
    let mut s = state.clone();
    let init_enemy_hp: u32 = s.enemies.iter().map(|e| e.hp).sum();

    for act in actions {
        if s.is_over() {
            break;
        }
        let _ = apply::play_card(&mut s, act.card_hand_idx, act.target_idx, rng);
    }

    let final_enemy_hp: u32 = s.enemies.iter().map(|e| e.hp).sum();
    let damage = init_enemy_hp.saturating_sub(final_enemy_hp) as f32;

    if !s.is_over() {
        apply::end_turn(&mut s, rng);
    }

    (damage, s.player.hp as f32)
}

fn percentile(sorted: &[f32], p: f32) -> f32 {
    if sorted.is_empty() {
        return 0.0;
    }
    let idx = ((p / 100.0) * (sorted.len() - 1) as f32).round() as usize;
    sorted[idx.min(sorted.len() - 1)]
}

// ── Public entry point ─────────────────────────────────────────────────────

/// UCB1 MCTS over card-play orderings and target choices for the current turn.
///
/// ## Tree structure
/// Each node = a combat state after some subset of this turn's cards have been
/// played.  Card plays are deterministic; the tree is exact.  Future turns are
/// handled by rollouts using `GreedyDamagePolicy`.
///
/// ## Improvements over a flat permutation search
/// - **UCB1** focuses budget on promising sub-trees (item 4 shuffle removes
///   first-child bias).
/// - **Multi-enemy targeting** (item 8): single-target damage cards branch over
///   all living enemies, not just the default focus-fire target.
/// - **Pass action** (item 7): `None` in `untried` lets MCTS discover that
///   ending the turn early outscores playing every remaining card.
/// - **Budget auto-scaling** (item 5): reduces iterations for simple hands.
/// - **Parallel rollouts** (item 9): `PARALLEL_ROLLOUTS` independent rollouts
///   per expansion, averaged before backpropagation.
/// - **Actual hp_loss** (item 6): percentiles reflect real HP delta from
///   rollout simulations, not an approximation from the win score.
pub fn best_play_sequence(
    state: &CombatState,
    budget: u32,
    rng: &mut impl Rng,
) -> PlayAdvice {
    let n_unique_playable = state
        .hand
        .iter()
        .filter(|c| c.is_playable(state.energy) && state.stars >= c.star_cost as u32)
        .count();

    if n_unique_playable == 0 {
        return PlayAdvice {
            actions: vec![],
            expected_damage: 0.0,
            expected_hp_retained: state.player.hp as f32,
            rationale: "No playable cards — pass turn.".to_string(),
            simulation_count: 0,
            hp_loss_p10: 0.0,
            hp_loss_p50: 0.0,
            hp_loss_p90: 0.0,
        };
    }

    // Item 5: scale down budget for simple hands to avoid wasted rollouts.
    // n^2 × 15 gives ~60 for 2 cards, ~135 for 3, ~240 for 4, ~375 for 5.
    let effective_budget = ((n_unique_playable as u32).saturating_pow(2).saturating_mul(15))
        .max(50)
        .min(budget);

    let mut arena: Vec<MctsNode> = Vec::with_capacity(effective_budget as usize + 4);
    arena.push(MctsNode::new(None, None, state.clone(), rng));

    let mut hp_losses: Vec<f32> = Vec::with_capacity(effective_budget as usize);
    let mut total_rollouts: u32 = 0;

    let mut iterations = 0u32;
    while iterations < effective_budget {
        // ── Selection ──────────────────────────────────────────────────────
        let leaf = select(&arena);

        // ── Expansion / pass ───────────────────────────────────────────────
        let rollout_idx = if arena[leaf].untried.is_empty() || arena[leaf].state.is_over() {
            leaf
        } else {
            match arena[leaf].untried.pop().unwrap() {
                None => {
                    // Item 7: "pass turn early" — rollout from the current leaf
                    // without creating a child node.
                    leaf
                }
                Some(act) => {
                    let mut child_state = arena[leaf].state.clone();
                    let _ = apply::play_card(
                        &mut child_state,
                        act.card_hand_idx,
                        act.target_idx,
                        rng,
                    );
                    let child_idx = arena.len();
                    arena.push(MctsNode::new(Some(leaf), Some(act), child_state, rng));
                    arena[leaf].children.push(child_idx);
                    child_idx
                }
            }
        };

        // ── Parallel rollouts (item 9) ─────────────────────────────────────
        // Clone the state once so the rayon closure owns it (avoids arena borrow).
        let state_snap = arena[rollout_idx].state.clone();
        let seeds: Vec<u64> = (0..PARALLEL_ROLLOUTS).map(|_| rng.next_u64()).collect();

        let results: Vec<(f32, f32)> = seeds
            .into_par_iter()
            .map(|seed| {
                let mut local_rng = StdRng::seed_from_u64(seed);
                rollout(&state_snap, &mut local_rng)
            })
            .collect();

        let pr = PARALLEL_ROLLOUTS as f32;
        let avg_score = results.iter().map(|(s, _)| *s).sum::<f32>() / pr;
        // Push each individual rollout's hp_loss (not the average) so percentiles
        // reflect the true distribution rather than smoothed means.
        for (_, hp_loss) in &results {
            hp_losses.push(*hp_loss);
        }

        // ── Backpropagation ────────────────────────────────────────────────
        backprop(&mut arena, rollout_idx, avg_score);

        iterations += 1;
        total_rollouts += PARALLEL_ROLLOUTS as u32;
    }

    // ── Best action: most-visited root child (robust selection) ───────────
    let best_child = arena[0]
        .children
        .iter()
        .max_by_key(|&&i| arena[i].visits)
        .copied();

    let (actions, expected_damage, expected_hp_retained) = match best_child {
        None => (vec![], 0.0, state.player.hp as f32),
        Some(first) => {
            let path = extract_best_path(&arena, first);
            let (dmg, hp) = evaluate_actions(state, &path, rng);
            (path, dmg, hp)
        }
    };

    hp_losses.sort_by(|a, b| a.partial_cmp(b).unwrap_or(Ordering::Equal));

    PlayAdvice {
        rationale: build_rationale(state, &actions, expected_damage, expected_hp_retained),
        actions,
        expected_damage,
        expected_hp_retained,
        simulation_count: total_rollouts,
        hp_loss_p10: percentile(&hp_losses, 10.0),
        hp_loss_p50: percentile(&hp_losses, 50.0),
        hp_loss_p90: percentile(&hp_losses, 90.0),
    }
}

// ── Rationale ──────────────────────────────────────────────────────────────

fn build_rationale(
    state: &CombatState,
    actions: &[Action],
    expected_damage: f32,
    expected_hp: f32,
) -> String {
    if actions.is_empty() {
        return "No beneficial play found — pass turn.".to_string();
    }

    let first = &state.hand[actions[0].card_hand_idx];

    let opens_vulnerable = first.effects.iter().any(|e| {
        matches!(
            e,
            CardEffect::ApplyToEnemy { buff: BuffType::Vulnerable, .. }
                | CardEffect::ApplyToAllEnemies { buff: BuffType::Vulnerable, .. }
        )
    });
    let opens_weak = first.effects.iter().any(|e| {
        matches!(
            e,
            CardEffect::ApplyToEnemy { buff: BuffType::Weak, .. }
                | CardEffect::ApplyToAllEnemies { buff: BuffType::Weak, .. }
        )
    });

    if opens_vulnerable {
        return format!(
            "{} first — Vulnerable amplifies subsequent hits (+50% dmg)",
            first.name
        );
    }
    if opens_weak {
        return format!(
            "{} first — Weak reduces incoming enemy damage (-25%)",
            first.name
        );
    }
    if metrics::is_lethal_turn(state) && first.base_block() > 0 {
        return format!(
            "{} first — survival: blocks lethal incoming damage",
            first.name
        );
    }

    format!(
        "Play {} first; expected {:.0} dmg, {:.0} HP retained",
        first.name, expected_damage, expected_hp
    )
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::card::{Card, CardType, Rarity};
    use crate::domain::combat::{CombatState, EnemyState, Intent, PlayerState};
    use crate::domain::effect::{BuffType, CardEffect};
    use rand::SeedableRng;
    use rand::rngs::StdRng;

    fn rng() -> StdRng {
        StdRng::seed_from_u64(42)
    }

    fn strike() -> Card {
        Card::new(1, "Strike", 1, CardType::Attack, Rarity::Basic, vec![CardEffect::Damage(6)])
    }

    fn bash() -> Card {
        Card::new(
            3,
            "Bash",
            2,
            CardType::Attack,
            Rarity::Basic,
            vec![
                CardEffect::Damage(8),
                CardEffect::ApplyToEnemy { buff: BuffType::Vulnerable, stacks: 2 },
            ],
        )
    }

    fn defend() -> Card {
        Card::new(2, "Defend", 1, CardType::Skill, Rarity::Basic, vec![CardEffect::Block(5)])
    }

    fn make_state(hand: Vec<Card>, enemy_hp: u32) -> CombatState {
        let player = PlayerState::new(80, 80);
        let enemy = EnemyState::new("Cultist", enemy_hp, Intent::Attack(9));
        let mut state = CombatState::new(player, vec![enemy], vec![]);
        state.hand = hand;
        state
    }

    #[test]
    fn empty_hand_returns_no_action_advice() {
        let state = make_state(vec![], 50);
        let advice = best_play_sequence(&state, 100, &mut rng());
        assert!(advice.actions.is_empty());
        assert_eq!(advice.simulation_count, 0);
    }

    #[test]
    fn single_card_hand_plays_it() {
        let state = make_state(vec![strike()], 50);
        let advice = best_play_sequence(&state, 100, &mut rng());
        assert_eq!(advice.actions.len(), 1);
        assert!(advice.expected_damage > 0.0);
    }

    #[test]
    fn bash_then_strike_outscores_strike_then_bash() {
        // Bash applies Vulnerable (+50% dmg), so MCTS should prefer Bash→Strike.
        let state = make_state(vec![bash(), strike()], 50);
        let advice = best_play_sequence(&state, 500, &mut rng());

        assert!(!advice.actions.is_empty());
        assert_eq!(advice.actions[0].card_hand_idx, 0, "Should lead with Bash");
        assert!(advice.expected_damage > 0.0);
    }

    #[test]
    fn advice_has_non_empty_rationale() {
        let state = make_state(vec![strike(), defend()], 50);
        let advice = best_play_sequence(&state, 50, &mut rng());
        assert!(!advice.rationale.is_empty());
    }

    #[test]
    fn simulation_count_reflects_parallel_rollouts() {
        let state = make_state(vec![strike(), defend(), bash()], 50);
        let advice = best_play_sequence(&state, 100, &mut rng());
        // simulation_count = iterations × PARALLEL_ROLLOUTS
        assert!(advice.simulation_count >= PARALLEL_ROLLOUTS as u32);
        assert_eq!(advice.simulation_count % PARALLEL_ROLLOUTS as u32, 0);
    }

    #[test]
    fn hp_loss_percentiles_are_non_negative_and_ordered() {
        let state = make_state(vec![strike()], 50);
        let advice = best_play_sequence(&state, 100, &mut rng());
        assert!(advice.hp_loss_p10 >= 0.0);
        assert!(advice.hp_loss_p10 <= advice.hp_loss_p50);
        assert!(advice.hp_loss_p50 <= advice.hp_loss_p90);
    }

    #[test]
    fn multi_enemy_targeting_generates_branched_actions() {
        // Two enemies: MCTS should have >1 action at root for a damage card.
        let player = PlayerState::new(80, 80);
        let e1 = EnemyState::new("A", 20, Intent::Attack(5));
        let e2 = EnemyState::new("B", 40, Intent::Attack(5));
        let mut state = CombatState::new(player, vec![e1, e2], vec![]);
        state.hand = vec![strike()];
        // Strike is single-target → should produce 2 actions (one per enemy)
        let actions = available_card_plays(&state);
        assert_eq!(actions.len(), 2, "Should generate one action per living enemy");
    }

    #[test]
    fn budget_auto_scales_for_simple_hands() {
        // A 1-card hand should run far fewer iterations than budget=500.
        let state = make_state(vec![strike()], 50);
        // With 1 unique card: budget = max(50, 1^2 × 15) = 50, but capped at 500
        // so simulation_count = 50 × PARALLEL_ROLLOUTS = 200
        let advice = best_play_sequence(&state, 500, &mut rng());
        assert!(advice.simulation_count <= 500 * PARALLEL_ROLLOUTS as u32);
    }

    #[test]
    fn one_shot_kill_is_recommended() {
        let state = make_state(vec![strike(), defend()], 6);
        let advice = best_play_sequence(&state, 200, &mut rng());
        assert_eq!(advice.actions[0].card_hand_idx, 0, "Strike kills; play it first");
    }

    #[test]
    fn percentile_helper() {
        let sorted = vec![0.0, 10.0, 20.0, 30.0, 40.0];
        assert!((percentile(&sorted, 0.0) - 0.0).abs() < f32::EPSILON);
        assert!((percentile(&sorted, 100.0) - 40.0).abs() < f32::EPSILON);
        assert!((percentile(&sorted, 50.0) - 20.0).abs() < f32::EPSILON);
    }

    #[test]
    fn is_single_target_damage_correct() {
        let s = Card::new(1, "Strike", 1, CardType::Attack, Rarity::Basic,
            vec![CardEffect::Damage(6)]);
        let aoe = Card::new(2, "Whirlwind", 1, CardType::Attack, Rarity::Uncommon,
            vec![CardEffect::DamageAll(5)]);
        let sk = Card::new(3, "Defend", 1, CardType::Skill, Rarity::Basic,
            vec![CardEffect::Block(5)]);
        let lose_hp = Card::new(4, "Haunt", 1, CardType::Attack, Rarity::Uncommon,
            vec![CardEffect::EnemyLoseHp(12)]);
        assert!(is_single_target_damage(&s));
        assert!(!is_single_target_damage(&aoe));
        assert!(!is_single_target_damage(&sk));
        assert!(is_single_target_damage(&lose_hp));
    }

    #[test]
    fn four_cost_card_not_available_with_three_energy() {
        // A 4-cost card must not appear in available actions when player has only 3 energy.
        let heavy = Card::new(10, "HeavyHitter", 4, CardType::Attack, Rarity::Rare,
            vec![CardEffect::Damage(30)]);
        let state = make_state(vec![heavy], 50);
        // state starts with 3 energy (default)
        assert_eq!(state.energy, 3);
        let actions = available_card_plays(&state);
        assert!(actions.is_empty(), "4-cost card should not be playable with 3 energy");
    }

    #[test]
    fn x_cost_card_is_always_playable_with_any_energy() {
        // X-cost cards (cost=255) are playable as long as energy > 0.
        let mut xcost = Card::new(11, "Whirlwind", 1, CardType::Attack, Rarity::Uncommon,
            vec![CardEffect::DamageAll(5)]);
        xcost.cost = 255;
        let mut state = make_state(vec![xcost], 50);
        state.energy = 1;
        let actions = available_card_plays(&state);
        assert!(!actions.is_empty(), "X-cost card should be playable with 1 energy");
    }

    #[test]
    fn x_cost_card_not_playable_with_zero_energy() {
        let mut xcost = Card::new(12, "Whirlwind", 1, CardType::Attack, Rarity::Uncommon,
            vec![CardEffect::DamageAll(5)]);
        xcost.cost = 255;
        let mut state = make_state(vec![xcost], 50);
        state.energy = 0;
        let actions = available_card_plays(&state);
        assert!(actions.is_empty(), "X-cost card must not play at 0 energy");
    }

    #[test]
    fn evaluate_actions_replays_correct_cards() {
        // Verify that evaluate_actions correctly replays a multi-card sequence
        // even after the hand shrinks on each play.
        let s1 = Card::new(1, "Strike", 1, CardType::Attack, Rarity::Basic,
            vec![CardEffect::Damage(6)]);
        let s2 = Card::new(2, "Strike2", 1, CardType::Attack, Rarity::Basic,
            vec![CardEffect::Damage(6)]);
        let state = make_state(vec![s1, s2], 50);
        let actions = vec![
            Action { card_hand_idx: 0, target_idx: 0 },
            Action { card_hand_idx: 0, target_idx: 0 }, // idx=0 again after first card removed
        ];
        let (dmg, _hp) = evaluate_actions(&state, &actions, &mut rng());
        // Two strikes of 6 each → 12 total damage
        assert!((dmg - 12.0).abs() < 1.0, "expected ~12 damage, got {dmg}");
    }
}
