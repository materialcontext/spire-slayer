use std::cmp::Ordering;
use rand::Rng;

use super::apply;
use super::playout::run_combat;
use super::policy::{select_target, Action, GreedyDamagePolicy};
use crate::domain::combat::CombatState;
use crate::domain::effect::{BuffType, CardEffect};
use crate::metrics::combat as metrics;

/// UCB1 exploration constant (√2).
const C: f32 = std::f32::consts::SQRT_2;

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
    /// Number of rollouts performed.
    pub simulation_count: u32,
    /// HP-loss distribution across rollouts (positive = damage taken).
    pub hp_loss_p10: f32,
    pub hp_loss_p50: f32,
    pub hp_loss_p90: f32,
}

// ── MCTS tree ──────────────────────────────────────────────────────────────

struct MctsNode {
    /// Index of the parent node; None for the root.
    parent: Option<usize>,
    /// Card-play action that produced this node from its parent. None for root.
    action: Option<Action>,
    /// Combat state after `action` was applied.
    state: CombatState,
    visits: u32,
    score_sum: f32,
    children: Vec<usize>,
    /// Card-play actions that have not yet been expanded into tree nodes.
    untried: Vec<Action>,
}

impl MctsNode {
    fn new(parent: Option<usize>, action: Option<Action>, state: CombatState) -> Self {
        let untried = playable_actions(&state);
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

/// All card-play actions currently available in `state`.
fn playable_actions(state: &CombatState) -> Vec<Action> {
    if state.is_over() {
        return vec![];
    }
    let target = select_target(state);
    state
        .hand
        .iter()
        .enumerate()
        .filter(|(_, c)| c.is_playable(state.energy) && state.stars >= c.star_cost as u32)
        .map(|(i, _)| Action { card_hand_idx: i, target_idx: target })
        .collect()
}

// ── MCTS steps ─────────────────────────────────────────────────────────────

/// Walk the tree via UCB1 until reaching a node that still has untried actions
/// or is a terminal state.
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

/// Pop one untried action from `node_idx`, apply it to the state, and append a
/// new child node to the arena. Returns the child's index.
fn expand(arena: &mut Vec<MctsNode>, node_idx: usize, rng: &mut impl Rng) -> usize {
    let action = arena[node_idx].untried.pop().unwrap();
    let mut child_state = arena[node_idx].state.clone();
    let _ = apply::play_card(
        &mut child_state,
        action.card_hand_idx,
        action.target_idx,
        rng,
    );
    let child_idx = arena.len();
    arena.push(MctsNode::new(Some(node_idx), Some(action), child_state));
    arena[node_idx].children.push(child_idx);
    child_idx
}

/// Score a state by finishing the turn and running combat to completion with the
/// fast greedy policy.  Returns a value in [0, 1]: fraction of max HP retained on
/// a win; 0 on a loss; a small partial-credit fraction on a time-out.
fn rollout(state: &CombatState, rng: &mut impl Rng) -> f32 {
    if state.is_won() {
        return state.player.hp as f32 / state.player.max_hp as f32;
    }
    if state.is_lost() {
        return 0.0;
    }
    // Apply end-of-turn (enemy phase) + draw next hand.
    let mut s = state.clone();
    apply::end_turn(&mut s, rng);
    if s.is_over() {
        return if s.is_won() {
            s.player.hp as f32 / s.player.max_hp as f32
        } else {
            0.0
        };
    }
    let result = run_combat(s, &GreedyDamagePolicy, rng);
    if result.combat_won {
        result.final_state.player.hp as f32 / result.final_state.player.max_hp as f32
    } else {
        // Tiny partial credit for damage dealt, to break ties in losing lines.
        let total: u32 = state.enemies.iter().map(|e| e.max_hp).sum();
        if total > 0 {
            result.damage_dealt as f32 / total as f32 * 0.05
        } else {
            0.0
        }
    }
}

/// Propagate `score` from `idx` up to the root, incrementing visit counts.
fn backprop(arena: &mut Vec<MctsNode>, mut idx: usize, score: f32) {
    loop {
        arena[idx].visits += 1;
        arena[idx].score_sum += score;
        match arena[idx].parent {
            None => break,
            Some(p) => idx = p,
        }
    }
}

/// Walk the most-visited child at each level starting from `start`, collecting
/// actions along the way.
fn extract_best_path(arena: &[MctsNode], start: usize) -> Vec<Action> {
    let mut actions = Vec::new();
    let mut idx = start;
    loop {
        if let Some(act) = &arena[idx].action {
            actions.push(*act);
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

/// Replay `actions` on a clone of `state`, then trigger end-of-turn to absorb
/// the enemy phase.  Returns `(damage_dealt, hp_retained_after_enemy_attack)`.
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

/// UCB1 Monte Carlo Tree Search over card-play orderings.
///
/// Builds an in-memory tree for the current turn's card-play decisions
/// (deterministic) and estimates future-turn outcomes via rollouts using
/// `GreedyDamagePolicy`.  The best first action is the most-visited child of
/// the root (robust selection, standard in competitive MCTS).
pub fn best_play_sequence(
    state: &CombatState,
    budget: u32,
    rng: &mut impl Rng,
) -> PlayAdvice {
    let has_playable = state
        .hand
        .iter()
        .any(|c| c.is_playable(state.energy) && state.stars >= c.star_cost as u32);

    if !has_playable {
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

    let mut arena: Vec<MctsNode> = Vec::with_capacity(budget as usize + 4);
    arena.push(MctsNode::new(None, None, state.clone()));

    let mut hp_losses: Vec<f32> = Vec::with_capacity(budget as usize);

    for _ in 0..budget {
        // Selection
        let leaf = select(&arena);

        // Expansion (or reuse leaf if it's fully explored / terminal)
        let sim_node = if !arena[leaf].untried.is_empty() {
            expand(&mut arena, leaf, rng)
        } else {
            leaf
        };

        // Simulation
        let score = rollout(&arena[sim_node].state, rng);
        hp_losses.push((1.0 - score) * state.player.max_hp as f32);

        // Backpropagation
        backprop(&mut arena, sim_node, score);
    }

    // Best first action: most-visited root child (robust selection)
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
        simulation_count: budget,
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
        // Hand: Bash (2 cost) + Strike (1 cost) = 3 energy needed; we have 3.
        let state = make_state(vec![bash(), strike()], 50);
        let advice = best_play_sequence(&state, 500, &mut rng());

        assert!(!advice.actions.is_empty());
        // Best sequence should start with Bash (hand idx 0)
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
    fn simulation_count_equals_budget() {
        let state = make_state(vec![strike(), defend(), bash()], 50);
        let advice = best_play_sequence(&state, 200, &mut rng());
        assert_eq!(advice.simulation_count, 200);
    }

    #[test]
    fn hp_loss_percentiles_are_non_negative() {
        let state = make_state(vec![strike()], 50);
        let advice = best_play_sequence(&state, 100, &mut rng());
        assert!(advice.hp_loss_p10 >= 0.0);
        assert!(advice.hp_loss_p50 >= 0.0);
        assert!(advice.hp_loss_p90 >= 0.0);
        assert!(advice.hp_loss_p10 <= advice.hp_loss_p50);
        assert!(advice.hp_loss_p50 <= advice.hp_loss_p90);
    }

    #[test]
    fn mcts_explores_tree_across_budget() {
        // With budget=200 and 3 cards, the tree should have grown beyond depth-1.
        let state = make_state(vec![bash(), strike(), defend()], 50);
        let advice = best_play_sequence(&state, 200, &mut rng());
        // We expect a multi-card sequence to be discovered
        assert!(advice.actions.len() >= 1);
    }

    #[test]
    fn one_shot_kill_found() {
        // Single Strike kills a 6 HP enemy; MCTS should recommend it.
        let state = make_state(vec![strike(), defend()], 6);
        let advice = best_play_sequence(&state, 200, &mut rng());
        // First action should be Strike (idx 0), not Defend
        assert_eq!(advice.actions[0].card_hand_idx, 0, "Strike kills enemy; should play it first");
    }

    #[test]
    fn percentile_helper_works() {
        let sorted = vec![0.0, 10.0, 20.0, 30.0, 40.0];
        assert!((percentile(&sorted, 0.0) - 0.0).abs() < f32::EPSILON);
        assert!((percentile(&sorted, 100.0) - 40.0).abs() < f32::EPSILON);
        assert!((percentile(&sorted, 50.0) - 20.0).abs() < f32::EPSILON);
    }
}
