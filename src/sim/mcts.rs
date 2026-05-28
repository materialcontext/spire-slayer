use rand::seq::SliceRandom;
use rand::Rng;

use super::apply;
use super::policy::{select_target, Action};
use crate::domain::combat::CombatState;
use crate::domain::effect::{BuffType, CardEffect};
use crate::metrics::combat as metrics;

const PERMUTATION_LIMIT: usize = 5; // enumerate all orderings up to this hand size

// ── Public types ───────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct PlayAdvice {
    /// Optimal card play sequence for the current turn.
    pub actions: Vec<Action>,
    /// Expected total damage dealt to enemies.
    pub expected_damage: f32,
    /// Expected player HP retained after enemy attacks.
    pub expected_hp_retained: f32,
    /// Human-readable rationale for why this sequence was chosen.
    pub rationale: String,
    /// Number of candidate sequences evaluated.
    pub simulation_count: u32,
    /// HP-loss distribution across stochastic playouts (positive = damage taken).
    pub hp_loss_p10: f32,
    pub hp_loss_p50: f32,
    pub hp_loss_p90: f32,
}

// ── Core algorithm ─────────────────────────────────────────────────────────

/// Flat Monte Carlo search over play orderings.
///
/// For hands with ≤ `PERMUTATION_LIMIT` (5) playable cards, all permutations
/// are evaluated. For larger hands, `budget` random orderings are sampled.
///
/// Sequences are scored by `1.0 × damage + 0.8 × block + 2.0 × hp_delta`.
pub fn best_play_sequence(
    state: &CombatState,
    budget: u32,
    rng: &mut impl Rng,
) -> PlayAdvice {
    // Indices into state.hand for playable cards only
    let playable: Vec<usize> = state
        .hand
        .iter()
        .enumerate()
        .filter(|(_, c)| c.is_playable(state.energy))
        .map(|(i, _)| i)
        .collect();

    if playable.is_empty() {
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

    let candidates: Vec<Vec<usize>> = if playable.len() <= PERMUTATION_LIMIT {
        all_permutations(playable)
    } else {
        sample_permutations(&playable, budget as usize, rng)
    };

    let sim_count = candidates.len() as u32;

    let mut best_score = f32::NEG_INFINITY;
    let mut best_seq: Vec<usize> = candidates[0].clone();
    let mut best_damage = 0f32;
    let mut best_hp = state.player.hp as f32;

    for seq in &candidates {
        let (dmg, blk, hp_delta) = evaluate_sequence(state, seq, rng);
        let score = dmg as f32 * 1.0 + blk as f32 * 0.8 + hp_delta as f32 * 2.0;

        if score > best_score {
            best_score = score;
            best_seq = seq.clone();
            best_damage = dmg as f32;
            best_hp = state.player.hp as f32 + hp_delta as f32;
        }
    }

    let actions = reconstruct_actions(state, &best_seq);
    let rationale = build_rationale(state, &best_seq, best_damage, best_hp);

    PlayAdvice {
        actions,
        expected_damage: best_damage,
        expected_hp_retained: best_hp,
        rationale,
        simulation_count: sim_count,
        hp_loss_p10: 0.0,
        hp_loss_p50: 0.0,
        hp_loss_p90: 0.0,
    }
}

// ── Sequence evaluation ────────────────────────────────────────────────────

/// Simulate playing `play_order` (original hand indices) on a clone of `state`,
/// then run the enemy phase. Returns `(damage_dealt, block_gained, hp_delta)`.
fn evaluate_sequence(
    initial_state: &CombatState,
    play_order: &[usize],
    rng: &mut impl Rng,
) -> (u32, u32, i32) {
    let mut state = initial_state.clone();
    let target = select_target(&state);
    let init_hp = state.player.hp;
    let init_enemy_hp: u32 = state.enemies.iter().map(|e| e.hp).sum();

    for (step, &orig_idx) in play_order.iter().enumerate() {
        if state.is_over() {
            break;
        }
        // Compute current index: each earlier card in the sequence with an
        // original index below ours shifts us one position to the left.
        let shift = play_order[..step]
            .iter()
            .filter(|&&j| j < orig_idx)
            .count();
        let current_idx = orig_idx.saturating_sub(shift);

        if current_idx >= state.hand.len() {
            break;
        }
        if !state.hand[current_idx].is_playable(state.energy) {
            // Out of energy for this card; skip remaining (energy can't recover)
            break;
        }
        let _ = apply::play_card(&mut state, current_idx, target, rng);
    }

    let block_gained = state.player.block;

    if !state.is_over() {
        apply::end_turn(&mut state, rng);
    }

    let final_enemy_hp: u32 = state.enemies.iter().map(|e| e.hp).sum();
    let damage = init_enemy_hp.saturating_sub(final_enemy_hp);
    let hp_delta = state.player.hp as i32 - init_hp as i32;

    (damage, block_gained, hp_delta)
}

/// Convert a sequence of original hand indices into `Action` objects,
/// accounting for the index shift as each card is removed.
fn reconstruct_actions(state: &CombatState, play_order: &[usize]) -> Vec<Action> {
    let target = select_target(state);
    play_order
        .iter()
        .enumerate()
        .map(|(step, &orig)| {
            let shift = play_order[..step].iter().filter(|&&j| j < orig).count();
            let current = orig.saturating_sub(shift);
            Action { card_hand_idx: current, target_idx: target }
        })
        .collect()
}

// ── Rationale generation ───────────────────────────────────────────────────

fn build_rationale(
    state: &CombatState,
    play_order: &[usize],
    expected_damage: f32,
    expected_hp: f32,
) -> String {
    if play_order.is_empty() {
        return "No cards to play.".to_string();
    }

    let first = &state.hand[play_order[0]];

    // Opening with a debuff amplifier?
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

    // Defensive opening under lethal threat?
    if metrics::is_lethal_turn(state) && first.base_block() > 0 {
        return format!(
            "{} first — survival: blocks lethal incoming damage",
            first.name
        );
    }

    format!(
        "Optimal order: expected {:.0} dmg, {:.0} HP retained",
        expected_damage, expected_hp
    )
}

// ── Permutation helpers ────────────────────────────────────────────────────

fn all_permutations(items: Vec<usize>) -> Vec<Vec<usize>> {
    let n = items.len();
    let mut out = Vec::with_capacity(factorial(n));
    let mut items = items;
    heap_permute(&mut items, n, &mut out);
    out
}

fn heap_permute(arr: &mut Vec<usize>, k: usize, out: &mut Vec<Vec<usize>>) {
    if k == 1 {
        out.push(arr.clone());
        return;
    }
    for i in 0..k {
        heap_permute(arr, k - 1, out);
        if k % 2 == 0 {
            arr.swap(i, k - 1);
        } else {
            arr.swap(0, k - 1);
        }
    }
}

fn sample_permutations(items: &[usize], n: usize, rng: &mut impl Rng) -> Vec<Vec<usize>> {
    (0..n)
        .map(|_| {
            let mut perm = items.to_vec();
            perm.shuffle(rng);
            perm
        })
        .collect()
}

fn factorial(n: usize) -> usize {
    (1..=n).product()
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
    fn single_card_hand() {
        let state = make_state(vec![strike()], 50);
        let advice = best_play_sequence(&state, 100, &mut rng());
        assert_eq!(advice.actions.len(), 1);
        assert!(advice.expected_damage > 0.0);
    }

    #[test]
    fn bash_then_strike_outscores_strike_then_bash() {
        // Bash applies Vulnerable (+50% damage), so Bash→Strike should score
        // higher than Strike→Bash because the follow-up Strike hits harder.
        // Hand: Bash (cost 2) + Strike (cost 1) = 3 energy needed; we have 3.
        let state = make_state(vec![bash(), strike()], 50);
        let advice = best_play_sequence(&state, 100, &mut rng());

        assert!(!advice.actions.is_empty());
        // Best sequence should start with Bash (hand idx 0)
        assert_eq!(advice.actions[0].card_hand_idx, 0, "Should lead with Bash");
        assert!(
            advice.expected_damage > 0.0,
            "Expected damage should be positive"
        );
    }

    #[test]
    fn advice_has_non_empty_rationale() {
        let state = make_state(vec![strike(), defend()], 50);
        let advice = best_play_sequence(&state, 50, &mut rng());
        assert!(!advice.rationale.is_empty());
    }

    #[test]
    fn simulation_count_matches_permutations() {
        // 3 playable cards → 3! = 6 permutations
        let state = make_state(vec![strike(), defend(), bash()], 50);
        let advice = best_play_sequence(&state, 1000, &mut rng());
        assert_eq!(advice.simulation_count, 6);
    }

    #[test]
    fn large_hand_uses_budget() {
        // 6 cards → exceeds PERMUTATION_LIMIT, should sample `budget` times
        let hand: Vec<Card> = (0..6).map(|_| strike()).collect();
        let state = make_state(hand, 50);
        let advice = best_play_sequence(&state, 50, &mut rng());
        assert_eq!(advice.simulation_count, 50);
    }

    #[test]
    fn all_permutations_count() {
        assert_eq!(all_permutations(vec![0, 1, 2]).len(), 6);
        assert_eq!(all_permutations(vec![0, 1, 2, 3, 4]).len(), 120);
    }

    #[test]
    fn factorial_values() {
        assert_eq!(factorial(0), 1);
        assert_eq!(factorial(5), 120);
    }
}
