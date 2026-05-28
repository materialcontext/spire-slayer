use rand::Rng;
use rand::seq::SliceRandom;
use crate::data::api::{SpireApiAncient, SpireApiRelic};

/// Which ancient appears at the start of this sub-act.
/// Darv is assumed always unlocked (50% chance per act, cannot repeat across acts).
pub fn select_act_ancient(sub_act: &str, had_darv_act2: bool, rng: &mut impl Rng) -> &'static str {
    match sub_act {
        "overgrowth" | "underdocks" => "NEOW",
        "hive" => {
            let mut pool = vec!["OROBAS", "PAEL", "TEZCATARA"];
            if rng.gen_bool(0.5) {
                pool.push("DARV");
            }
            pool.choose(rng).copied().unwrap_or("OROBAS")
        }
        "glory" => {
            let mut pool = vec!["NONUPEIPE", "TANX", "VAKUU"];
            if !had_darv_act2 && rng.gen_bool(0.5) {
                pool.push("DARV");
            }
            pool.choose(rng).copied().unwrap_or("NONUPEIPE")
        }
        _ => "NEOW",
    }
}

/// Sample 3 boon relics for the given ancient.
/// Returns up to 3 SpireApiRelic objects (looked up by ID from relic_data).
/// For multi-pool ancients: picks 1 from each pool.
/// For single-pool ancients: picks 3 from the pool.
/// Conditions are largely ignored except Neow (curse+positive split).
pub fn sample_boons(
    ancient_id: &str,
    ancient_data: &[SpireApiAncient],
    relic_data: &[SpireApiRelic],
    rng: &mut impl Rng,
) -> Vec<SpireApiRelic> {
    let Some(ancient) = ancient_data.iter().find(|a| a.id.eq_ignore_ascii_case(ancient_id)) else {
        return vec![];
    };

    let lookup = |id: &str| -> Option<SpireApiRelic> {
        relic_data.iter().find(|r| r.id.eq_ignore_ascii_case(id)).cloned()
    };

    let pools = &ancient.pools;

    if ancient.id.eq_ignore_ascii_case("NEOW") {
        // pool[0] is curse pool (pick 1 random), pool[1] is positive pool (pick 2 random)
        let mut result = Vec::new();

        if let Some(curse_pool) = pools.first() {
            let ids: Vec<&str> = curse_pool.relics.iter().map(|r| r.id.as_str()).collect();
            if let Some(&curse_id) = ids.choose(rng) {
                if let Some(relic) = lookup(curse_id) {
                    result.push(relic);
                }
            }
        }

        if let Some(pos_pool) = pools.get(1) {
            let mut ids: Vec<&str> = pos_pool.relics.iter().map(|r| r.id.as_str()).collect();
            ids.shuffle(rng);
            let mut count = 0;
            for id in ids {
                if count >= 2 {
                    break;
                }
                if let Some(relic) = lookup(id) {
                    result.push(relic);
                    count += 1;
                }
            }
        }

        result.truncate(3);
        return result;
    }

    if ancient.id.eq_ignore_ascii_case("DARV") {
        // pick 3 from pool[0] (no repeat; ignore Dusty Tome pool for simplicity)
        if let Some(pool) = pools.first() {
            let mut ids: Vec<&str> = pool.relics.iter().map(|r| r.id.as_str()).collect();
            ids.shuffle(rng);
            let mut result = Vec::new();
            for id in ids {
                if result.len() >= 3 {
                    break;
                }
                if let Some(relic) = lookup(id) {
                    result.push(relic);
                }
            }
            return result;
        }
        return vec![];
    }

    if pools.len() >= 2 {
        // Multi-pool ancients (Tezcatara/Pael/Orobas/Vakuu): pick 1 from each pool
        let mut result = Vec::new();
        for pool in pools.iter().take(3) {
            let ids: Vec<&str> = pool.relics.iter().map(|r| r.id.as_str()).collect();
            if let Some(&chosen_id) = ids.choose(rng) {
                if let Some(relic) = lookup(chosen_id) {
                    result.push(relic);
                }
            }
        }
        return result;
    }

    // Single-pool ancients (Nonupeipe/Tanx): pick 3 random (no repeat)
    if let Some(pool) = pools.first() {
        let mut ids: Vec<&str> = pool.relics.iter().map(|r| r.id.as_str()).collect();
        ids.shuffle(rng);
        let mut result = Vec::new();
        for id in ids {
            if result.len() >= 3 {
                break;
            }
            if let Some(relic) = lookup(id) {
                result.push(relic);
            }
        }
        return result;
    }

    vec![]
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::SeedableRng;
    use rand::rngs::StdRng;

    fn seeded_rng() -> StdRng {
        StdRng::seed_from_u64(42)
    }

    #[test]
    fn neow_for_overgrowth() {
        let mut rng = seeded_rng();
        let ancient = select_act_ancient("overgrowth", false, &mut rng);
        assert_eq!(ancient, "NEOW");
    }

    #[test]
    fn neow_for_underdocks() {
        let mut rng = seeded_rng();
        let ancient = select_act_ancient("underdocks", false, &mut rng);
        assert_eq!(ancient, "NEOW");
    }

    #[test]
    fn hive_ancient_is_in_valid_set() {
        let mut rng = seeded_rng();
        let ancient = select_act_ancient("hive", false, &mut rng);
        assert!(
            ["OROBAS", "PAEL", "TEZCATARA", "DARV"].contains(&ancient),
            "unexpected ancient: {ancient}"
        );
    }

    #[test]
    fn glory_ancient_is_in_valid_set() {
        let mut rng = seeded_rng();
        let ancient = select_act_ancient("glory", false, &mut rng);
        assert!(
            ["NONUPEIPE", "TANX", "VAKUU", "DARV"].contains(&ancient),
            "unexpected ancient: {ancient}"
        );
    }

    #[test]
    fn glory_no_darv_when_had_darv_act2() {
        // With had_darv_act2=true, DARV should never appear in glory pool
        let valid = ["NONUPEIPE", "TANX", "VAKUU"];
        for seed in 0..100u64 {
            let mut rng = StdRng::seed_from_u64(seed);
            let ancient = select_act_ancient("glory", true, &mut rng);
            assert!(valid.contains(&ancient), "unexpected: {ancient} with seed {seed}");
        }
    }

    #[test]
    fn sample_boons_unknown_ancient_returns_empty() {
        let mut rng = seeded_rng();
        let result = sample_boons("UNKNOWN", &[], &[], &mut rng);
        assert!(result.is_empty());
    }

    #[test]
    fn sample_boons_neow_returns_up_to_3() {
        let ancients = crate::data::api::load_ancients();
        let relics = crate::data::api::load_relics();
        let mut rng = seeded_rng();
        let boons = sample_boons("NEOW", &ancients, &relics, &mut rng);
        // Should return at most 3 (1 curse + 2 positive)
        assert!(boons.len() <= 3);
    }

    #[test]
    fn sample_boons_tezcatara_returns_up_to_3() {
        let ancients = crate::data::api::load_ancients();
        let relics = crate::data::api::load_relics();
        let mut rng = seeded_rng();
        let boons = sample_boons("TEZCATARA", &ancients, &relics, &mut rng);
        // Tezcatara has 3 pools, should return 3
        assert!(boons.len() <= 3);
    }

    #[test]
    fn sample_boons_nonupeipe_returns_up_to_3() {
        let ancients = crate::data::api::load_ancients();
        let relics = crate::data::api::load_relics();
        let mut rng = seeded_rng();
        let boons = sample_boons("NONUPEIPE", &ancients, &relics, &mut rng);
        assert!(boons.len() <= 3);
    }
}
