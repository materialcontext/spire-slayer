use rand::Rng;
use rand::seq::SliceRandom;

use crate::data::api::{SpireApiEncounter, SpireApiMonster};
use crate::domain::card::{Card, Rarity};
use crate::domain::encounter::{encounter_to_combat_with_deck, normalize_act};
use crate::domain::run::RunState;
use crate::sim::playout::run_combat;
use crate::sim::policy::GreedyDamagePolicy;
use super::deck::synergy_score;

// ── Public types ───────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct CardAdvice {
    /// Index into the offered slice; `usize::MAX` = skip.
    pub card_index: usize,
    /// Composite score used for ranking (higher is better).
    pub score: f32,
    /// Human-readable one-liner explaining the recommendation.
    pub reason: String,
    // Simulation-backed stats (all 0.0 when using heuristic fallback)
    pub win_rate: f32,
    pub mean_hp_delta: f32,
    pub hp_loss_p50: f32,
    /// Delta vs. skip baseline (positive = better than skipping).
    pub delta_win_rate: f32,
    pub delta_hp: f32,
}

// ── Gauntlet simulation ────────────────────────────────────────────────────

struct GauntletResult {
    win_rate: f32,
    mean_hp_delta: f32,
    hp_loss_p50: f32,
}

/// Run `n` chained 2-encounter playouts, returning aggregate stats.
///
/// Relics are now passed through so relic effects (Burning Blood heals, Nunchaku
/// energy, etc.) are correctly reflected in combat outcomes.
fn eval_gauntlet(
    deck: &[Card],
    hp: u32,
    max_hp: u32,
    enc1: &SpireApiEncounter,
    enc2: &SpireApiEncounter,
    all_monsters: &[SpireApiMonster],
    relics: &[String],
    n: u32,
    rng: &mut impl Rng,
) -> GauntletResult {
    let mut survived = 0u32;
    let mut hp_deltas: Vec<i32> = Vec::with_capacity(n as usize);

    for _ in 0..n {
        let s1 = encounter_to_combat_with_deck(enc1, all_monsters, deck, hp, max_hp, relics, rng);
        let r1 = run_combat(s1, &GreedyDamagePolicy, rng);
        if !r1.player_alive {
            hp_deltas.push(-(hp as i32));
            continue;
        }
        let hp2 = r1.final_state.player.hp;
        let s2 = encounter_to_combat_with_deck(enc2, all_monsters, deck, hp2, max_hp, relics, rng);
        let r2 = run_combat(s2, &GreedyDamagePolicy, rng);
        hp_deltas.push(r1.player_hp_delta + r2.player_hp_delta);
        if r2.player_alive {
            survived += 1;
        }
    }

    let f = n as f32;
    let mut losses: Vec<f32> = hp_deltas.iter().map(|&d| (-d).max(0) as f32).collect();
    losses.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

    GauntletResult {
        win_rate: survived as f32 / f,
        mean_hp_delta: hp_deltas.iter().sum::<i32>() as f32 / f,
        hp_loss_p50: sorted_percentile(&losses, 50.0),
    }
}

fn sorted_percentile(sorted: &[f32], p: f32) -> f32 {
    if sorted.is_empty() {
        return 0.0;
    }
    let idx = ((p / 100.0) * (sorted.len() - 1) as f32).round() as usize;
    sorted[idx.min(sorted.len() - 1)]
}

/// Composite score for ranking gauntlet results.
///
/// Uses the actual `max_hp` for HP normalization rather than a hardcoded 80,
/// so Silent (75 HP) and other classes aren't systematically mis-scored.
fn gauntlet_score(g: &GauntletResult, max_hp: u32) -> f32 {
    g.win_rate * 2.0 + g.mean_hp_delta / max_hp.max(1) as f32
}

/// Average two GauntletResults (for multi-pair sampling).
fn avg_gauntlet(a: GauntletResult, b: GauntletResult) -> GauntletResult {
    GauntletResult {
        win_rate:      (a.win_rate      + b.win_rate)      / 2.0,
        mean_hp_delta: (a.mean_hp_delta + b.mean_hp_delta) / 2.0,
        hp_loss_p50:   (a.hp_loss_p50   + b.hp_loss_p50)   / 2.0,
    }
}

// ── Public API ─────────────────────────────────────────────────────────────

/// Simulator-backed card pick evaluation.
///
/// Samples TWO independent encounter pairs from the current act and averages
/// the gauntlet results across both. This halves the variance from a single
/// unlucky encounter draw while keeping total combat count the same (2 pairs
/// × N_PER_PAIR gauntlets vs 1 pair × N gauntlets).
///
/// Relics are passed through so combat outcomes reflect the player's full kit.
///
/// The returned `Vec` is sorted best-first. The skip option has
/// `card_index == usize::MAX`.
pub fn sim_pick_score(
    offered: &[Card],
    deck: &[Card],
    hp: u32,
    max_hp: u32,
    sub_act: &str,
    all_encounters: &[SpireApiEncounter],
    all_monsters: &[SpireApiMonster],
    relics: &[String],
    rng: &mut impl Rng,
) -> Vec<CardAdvice> {
    let pool: Vec<&SpireApiEncounter> = all_encounters
        .iter()
        .filter(|e| {
            let a = e.act.as_deref().map(normalize_act).unwrap_or("other");
            a == sub_act
                && e.room_type.as_deref().map(|rt| rt != "Boss").unwrap_or(true)
        })
        .collect();

    if pool.len() < 2 || offered.is_empty() {
        return heuristic_advice(offered, deck);
    }

    // Two independent encounter pairs; N_PER_PAIR each → same total combats as N=30 single-pair.
    const N_PER_PAIR: u32 = 15;
    let enc_a1 = *pool.choose(rng).unwrap();
    let enc_a2 = *pool.choose(rng).unwrap();
    let enc_b1 = *pool.choose(rng).unwrap();
    let enc_b2 = *pool.choose(rng).unwrap();

    // Skip baseline — averaged over both encounter pairs.
    let skip = {
        let ga = eval_gauntlet(deck, hp, max_hp, enc_a1, enc_a2, all_monsters, relics, N_PER_PAIR, rng);
        let gb = eval_gauntlet(deck, hp, max_hp, enc_b1, enc_b2, all_monsters, relics, N_PER_PAIR, rng);
        avg_gauntlet(ga, gb)
    };
    let skip_score = gauntlet_score(&skip, max_hp);

    // Evaluate each card option
    let mut advice: Vec<CardAdvice> = offered
        .iter()
        .enumerate()
        .map(|(i, card)| {
            let mut new_deck = deck.to_vec();
            new_deck.push(card.clone());
            let ga = eval_gauntlet(&new_deck, hp, max_hp, enc_a1, enc_a2, all_monsters, relics, N_PER_PAIR, rng);
            let gb = eval_gauntlet(&new_deck, hp, max_hp, enc_b1, enc_b2, all_monsters, relics, N_PER_PAIR, rng);
            let g = avg_gauntlet(ga, gb);
            let score = gauntlet_score(&g, max_hp);
            CardAdvice {
                card_index: i,
                score,
                reason: format!(
                    "{:.0}% surv  Δwin{:+.0}%  Δhp{:+.0}",
                    g.win_rate * 100.0,
                    (g.win_rate - skip.win_rate) * 100.0,
                    g.mean_hp_delta - skip.mean_hp_delta,
                ),
                win_rate: g.win_rate,
                mean_hp_delta: g.mean_hp_delta,
                hp_loss_p50: g.hp_loss_p50,
                delta_win_rate: g.win_rate - skip.win_rate,
                delta_hp: g.mean_hp_delta - skip.mean_hp_delta,
            }
        })
        .collect();

    // Add skip option
    advice.push(CardAdvice {
        card_index: usize::MAX,
        score: skip_score,
        reason: format!(
            "{:.0}% surv  (baseline — skip reward)",
            skip.win_rate * 100.0
        ),
        win_rate: skip.win_rate,
        mean_hp_delta: skip.mean_hp_delta,
        hp_loss_p50: skip.hp_loss_p50,
        delta_win_rate: 0.0,
        delta_hp: 0.0,
    });

    advice.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
    advice
}

// ── Heuristic fallback ─────────────────────────────────────────────────────

fn heuristic_advice(offered: &[Card], deck: &[Card]) -> Vec<CardAdvice> {
    let mut advice: Vec<CardAdvice> = offered
        .iter()
        .enumerate()
        .map(|(i, card)| {
            let score = score_single(card, deck, 1);
            CardAdvice {
                card_index: i,
                score,
                reason: build_heuristic_reason(card, deck),
                win_rate: 0.0,
                mean_hp_delta: 0.0,
                hp_loss_p50: 0.0,
                delta_win_rate: 0.0,
                delta_hp: 0.0,
            }
        })
        .collect();
    advice.push(CardAdvice {
        card_index: usize::MAX,
        score: 0.0,
        reason: "skip reward".to_string(),
        win_rate: 0.0,
        mean_hp_delta: 0.0,
        hp_loss_p50: 0.0,
        delta_win_rate: 0.0,
        delta_hp: 0.0,
    });
    advice.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
    advice
}

/// Score a single card against an existing deck (heuristic).
pub fn score_single(card: &Card, deck: &[Card], _act: u8) -> f32 {
    let rarity_weight: f32 = match card.rarity {
        Rarity::Rare => 0.30,
        Rarity::Uncommon => 0.20,
        Rarity::Common => 0.10,
        Rarity::Basic => 0.05,
        Rarity::Ancient | Rarity::Special => 0.0,
    };

    let base_synergy = synergy_score(deck) as i32;
    let mut extended: Vec<Card> = Vec::with_capacity(deck.len() + 1);
    extended.extend_from_slice(deck);
    extended.push(card.clone());
    let new_synergy = synergy_score(&extended) as i32;
    let synergy_delta = ((new_synergy - base_synergy).max(0) as f32) * 0.10;

    let cost = card.cost.clamp(1, 254) as f32;
    let output = (card.base_damage() + card.base_block()) as f32;
    let efficiency = if output > 0.0 {
        (output / cost / 20.0).min(0.30)
    } else {
        0.10
    };

    let dilution_bonus = (1.0 / (deck.len() as f32 + 1.0)).min(0.15);

    rarity_weight + synergy_delta + efficiency + dilution_bonus
}

/// Rank the offered cards best-first using heuristics only (no simulation).
#[allow(dead_code)]
pub fn pick_score(offered: &[Card], run: &RunState) -> Vec<CardAdvice> {
    heuristic_advice(offered, &run.deck)
}

fn build_heuristic_reason(card: &Card, deck: &[Card]) -> String {
    let dmg = card.base_damage();
    let blk = card.base_block();

    let mut extended = deck.to_vec();
    extended.push(card.clone());
    let delta = synergy_score(&extended) as i32 - synergy_score(deck) as i32;

    if delta > 0 {
        return format!("adds to {} synergy axis", delta);
    }
    if dmg > 0 && card.cost != 255 {
        let per_e = dmg / card.cost.max(1) as u32;
        return format!("{} dmg at {} energy ({}/e)", dmg, card.cost, per_e);
    }
    if blk > 0 {
        return format!("{} block", blk);
    }
    "passive effect".to_string()
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::card::{Card, CardType, Rarity};
    use crate::domain::effect::CardEffect;
    use crate::domain::run::{RunState, PlayerClass, starting_relics};
    use rand::SeedableRng;
    use rand::rngs::StdRng;

    fn strike() -> Card {
        Card::new(1, "Strike", 1, CardType::Attack, Rarity::Basic, vec![CardEffect::Damage(6)])
    }

    fn rare_card() -> Card {
        Card::new(
            99,
            "Demon Form",
            3,
            CardType::Power,
            Rarity::Rare,
            vec![CardEffect::Passive("At the start of each turn, gain 2 Strength.".into())],
        )
    }

    fn empty_run() -> RunState {
        RunState::new(
            PlayerClass::Ironclad,
            80,
            80,
            vec![],
            starting_relics::ironclad(),
        )
    }

    #[test]
    fn rare_scores_higher_than_basic() {
        let run = empty_run();
        let advice = pick_score(&[strike(), rare_card()], &run);
        assert_eq!(advice[0].card_index, 1, "Rare should rank higher");
    }

    #[test]
    fn all_cards_get_a_reason() {
        let run = empty_run();
        let offered = vec![strike(), rare_card()];
        let advice = pick_score(&offered, &run);
        for a in &advice {
            if a.card_index != usize::MAX {
                assert!(!a.reason.is_empty());
            }
        }
    }

    #[test]
    fn results_sorted_descending() {
        let run = empty_run();
        let advice = pick_score(&[strike(), strike(), rare_card()], &run);
        for w in advice.windows(2) {
            assert!(w[0].score >= w[1].score);
        }
    }

    #[test]
    fn score_single_positive() {
        let score = score_single(&strike(), &[], 1);
        assert!(score > 0.0);
    }

    #[test]
    fn sim_pick_fallback_when_no_encounters() {
        let mut rng = StdRng::seed_from_u64(1);
        let deck = crate::domain::catalog::ironclad::starter_deck();
        let advice = sim_pick_score(&[strike(), rare_card()], &deck, 80, 80, "overgrowth", &[], &[], &[], &mut rng);
        assert!(!advice.is_empty());
        // All scores non-negative when heuristic fallback
        for a in &advice {
            assert!(a.score >= 0.0);
        }
    }

    #[test]
    fn sim_pick_includes_skip_option() {
        let mut rng = StdRng::seed_from_u64(2);
        let deck = crate::domain::catalog::ironclad::starter_deck();
        let advice = sim_pick_score(&[strike()], &deck, 80, 80, "overgrowth", &[], &[], &[], &mut rng);
        assert!(advice.iter().any(|a| a.card_index == usize::MAX));
    }

    #[test]
    fn sim_pick_sorted_descending() {
        let mut rng = StdRng::seed_from_u64(3);
        let deck = crate::domain::catalog::ironclad::starter_deck();
        let advice = sim_pick_score(&[strike(), rare_card()], &deck, 80, 80, "overgrowth", &[], &[], &[], &mut rng);
        for w in advice.windows(2) {
            assert!(w[0].score >= w[1].score);
        }
    }
}
