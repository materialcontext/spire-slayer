use rand::Rng;

use crate::data::api::{SpireApiEncounter, SpireApiMonster, SpireApiEvent};
use crate::domain::card::Card;
use crate::domain::encounter::normalize_act;
use crate::domain::map::ActMap;
use crate::metrics::deck_dash::compute_deck_stats;
use crate::metrics::map_ev::{post_act_heal_hp, POST_ACT_HEAL_ASCENSION_THRESHOLD};
use crate::metrics::path_ev::{compute_path_choices, NodeCosts};

// ── Act structure ──────────────────────────────────────────────────────────
//
// STS2 has 3 Acts:
//   Act 1 — randomly either "overgrowth" or "underdocks" (alternate acts)
//   Act 2 — always "hive"
//   Act 3 — always "glory"
//
// Each act starts with an Ancient (Neow for Act 1) offering 3 boons, and ends
// with a Boss fight.  After the boss the Ancient heals the player before the
// next act begins.

/// Number of randomly generated maps sampled when estimating a future act's cost.
const N_SAMPLE_MAPS: u32 = 5;

/// Returns the act number (1, 2, or 3) for a given sub-act name.
pub fn act_number(sub_act: &str) -> u8 {
    match sub_act {
        "overgrowth" | "underdocks" => 1,
        "hive"                      => 2,
        "glory"                     => 3,
        _                           => 0,
    }
}

/// Returns the (sub_act, act_number) pairs for all acts that follow the current one.
///
/// Act 1 (either overgrowth or underdocks) always leads to Act 2 (hive) then Act 3 (glory).
pub fn acts_after(current_sub_act: &str) -> Vec<(&'static str, u8)> {
    match current_sub_act {
        "overgrowth" | "underdocks" => vec![("hive", 2), ("glory", 3)],
        "hive"                      => vec![("glory", 3)],
        _                           => vec![],
    }
}

// ── Public types ────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct FutureActEv {
    pub sub_act: String,
    pub act_number: u8,
    /// Expected net HP delta navigating this act optimally, including the boss.
    pub expected_delta: f32,
    /// HP at the start of this act (after the prior post-act heal).
    pub entry_hp: f32,
    /// HP immediately after the act boss (before any post-act heal).
    pub exit_hp: f32,
    /// HP at the start of the next act after the post-act heal.
    /// Equal to `exit_hp` for the final act (no heal after the last boss).
    pub post_heal_hp: f32,
}

#[derive(Debug, Clone)]
pub struct RunEv {
    pub current_sub_act: String,
    pub current_act_number: u8,
    /// HP delta from the current map position through the current act's boss.
    pub current_act_remaining_delta: f32,
    /// Projected HP immediately after the current act's boss fight.
    pub hp_after_current_boss: f32,
    /// HP at the start of the next act (after the post-act heal).
    /// Equal to `hp_after_current_boss` when this is the final act.
    pub hp_after_current_heal: f32,
    /// Projections for each remaining future act, in order.
    pub future_acts: Vec<FutureActEv>,
    /// Projected HP at the very end of the run (after the final boss, no heal).
    pub projected_final_hp: f32,
    /// True when encounter simulation backed the costs; false = defaults used.
    pub simulated: bool,
}

// ── Internal helpers ────────────────────────────────────────────────────────

/// Build NodeCosts for a given sub-act by simulating against its encounter pool.
/// Returns `(costs, simulated)` — simulated is false when no data was available.
fn act_node_costs(
    sub_act: &str,
    deck: &[Card],
    hp: u32,
    max_hp: u32,
    ascension: u8,
    all_encounters: &[SpireApiEncounter],
    all_monsters: &[SpireApiMonster],
    all_events: &[SpireApiEvent],
    gold: u32,
    relics: &[String],
    rng: &mut impl Rng,
) -> (NodeCosts, bool) {
    let filter = |room: &str| -> Vec<SpireApiEncounter> {
        all_encounters.iter()
            .filter(|e| {
                let act = e.act.as_deref().map(normalize_act).unwrap_or("other");
                act == sub_act && e.room_type.as_deref() == Some(room)
            })
            .cloned()
            .collect()
    };

    let normal_enc = filter("Monster");
    let elite_enc  = filter("Elite");
    let boss_enc   = filter("Boss");
    let has_data   = !normal_enc.is_empty() || !boss_enc.is_empty();

    let ns = compute_deck_stats(deck, hp, max_hp, sub_act, &normal_enc, all_monsters, relics, rng);
    let es = compute_deck_stats(deck, hp, max_hp, sub_act, &elite_enc,  all_monsters, relics, rng);
    let bs = compute_deck_stats(deck, hp, max_hp, sub_act, &boss_enc,   all_monsters, relics, rng);

    let mut costs = NodeCosts::defaults(max_hp, ascension);
    if has_data {
        costs.monster_loss = ns.mean_hp_loss;
        costs.elite_loss   = es.mean_hp_loss;
        costs.boss_loss    = bs.mean_hp_loss;
    }

    let event_hp = crate::metrics::map_ev::compute_event_hp_delta(
        all_events, sub_act, hp as f32 / max_hp as f32
    );
    let shop_hp = crate::metrics::map_ev::compute_shop_hp_value(
        deck, hp, max_hp, sub_act, all_encounters, all_monsters, gold, relics, rng
    );
    costs.event_hp_delta = event_hp;
    costs.treasure_hp    = 6.0;
    costs.shop_hp_value  = shop_hp;

    (costs, has_data)
}

/// Estimate the average best-path HP delta for a randomly generated act map.
/// Boss cost is already baked into the DP terminal via `costs.boss_loss`.
fn estimate_act_delta(costs: &NodeCosts, ascension: u8, rng: &mut impl Rng) -> f32 {
    let mut total = 0.0f32;
    let mut count = 0u32;

    for _ in 0..N_SAMPLE_MAPS {
        let seed: u64 = rng.r#gen();
        let map = ActMap::generate(seed, ascension);
        let entries = map.entry_nodes();
        if entries.is_empty() {
            continue;
        }
        let choices = compute_path_choices(&map, &entries, 0, costs, true);
        if let Some(best) = choices.iter().map(|c| c.total_hp_delta).reduce(f32::max) {
            total += best;
            count += 1;
        }
    }

    if count > 0 {
        total / count as f32
    } else {
        -(costs.monster_loss * 8.0 + costs.boss_loss)
    }
}

// ── Public API ──────────────────────────────────────────────────────────────

/// Compute a full run projection from the current position.
///
/// `current_act_remaining_delta` is the best-path HP delta from the current
/// map position through the end of the current act (boss included), taken
/// from `app.path_choices`.
pub fn compute_run_ev(
    deck: &[Card],
    current_hp: u32,
    max_hp: u32,
    current_sub_act: &str,
    current_act_remaining_delta: f32,
    all_encounters: &[SpireApiEncounter],
    all_monsters: &[SpireApiMonster],
    ascension: u8,
    current_act_simulated: bool,
    all_events: &[SpireApiEvent],
    gold: u32,
    relics: &[String],
    rng: &mut impl Rng,
) -> RunEv {
    let current_act_number = act_number(current_sub_act);
    let hp_after_current_boss = current_hp as f32 + current_act_remaining_delta;

    // Player dies in current act — no further projection.
    if hp_after_current_boss <= 0.0 {
        return RunEv {
            current_sub_act: current_sub_act.to_string(),
            current_act_number,
            current_act_remaining_delta,
            hp_after_current_boss: 0.0,
            hp_after_current_heal: 0.0,
            future_acts: vec![],
            projected_final_hp: 0.0,
            simulated: current_act_simulated,
        };
    }

    let heal_target = post_act_heal_hp(max_hp, ascension);

    // Acts that follow the current one, in run order.
    let future = acts_after(current_sub_act);
    let is_final_act = future.is_empty();

    // If this is the last act, there's no post-act heal after the boss.
    let hp_after_current_heal = if is_final_act { hp_after_current_boss } else { heal_target };

    let mut entry_hp = hp_after_current_heal;
    let mut future_acts: Vec<FutureActEv> = Vec::new();
    let mut all_simulated = current_act_simulated;

    for (i, &(sub_act, act_num)) in future.iter().enumerate() {
        let is_last = i == future.len() - 1;

        let (costs, has_data) = act_node_costs(
            sub_act, deck, entry_hp as u32, max_hp, ascension, all_encounters, all_monsters,
            all_events, gold, relics,
            rng,
        );
        if !has_data {
            all_simulated = false;
        }

        let expected_delta = estimate_act_delta(&costs, ascension, rng);
        let exit_hp = (entry_hp + expected_delta).max(0.0);
        // No post-act heal after the final boss.
        let post_heal = if is_last { exit_hp } else { heal_target };

        future_acts.push(FutureActEv {
            sub_act: sub_act.to_string(),
            act_number: act_num,
            expected_delta,
            entry_hp,
            exit_hp,
            post_heal_hp: post_heal,
        });

        if exit_hp <= 0.0 {
            break; // projected death — stop here
        }
        entry_hp = post_heal;
    }

    let projected_final_hp = future_acts.last()
        .map(|a| a.exit_hp)
        .unwrap_or(hp_after_current_boss);

    RunEv {
        current_sub_act: current_sub_act.to_string(),
        current_act_number,
        current_act_remaining_delta,
        hp_after_current_boss,
        hp_after_current_heal,
        future_acts,
        projected_final_hp,
        simulated: all_simulated,
    }
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use rand::SeedableRng;
    use rand::rngs::StdRng;

    fn rng() -> StdRng {
        StdRng::seed_from_u64(99)
    }

    fn deck() -> Vec<Card> {
        crate::domain::catalog::ironclad::starter_deck()
    }

    #[test]
    fn post_act_heal_ascension_thresholds() {
        assert_eq!(post_act_heal_hp(80, 0), 80.0);
        assert_eq!(post_act_heal_hp(80, 5), 80.0);
        assert_eq!(post_act_heal_hp(80, POST_ACT_HEAL_ASCENSION_THRESHOLD), 64.0);
        assert_eq!(post_act_heal_hp(80, 10), 64.0);
    }

    #[test]
    fn act_number_correct() {
        assert_eq!(act_number("overgrowth"), 1);
        assert_eq!(act_number("underdocks"), 1);
        assert_eq!(act_number("hive"), 2);
        assert_eq!(act_number("glory"), 3);
    }

    #[test]
    fn acts_after_act1_is_hive_and_glory() {
        // Both Act 1 variants lead to the same future acts.
        let og = acts_after("overgrowth");
        let ud = acts_after("underdocks");
        assert_eq!(og, ud);
        assert_eq!(og.len(), 2);
        assert_eq!(og[0], ("hive", 2));
        assert_eq!(og[1], ("glory", 3));
    }

    #[test]
    fn acts_after_hive_is_glory_only() {
        let h = acts_after("hive");
        assert_eq!(h.len(), 1);
        assert_eq!(h[0], ("glory", 3));
    }

    #[test]
    fn acts_after_glory_is_empty() {
        assert!(acts_after("glory").is_empty());
    }

    #[test]
    fn run_ev_from_overgrowth_has_two_future_acts() {
        let result = compute_run_ev(
            &deck(), 60, 80, "overgrowth", -20.0,
            &[], &[], 0, false, &[], 0, &[], &mut rng(),
        );
        // overgrowth (Act 1) → hive (Act 2) → glory (Act 3)
        assert_eq!(result.current_act_number, 1);
        assert_eq!(result.future_acts.len(), 2);
        assert_eq!(result.future_acts[0].sub_act, "hive");
        assert_eq!(result.future_acts[0].act_number, 2);
        assert_eq!(result.future_acts[1].sub_act, "glory");
        assert_eq!(result.future_acts[1].act_number, 3);
    }

    #[test]
    fn run_ev_from_underdocks_same_as_overgrowth_structure() {
        let og = compute_run_ev(&deck(), 60, 80, "overgrowth", -20.0, &[], &[], 0, false, &[], 0, &[], &mut rng());
        let ud = compute_run_ev(&deck(), 60, 80, "underdocks", -20.0, &[], &[], 0, false, &[], 0, &[], &mut rng());
        // Both are Act 1 — same future structure
        assert_eq!(og.future_acts.len(), ud.future_acts.len());
        assert_eq!(og.future_acts[0].sub_act, ud.future_acts[0].sub_act);
    }

    #[test]
    fn run_ev_from_hive_has_one_future_act() {
        let result = compute_run_ev(
            &deck(), 60, 80, "hive", -20.0,
            &[], &[], 0, false, &[], 0, &[], &mut rng(),
        );
        assert_eq!(result.current_act_number, 2);
        assert_eq!(result.future_acts.len(), 1);
        assert_eq!(result.future_acts[0].sub_act, "glory");
    }

    #[test]
    fn run_ev_from_glory_has_no_future_acts() {
        let result = compute_run_ev(
            &deck(), 60, 80, "glory", -20.0,
            &[], &[], 0, false, &[], 0, &[], &mut rng(),
        );
        assert_eq!(result.current_act_number, 3);
        assert!(result.future_acts.is_empty());
        assert_eq!(result.projected_final_hp, result.hp_after_current_boss);
    }

    #[test]
    fn glory_has_no_post_act_heal() {
        let result = compute_run_ev(
            &deck(), 80, 80, "glory", 0.0,
            &[], &[], 0, false, &[], 0, &[], &mut rng(),
        );
        // Final act: no post-act heal, hp_after_current_heal == hp_after_current_boss
        assert_eq!(result.hp_after_current_heal, result.hp_after_current_boss);
    }

    #[test]
    fn last_future_act_has_no_post_heal() {
        let result = compute_run_ev(
            &deck(), 80, 80, "overgrowth", 0.0,
            &[], &[], 0, false, &[], 0, &[], &mut rng(),
        );
        let last = result.future_acts.last().unwrap();
        // Glory (last act) has no post-act heal.
        assert_eq!(last.post_heal_hp, last.exit_hp);
    }

    #[test]
    fn projected_death_short_circuits() {
        let result = compute_run_ev(
            &deck(), 10, 80, "overgrowth", -50.0,
            &[], &[], 0, false, &[], 0, &[], &mut rng(),
        );
        assert_eq!(result.hp_after_current_boss, 0.0);
        assert!(result.future_acts.is_empty());
    }

    #[test]
    fn estimate_act_delta_is_finite() {
        let costs = NodeCosts::defaults(80, 0);
        let delta = estimate_act_delta(&costs, 0, &mut rng());
        assert!(delta.is_finite());
    }
}
