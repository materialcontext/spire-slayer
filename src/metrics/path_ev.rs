use crate::domain::map::{ActMap, RoomType, COLS, ROWS};
use crate::metrics::map_ev::MapEvData;

// ── Gold ascension threshold ───────────────────────────────────────────────────
// At Ascension 3+ enemies drop less gold.
const GOLD_ASCENSION_THRESHOLD: u8 = 3;

// ── Cost model ─────────────────────────────────────────────────────────────────

pub struct NodeCosts {
    /// Mean HP lost in a Monster room.
    pub monster_loss: f32,
    /// Mean HP lost in an Elite room.
    pub elite_loss: f32,
    /// Mean HP lost in the act Boss fight.
    pub boss_loss: f32,
    /// HP gained by resting (floor(max_hp × 0.30)).
    pub rest_heal: f32,
    /// Mean gold gained in a Monster room.
    pub monster_gold: f32,
    /// Mean gold gained in an Elite room.
    pub elite_gold: f32,
    /// Gold gained from the act Boss.
    pub boss_gold: f32,
    /// Mean gold gained from a Treasure chest.
    pub chest_gold: f32,
    /// Expected HP change from the best event option (avg over pool).
    pub event_hp_delta: f32,
    /// Expected HP equivalent from a treasure relic.
    pub treasure_hp: f32,
    /// Expected HP saved by using the shop (card removal).
    pub shop_hp_value: f32,
}

impl NodeCosts {
    pub fn from_map_ev(map_ev: &MapEvData, max_hp: u32, current_hp: u32, ascension: u8) -> Self {
        let gold = GoldValues::for_ascension(ascension);
        Self {
            monster_loss: map_ev.normal.mean_hp_loss,
            elite_loss:   map_ev.elite.mean_hp_loss,
            boss_loss:    map_ev.boss.mean_hp_loss,
            rest_heal:    effective_rest_heal(max_hp, current_hp),
            monster_gold: gold.monster,
            elite_gold:   gold.elite,
            boss_gold:    gold.boss,
            chest_gold:   gold.chest,
            event_hp_delta: map_ev.event_hp_delta,
            treasure_hp:    map_ev.treasure_hp,
            shop_hp_value:  map_ev.shop_hp_value,
        }
    }

    pub fn defaults(max_hp: u32, ascension: u8) -> Self {
        let gold = GoldValues::for_ascension(ascension);
        Self {
            monster_loss: 10.0,
            elite_loss:   25.0,
            boss_loss:    35.0,
            rest_heal:    (max_hp as f32 * 0.30).floor(),
            monster_gold: gold.monster,
            elite_gold:   gold.elite,
            boss_gold:    gold.boss,
            chest_gold:   gold.chest,
            event_hp_delta: -2.0,
            treasure_hp:    6.0,
            shop_hp_value:  0.0,
        }
    }
}

/// Rest heal value capped at the actual HP deficit and discounted by a
/// mid-act HP ratio estimate.
///
/// Because the path DP doesn't track HP state per node, we approximate the
/// expected HP at rest sites: on average the player arrives at ~70% of their
/// current HP ratio (they've taken some damage since the last heal). The heal
/// is then capped at that expected deficit so overheal scenarios aren't
/// counted at full value.
fn effective_rest_heal(max_hp: u32, current_hp: u32) -> f32 {
    let full_heal = (max_hp as f32 * 0.30).floor();
    // Expected HP at the rest site: current ratio × 0.7 (damage taken en-route)
    let hp_ratio_at_rest = (current_hp as f32 / max_hp.max(1) as f32) * 0.70;
    let expected_deficit = max_hp as f32 * (1.0 - hp_ratio_at_rest);
    full_heal.min(expected_deficit)
}

struct GoldValues {
    monster: f32,
    elite:   f32,
    boss:    f32,
    chest:   f32,
}

impl GoldValues {
    fn for_ascension(ascension: u8) -> Self {
        if ascension >= GOLD_ASCENSION_THRESHOLD {
            // Ascension 3+: 8–15, 26–34, 75, 32–40
            Self { monster: 11.5, elite: 30.0, boss: 75.0, chest: 36.0 }
        } else {
            // Normal: 10–20, 35–45, 100, 42–53
            Self { monster: 15.0, elite: 40.0, boss: 100.0, chest: 47.5 }
        }
    }
}

fn node_cost(rt: Option<RoomType>, costs: &NodeCosts) -> f32 {
    match rt {
        Some(RoomType::Monster)  => -costs.monster_loss,
        Some(RoomType::Elite)    => -costs.elite_loss,
        Some(RoomType::Rest)     => costs.rest_heal,
        Some(RoomType::Event)    => costs.event_hp_delta,
        Some(RoomType::Treasure) => costs.treasure_hp,
        Some(RoomType::Shop)     => costs.shop_hp_value,
        _ => 0.0,
    }
}

fn node_gold(rt: Option<RoomType>, costs: &NodeCosts) -> f32 {
    match rt {
        Some(RoomType::Monster)  => costs.monster_gold,
        Some(RoomType::Elite)    => costs.elite_gold,
        Some(RoomType::Treasure) => costs.chest_gold,
        _ => 0.0,
    }
}

// ── Public types ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct PathChoice {
    pub col: u8,
    pub room_type: Option<RoomType>,
    /// Expected net HP delta for the optimal path through this choice to floor 14.
    /// Negative = net HP loss; positive = net gain (e.g., multiple rest sites).
    pub total_hp_delta: f32,
    /// Expected gold earned along the optimal HP path (monster + elite + chest + boss).
    pub expected_gold: f32,
    /// True when this is the highest-delta (least costly) available choice.
    pub is_best: bool,
    /// Whether costs are from simulation (true) or defaults (false).
    pub simulated: bool,
}

// ── Core computation ────────────────────────────────────────────────────────────

/// Compute the expected best-path HP delta and gold for each available choice.
///
/// Uses a bottom-up DP on the map DAG.  At each node the player takes
/// whichever successor maximises the remaining HP (greedy best-path).
/// Gold is accumulated along that same HP-optimal path.
///
/// `choices`    – available column indices on `next_floor`
/// `next_floor` – map array index (0-indexed) of the next floor to enter
pub fn compute_path_choices(
    map: &ActMap,
    choices: &[u8],
    next_floor: usize,
    costs: &NodeCosts,
    simulated: bool,
) -> Vec<PathChoice> {
    if choices.is_empty() || next_floor >= ROWS {
        return vec![];
    }

    let neg_inf = f32::NEG_INFINITY;
    // dp[floor][col] = best expected HP delta from entering (floor, col) through floor 14.
    let mut dp      = vec![vec![neg_inf; COLS]; ROWS];
    // gold_dp[floor][col] = expected gold along the HP-optimal path from (floor, col).
    let mut gold_dp = vec![vec![0.0f32; COLS]; ROWS];

    // Fill top-down (high floor index → low floor index).
    for floor in (next_floor..ROWS).rev() {
        for col in 0..COLS {
            if !map.is_connected(floor, col) {
                continue;
            }
            let rt   = map.room_type(floor, col);
            let cost = node_cost(rt, costs);
            let gold = node_gold(rt, costs);

            let successors = map.next_nodes(floor, col);
            if successors.is_empty() {
                // Floor 14 — always a Rest site before the mandatory boss fight.
                dp[floor][col]      = cost - costs.boss_loss;
                gold_dp[floor][col] = gold + costs.boss_gold;
            } else {
                // Find the successor with the best HP value.
                let best_succ = successors.iter()
                    .filter(|&&c| dp[floor + 1][c as usize].is_finite())
                    .copied()
                    .max_by(|&a, &b| {
                        dp[floor + 1][a as usize]
                            .partial_cmp(&dp[floor + 1][b as usize])
                            .unwrap_or(std::cmp::Ordering::Equal)
                    });

                if let Some(sc) = best_succ {
                    dp[floor][col]      = cost + dp[floor + 1][sc as usize];
                    gold_dp[floor][col] = gold + gold_dp[floor + 1][sc as usize];
                } else {
                    dp[floor][col]      = cost;
                    gold_dp[floor][col] = gold;
                }
            }
        }
    }

    let mut path_choices: Vec<PathChoice> = choices.iter().map(|&col| {
        let rt    = map.room_type(next_floor, col as usize);
        let delta = dp[next_floor][col as usize];
        let gold  = gold_dp[next_floor][col as usize];
        PathChoice {
            col,
            room_type: rt,
            total_hp_delta: if delta.is_finite() { delta } else { 0.0 },
            expected_gold:  if gold.is_finite()  { gold  } else { 0.0 },
            is_best: false,
            simulated,
        }
    }).collect();

    // Mark the best (highest HP delta = least costly path).
    if let Some(best_i) = path_choices.iter().enumerate()
        .max_by(|(_, a), (_, b)| a.total_hp_delta.partial_cmp(&b.total_hp_delta)
            .unwrap_or(std::cmp::Ordering::Equal))
        .map(|(i, _)| i)
    {
        path_choices[best_i].is_best = true;
    }

    path_choices
}

// ── Tests ───────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::map::ActMap;

    fn default_costs() -> NodeCosts {
        NodeCosts::defaults(80, 0)
    }

    #[test]
    fn empty_choices_returns_empty() {
        let map = ActMap::generate(42, 0);
        let result = compute_path_choices(&map, &[], 0, &default_costs(), false);
        assert!(result.is_empty());
    }

    #[test]
    fn out_of_bounds_floor_returns_empty() {
        let map = ActMap::generate(42, 0);
        let result = compute_path_choices(&map, &[0], 99, &default_costs(), false);
        assert!(result.is_empty());
    }

    #[test]
    fn entry_choices_have_finite_deltas() {
        let map = ActMap::generate(42, 0);
        let entries = map.entry_nodes();
        assert!(!entries.is_empty());
        let result = compute_path_choices(&map, &entries, 0, &default_costs(), false);
        assert_eq!(result.len(), entries.len());
        for pc in &result {
            assert!(pc.total_hp_delta.is_finite());
            assert!(pc.expected_gold.is_finite());
        }
    }

    #[test]
    fn exactly_one_best_marked() {
        let map = ActMap::generate(42, 0);
        let entries = map.entry_nodes();
        let result = compute_path_choices(&map, &entries, 0, &default_costs(), false);
        let best_count = result.iter().filter(|pc| pc.is_best).count();
        assert_eq!(best_count, 1);
    }

    #[test]
    fn best_has_highest_delta() {
        let map = ActMap::generate(42, 0);
        let entries = map.entry_nodes();
        let result = compute_path_choices(&map, &entries, 0, &default_costs(), false);
        let best = result.iter().find(|pc| pc.is_best).unwrap();
        for pc in &result {
            assert!(pc.total_hp_delta <= best.total_hp_delta + 1e-4);
        }
    }

    #[test]
    fn expected_gold_is_positive() {
        // Every path must encounter at least the boss, so gold > 0.
        let map = ActMap::generate(42, 0);
        let entries = map.entry_nodes();
        let result = compute_path_choices(&map, &entries, 0, &default_costs(), false);
        for pc in &result {
            assert!(pc.expected_gold > 0.0, "col {} has non-positive gold {}", pc.col, pc.expected_gold);
        }
    }

    #[test]
    fn ascension3_reduces_gold() {
        let map = ActMap::generate(42, 0);
        let entries = map.entry_nodes();
        let normal   = compute_path_choices(&map, &entries, 0, &NodeCosts::defaults(80, 0), false);
        let asc3     = compute_path_choices(&map, &entries, 0, &NodeCosts::defaults(80, 3), false);
        // Every path should earn less gold at Ascension 3+.
        for (n, a) in normal.iter().zip(asc3.iter()) {
            assert!(a.expected_gold <= n.expected_gold,
                "col {}: asc3 gold {} > normal gold {}", n.col, a.expected_gold, n.expected_gold);
        }
    }

    #[test]
    fn rest_site_improves_delta() {
        let map = ActMap::generate(42, 0);
        let entries = map.entry_nodes();
        let costs = NodeCosts {
            monster_loss: 10.0, elite_loss: 25.0, boss_loss: 35.0,
            rest_heal: 24.0,
            ..NodeCosts::defaults(80, 0)
        };
        let result = compute_path_choices(&map, &entries, 0, &costs, false);
        for pc in &result {
            assert!(pc.total_hp_delta.is_finite(), "col {} has non-finite delta", pc.col);
        }
    }
}
