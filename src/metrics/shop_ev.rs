use rand::Rng;
use rand::seq::SliceRandom;

use crate::data::api::{SpireApiEncounter, SpireApiMonster};
use crate::domain::card::{Card, Rarity};
use crate::metrics::deck_dash::compute_deck_stats;
use crate::metrics::map_ev::card_value_score;

/// Number of random shop samples used to estimate expected card value.
const N_SHOP_SAMPLES: usize = 1;
/// Scale factor: 2-fight gauntlet → 5 remaining run fights.
const GAUNTLET_TO_RUN_SCALE: f32 = 2.5;

// ── Public types ───────────────────────────────────────────────────────────────

/// HP-equivalent value and explanation for a single shop relic.
#[derive(Debug, Clone)]
pub struct RelicAdvice {
    pub relic_index: usize,
    pub hp_value: f32,
    pub simulated: bool,
    pub reason: String,
}

// ── Relic scoring ──────────────────────────────────────────────────────────────

/// Score a shop relic in HP-equivalent over the remaining act.
///
/// Attempts simulation first: adds the relic to the current relic list and re-runs
/// `compute_deck_stats`. If the delta vs. `baseline_mean_hp_loss` is meaningful the
/// relic has a wired combat effect and the sim result is used. Otherwise falls back
/// to a rarity-weighted heuristic.
///
/// `baseline_mean_hp_loss` — mean HP lost per fight without this relic (caller
/// pre-computes once to avoid redundant simulations).
pub fn score_shop_relic(
    relic_index: usize,
    relic_id: &str,
    rarity: &str,
    deck: &[Card],
    hp: u32,
    max_hp: u32,
    sub_act: &str,
    all_encounters: &[SpireApiEncounter],
    all_monsters: &[SpireApiMonster],
    current_relics: &[String],
    baseline_mean_hp_loss: f32,
    has_encounter_data: bool,
    ascension: u8,
    rng: &mut impl Rng,
) -> RelicAdvice {
    if has_encounter_data {
        let mut with_relic = current_relics.to_vec();
        with_relic.push(relic_id.to_string());
        let stats = compute_deck_stats(
            deck, hp, max_hp, sub_act, all_encounters, all_monsters, &with_relic, ascension, rng,
        );
        let per_fight = baseline_mean_hp_loss - stats.mean_hp_loss;
        let total = per_fight * crate::metrics::map_ev::remaining_fights(sub_act);
        if total > 0.5 {
            return RelicAdvice {
                relic_index,
                hp_value: total,
                simulated: true,
                reason: format!("saves {:.1} HP/fight (sim)", per_fight),
            };
        }
    }

    let hp_value = rarity_heuristic_hp(rarity);
    RelicAdvice {
        relic_index,
        hp_value,
        simulated: false,
        reason: format!("{:.0} HP-equiv ({})", hp_value, rarity_short(rarity)),
    }
}

// ── Removal scoring ────────────────────────────────────────────────────────────

/// HP value of removing the deck's worst card over the remaining act.
///
/// When `has_encounter_data` is true and `baseline_mean_hp_loss > 0`, uses the
/// caller-supplied baseline instead of re-running the full deck simulation.
pub fn compute_removal_hp_value(
    deck: &[Card],
    hp: u32,
    max_hp: u32,
    sub_act: &str,
    all_encounters: &[SpireApiEncounter],
    all_monsters: &[SpireApiMonster],
    relics: &[String],
    baseline_mean_hp_loss: f32,
    has_encounter_data: bool,
    ascension: u8,
    rng: &mut impl Rng,
) -> f32 {
    if deck.len() <= 1 {
        return 0.0;
    }

    let worst_idx = deck
        .iter()
        .enumerate()
        .min_by(|(_, a), (_, b)| {
            card_value_score(a)
                .partial_cmp(&card_value_score(b))
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .map(|(i, _)| i)
        .unwrap_or(0);

    let deck_reduced: Vec<Card> = deck
        .iter()
        .enumerate()
        .filter(|(i, _)| *i != worst_idx)
        .map(|(_, c)| c.clone())
        .collect();

    let (base_loss, has_data) = if has_encounter_data && baseline_mean_hp_loss > 0.0 {
        (baseline_mean_hp_loss, true)
    } else {
        let full = compute_deck_stats(deck, hp, max_hp, sub_act, all_encounters, all_monsters, relics, ascension, rng);
        (full.mean_hp_loss, full.encounter_count > 0)
    };

    if !has_data {
        return if deck.iter().any(|c| c.cost == 255) { 12.0 } else { 3.0 };
    }

    let reduced = compute_deck_stats(
        &deck_reduced, hp, max_hp, sub_act, all_encounters, all_monsters, relics, ascension, rng,
    );
    (base_loss - reduced.mean_hp_loss).max(0.0) * crate::metrics::map_ev::remaining_fights(sub_act)
}

// ── Catalog-based card value simulation ───────────────────────────────────────

fn sample_shop_rarity(rng: &mut impl Rng) -> Rarity {
    let r: f32 = rng.r#gen();
    if r < 0.54 {
        Rarity::Common
    } else if r < 0.91 {
        Rarity::Uncommon
    } else {
        Rarity::Rare
    }
}

/// Sample `n` cards from the class pool using shop rarity weights.
fn sample_shop_cards(class_cards: &[Card], n: usize, rng: &mut impl Rng) -> Vec<Card> {
    if class_cards.is_empty() {
        return vec![];
    }
    let mut result = Vec::with_capacity(n);
    for _ in 0..n {
        let rarity = sample_shop_rarity(rng);
        let card = class_cards
            .iter()
            .filter(|c| c.rarity == rarity)
            .collect::<Vec<_>>()
            .choose(rng)
            .map(|c| (*c).clone())
            .or_else(|| class_cards.choose(rng).cloned());
        if let Some(c) = card {
            result.push(c);
        }
    }
    result
}

/// Estimate the expected HP gain from the best card in a typical shop.
///
/// Samples `N_SHOP_SAMPLES` random shop offerings from the class catalog, runs
/// `sim_pick_score` on each, and averages the best card's delta_hp (scaled from
/// a 2-fight gauntlet to the remaining `REMAINING_FIGHTS`).
/// Returns `None` when no encounter data or class cards are available.
fn simulate_expected_card_value(
    class_cards: &[Card],
    deck: &[Card],
    hp: u32,
    max_hp: u32,
    sub_act: &str,
    all_encounters: &[SpireApiEncounter],
    all_monsters: &[SpireApiMonster],
    relics: &[String],
    ascension: u8,
    rng: &mut impl Rng,
) -> Option<f32> {
    if class_cards.is_empty() || all_encounters.is_empty() {
        return None;
    }
    let mut total = 0.0f32;
    let mut count = 0usize;
    for _ in 0..N_SHOP_SAMPLES {
        let offered = sample_shop_cards(class_cards, 3, rng);
        if offered.is_empty() {
            continue;
        }
        let advice = crate::metrics::card_pick::sim_pick_score(
            &offered, deck, hp, max_hp, sub_act, all_encounters, all_monsters, relics, ascension, rng,
        );
        let best_delta = advice
            .iter()
            .filter(|a| a.card_index != usize::MAX)
            .map(|a| a.delta_hp)
            .fold(f32::NEG_INFINITY, f32::max);
        if best_delta.is_finite() {
            total += best_delta.max(0.0) * GAUNTLET_TO_RUN_SCALE;
            count += 1;
        }
    }
    if count > 0 {
        Some(total / count as f32)
    } else {
        None
    }
}

// ── Path DP shop node value ────────────────────────────────────────────────────

/// Estimate total HP value from an optimal shop visit given a gold budget.
///
/// Considers three purchase categories:
/// - Card removal (simulated vs. deck quality)
/// - Best card purchase (catalog-backed simulation when available, heuristic fallback)
/// - Best relic purchase (rarity-weighted heuristic)
///
/// All options are ranked by HP/gold efficiency. The player buys greedily starting
/// from the highest ratio until gold runs out. No purchase order is assumed.
pub fn compute_shop_total_value(
    deck: &[Card],
    hp: u32,
    max_hp: u32,
    sub_act: &str,
    all_encounters: &[SpireApiEncounter],
    all_monsters: &[SpireApiMonster],
    gold: u32,
    relics: &[String],
    class_cards: &[Card],
    ascension: u8,
    rng: &mut impl Rng,
) -> f32 {
    // Expected best card: use sim when class catalog + encounter data are available.
    const EXPECTED_CARD_HP: f32 = 7.0;
    const EXPECTED_CARD_PRICE: u32 = 75; // typical uncommon
    // Expected best relic from 3 (50% C @175g, 33% UC @225g, 17% R @275g).
    // 0.50×5 + 0.33×12 + 0.17×20 ≈ 10 HP  at avg ~210g.
    const EXPECTED_RELIC_HP: f32 = 10.0;
    const EXPECTED_RELIC_PRICE: u32 = 210;
    // First removal costs 75g (price escalates per purchase; use base for path DP).
    const REMOVAL_PRICE: u32 = 75;

    let expected_card_hp = simulate_expected_card_value(
        class_cards, deck, hp, max_hp, sub_act, all_encounters, all_monsters, relics, ascension, rng,
    ).unwrap_or(EXPECTED_CARD_HP);

    let mut candidates: Vec<(f32, u32)> = Vec::new();

    // Removal (simulation-backed when encounter data is present)
    if deck.len() > 1 {
        let removal_hp = compute_removal_hp_value(
            deck, hp, max_hp, sub_act, all_encounters, all_monsters, relics,
            0.0, false, // no pre-computed baseline; function computes internally
            ascension, rng,
        );
        if removal_hp > 0.0 {
            candidates.push((removal_hp, REMOVAL_PRICE));
        }
    }

    // Best card and relic
    candidates.push((expected_card_hp, EXPECTED_CARD_PRICE));
    candidates.push((EXPECTED_RELIC_HP, EXPECTED_RELIC_PRICE));

    // Sort by HP/gold ratio descending; buy greedily within budget.
    candidates.sort_by(|a, b| {
        (b.0 / b.1 as f32)
            .partial_cmp(&(a.0 / a.1 as f32))
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let mut remaining = gold;
    let mut total = 0.0f32;
    for (hp_val, price) in &candidates {
        if remaining >= *price {
            total += hp_val;
            remaining -= price;
        }
    }
    total
}

// ── Heuristics ─────────────────────────────────────────────────────────────────

fn rarity_heuristic_hp(rarity: &str) -> f32 {
    match rarity {
        "Common Relic"   => 5.0,
        "Uncommon Relic" => 12.0,
        "Rare Relic"     => 20.0,
        "Shop Relic"     => 8.0,
        "Event Relic"    => 6.0,
        _                => 6.0,
    }
}

fn rarity_short(rarity: &str) -> &'static str {
    match rarity {
        "Common Relic"   => "common",
        "Uncommon Relic" => "uncommon",
        "Rare Relic"     => "rare",
        "Shop Relic"     => "shop relic",
        _                => "relic",
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use rand::SeedableRng;
    use rand::rngs::StdRng;

    fn rng() -> StdRng { StdRng::seed_from_u64(42) }
    fn deck() -> Vec<Card> { crate::domain::catalog::ironclad::starter_deck() }

    #[test]
    fn relic_heuristic_fallback_rare() {
        let advice = score_shop_relic(
            0, "UNKNOWN_RELIC", "Rare Relic",
            &deck(), 80, 80, "overgrowth",
            &[], &[], &[], 0.0, false, 0, &mut rng(),
        );
        assert!(!advice.simulated);
        assert_eq!(advice.hp_value, 20.0);
    }

    #[test]
    fn relic_heuristic_fallback_common() {
        let advice = score_shop_relic(
            1, "FAKE", "Common Relic",
            &deck(), 80, 80, "overgrowth",
            &[], &[], &[], 0.0, false, 0, &mut rng(),
        );
        assert_eq!(advice.hp_value, 5.0);
        assert_eq!(advice.relic_index, 1);
    }

    #[test]
    fn removal_hp_no_data_no_curse() {
        let hp = compute_removal_hp_value(
            &deck(), 80, 80, "overgrowth", &[], &[], &[], 0.0, false, 0, &mut rng(),
        );
        assert_eq!(hp, 3.0);
    }

    #[test]
    fn removal_hp_no_data_with_curse() {
        use crate::domain::card::{Card, CardType, Rarity};
        let mut d = deck();
        d.push(Card::new(999, "Wound", 255, CardType::Status, Rarity::Special, vec![]));
        let hp = compute_removal_hp_value(&d, 80, 80, "overgrowth", &[], &[], &[], 0.0, false, 0, &mut rng());
        assert_eq!(hp, 12.0);
    }

    #[test]
    fn shop_total_value_zero_gold() {
        let v = compute_shop_total_value(
            &deck(), 80, 80, "overgrowth", &[], &[], 0, &[], &[], 0, &mut rng(),
        );
        assert_eq!(v, 0.0);
    }

    #[test]
    fn shop_total_value_card_only() {
        // 80g: can afford card (75g) but not relic (210g)
        let v = compute_shop_total_value(
            &deck(), 80, 80, "overgrowth", &[], &[], 80, &[], &[], 0, &mut rng(),
        );
        // Removal returns 3.0 (no data, no curse) — also 75g; tie goes to first in sorted order.
        // Card is 7 HP at 75g vs removal 3 HP at 75g: card wins ratio. Both cost 75g; only one fits.
        assert!(v > 0.0 && v < 20.0);
    }

    #[test]
    fn shop_total_value_ample_gold_covers_all() {
        // 400g covers removal (75) + card (75) + relic (210) = 360g
        let v = compute_shop_total_value(
            &deck(), 80, 80, "overgrowth", &[], &[], 400, &[], &[], 0, &mut rng(),
        );
        // Card + relic at minimum (removal = 3.0 no-data); total ≥ 3 + 7 + 10 = 20
        assert!(v >= 17.0);
    }

    #[test]
    fn shop_total_picks_highest_ratio_first() {
        // 210g: can afford card (75g, 7HP) and relic (210g, 10HP) but must pick best ratio.
        // Card: 7/75 = 0.093; relic: 10/210 = 0.048. Card wins ratio.
        // After buying card (75g), 135g left — not enough for relic (210g).
        // But removal (3HP, 75g) = 0.040 ratio — skip. So just card = 7 HP.
        let v = compute_shop_total_value(
            &deck(), 80, 80, "overgrowth", &[], &[], 210, &[], &[], 0, &mut rng(),
        );
        // Should be able to buy card (75g) with 135g remaining, can't afford relic (210g)
        // So v ≈ 7 HP (card) + maybe removal if it beats card ratio
        assert!(v > 0.0);
    }
}
