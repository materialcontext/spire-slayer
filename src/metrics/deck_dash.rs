use rand::Rng;
use rand::seq::SliceRandom;

use crate::data::api::{SpireApiEncounter, SpireApiMonster};
use crate::domain::card::Card;
use crate::domain::encounter::{encounter_to_combat_with_deck, normalize_act};
use crate::sim::playout::run_combat;
use crate::sim::policy::GreedyDamagePolicy;
use super::deck::{attack_ratio, deck_score, has_block_density, mean_cost, synergy_score};

// ── Public types ───────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct DeckStats {
    // Deck intrinsics (no simulation needed)
    pub deck_size: usize,
    pub cycle_turns: f32,
    pub mean_energy_cost: f32,
    pub attack_fraction: f32,
    pub block_card_count: usize,
    pub synergy_axes: u32,
    pub heuristic_score: f32,
    // Single-turn output distributions vs act panel
    pub dpt_p10: f32,
    pub dpt_p50: f32,
    pub dpt_p90: f32,
    pub mean_dpt: f32,
    pub bpt_p10: f32,
    pub bpt_p50: f32,
    pub bpt_p90: f32,
    pub mean_bpt: f32,
    // Combat outcomes
    pub kill_rate: f32,
    pub survival_rate: f32,
    pub mean_hp_loss: f32,
    // Context
    pub sub_act: String,
    pub encounter_count: usize,
    pub playout_count: u32,
}

// ── Core computation ───────────────────────────────────────────────────────

/// Compute deck statistics by running simulations against a sampled act panel.
///
/// Samples up to 5 normal/elite encounters from `act`, runs 8 full combats each,
/// and aggregates damage-per-turn, block-per-turn, and survival distributions.
pub fn compute_deck_stats(
    deck: &[Card],
    hp: u32,
    max_hp: u32,
    sub_act: &str,
    all_encounters: &[SpireApiEncounter],
    all_monsters: &[SpireApiMonster],
    relics: &[String],
    rng: &mut impl Rng,
) -> DeckStats {
    const MAX_ENCOUNTERS: usize = 5;
    const FULL_COMBATS_PER_ENC: u32 = 8;

    let deck_size = deck.len();
    let cycle_turns = if deck_size == 0 { 0.0 } else { deck_size as f32 / 5.0 };
    let mean_energy_cost = mean_cost(deck);
    let attack_fraction = attack_ratio(deck);
    let block_card_count = deck.iter().filter(|c| c.base_block() > 0).count();
    let synergy_axes = synergy_score(deck);
    let heuristic_score = deck_score(deck, 1);

    let pool: Vec<&SpireApiEncounter> = all_encounters
        .iter()
        .filter(|e| {
            let a = e.act.as_deref().map(normalize_act).unwrap_or("other");
            a == sub_act
                && e.room_type.as_deref().map(|rt| rt != "Boss").unwrap_or(true)
        })
        .collect();

    if pool.is_empty() || deck.is_empty() {
        return DeckStats {
            deck_size,
            cycle_turns,
            mean_energy_cost,
            attack_fraction,
            block_card_count,
            synergy_axes,
            heuristic_score,
            dpt_p10: 0.0,
            dpt_p50: 0.0,
            dpt_p90: 0.0,
            mean_dpt: 0.0,
            bpt_p10: 0.0,
            bpt_p50: 0.0,
            bpt_p90: 0.0,
            mean_bpt: 0.0,
            kill_rate: 0.0,
            survival_rate: 0.0,
            mean_hp_loss: 0.0,
            sub_act: sub_act.to_string(),
            encounter_count: 0,
            playout_count: 0,
        };
    }

    let sampled: Vec<&SpireApiEncounter> =
        pool.choose_multiple(rng, MAX_ENCOUNTERS.min(pool.len())).copied().collect();

    let total_playouts = sampled.len() as u32 * FULL_COMBATS_PER_ENC;
    let mut damages: Vec<f32> = Vec::with_capacity(total_playouts as usize);
    let mut blocks: Vec<f32> = Vec::with_capacity(total_playouts as usize);
    let mut hp_losses: Vec<f32> = Vec::with_capacity(total_playouts as usize);
    let mut kills = 0u32;
    let mut survivals = 0u32;

    for enc in &sampled {
        for _ in 0..FULL_COMBATS_PER_ENC {
            let state = encounter_to_combat_with_deck(enc, all_monsters, deck, hp, max_hp, relics, rng);
            let r = run_combat(state, &GreedyDamagePolicy, rng);
            damages.push(r.damage_dealt as f32 / r.turns.max(1) as f32);
            blocks.push(r.block_absorbed as f32 / r.turns.max(1) as f32);
            hp_losses.push((-r.player_hp_delta).max(0) as f32);
            if r.combat_won { kills += 1; }
            if r.player_alive { survivals += 1; }
        }
    }

    let f = total_playouts as f32;
    damages.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    blocks.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

    DeckStats {
        deck_size,
        cycle_turns,
        mean_energy_cost,
        attack_fraction,
        block_card_count,
        synergy_axes,
        heuristic_score,
        dpt_p10: sorted_pct(&damages, 10.0),
        dpt_p50: sorted_pct(&damages, 50.0),
        dpt_p90: sorted_pct(&damages, 90.0),
        mean_dpt: damages.iter().sum::<f32>() / f,
        bpt_p10: sorted_pct(&blocks, 10.0),
        bpt_p50: sorted_pct(&blocks, 50.0),
        bpt_p90: sorted_pct(&blocks, 90.0),
        mean_bpt: blocks.iter().sum::<f32>() / f,
        kill_rate: kills as f32 / f,
        survival_rate: survivals as f32 / f,
        mean_hp_loss: hp_losses.iter().sum::<f32>() / f,
        sub_act: sub_act.to_string(),
        encounter_count: sampled.len(),
        playout_count: total_playouts,
    }
}

/// Render a compact distribution bar: `p10 ──███████──── p90`.
///
/// Width is fixed at 20 characters. Returns an owned string.
pub fn dist_bar(p10: f32, p50: f32, p90: f32) -> String {
    if p90 <= 0.0 {
        return "─".repeat(20);
    }
    let scale = 20.0 / p90;
    let lo = (p10 * scale).round() as usize;
    let mid = (p50 * scale).round() as usize;
    let hi = 20usize;
    let mut bar = vec!['─'; 20];
    for i in lo..hi {
        bar[i] = '░';
    }
    for i in lo..mid.min(hi) {
        bar[i] = '█';
    }
    bar.into_iter().collect()
}

fn sorted_pct(sorted: &[f32], p: f32) -> f32 {
    if sorted.is_empty() {
        return 0.0;
    }
    let idx = ((p / 100.0) * (sorted.len() - 1) as f32).round() as usize;
    sorted[idx.min(sorted.len() - 1)]
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::catalog::ironclad;
    use rand::SeedableRng;
    use rand::rngs::StdRng;

    #[test]
    fn intrinsics_no_encounters() {
        let mut rng = StdRng::seed_from_u64(7);
        let deck = ironclad::starter_deck();
        let stats = compute_deck_stats(&deck, 80, 80, "overgrowth", &[], &[], &[], &mut rng);
        assert_eq!(stats.deck_size, 10);
        assert!((stats.cycle_turns - 2.0).abs() < 0.01);
        assert_eq!(stats.encounter_count, 0);
        assert_eq!(stats.playout_count, 0);
    }

    #[test]
    fn dist_bar_smoke() {
        let bar = dist_bar(4.0, 9.0, 16.0);
        assert_eq!(bar.chars().count(), 20);
    }

    #[test]
    fn dist_bar_all_zero() {
        let bar = dist_bar(0.0, 0.0, 0.0);
        assert_eq!(bar.chars().count(), 20);
    }

    #[test]
    fn block_card_count_correct() {
        let mut rng = StdRng::seed_from_u64(8);
        let deck = ironclad::starter_deck(); // 4× Defend
        let stats = compute_deck_stats(&deck, 80, 80, "overgrowth", &[], &[], &[], &mut rng);
        assert_eq!(stats.block_card_count, 4);
    }
}
