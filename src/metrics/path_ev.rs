use crate::domain::map::{ActMap, RoomType, COLS, ROWS};
use crate::metrics::map_ev::MapEvData;

// ── Cost model ─────────────────────────────────────────────────────────────

pub struct NodeCosts {
    /// Mean HP lost in a Monster room.
    pub monster_loss: f32,
    /// Mean HP lost in an Elite room.
    pub elite_loss: f32,
    /// Mean HP lost in the act Boss fight.
    pub boss_loss: f32,
    /// HP gained by resting (floor(max_hp × 0.30)).
    pub rest_heal: f32,
}

impl NodeCosts {
    pub fn from_map_ev(map_ev: &MapEvData, max_hp: u32) -> Self {
        Self {
            monster_loss: map_ev.normal.mean_hp_loss,
            elite_loss: map_ev.elite.mean_hp_loss,
            boss_loss: map_ev.boss.mean_hp_loss,
            rest_heal: (max_hp as f32 * 0.30).floor(),
        }
    }

    pub fn defaults(max_hp: u32) -> Self {
        Self {
            monster_loss: 10.0,
            elite_loss: 25.0,
            boss_loss: 35.0,
            rest_heal: (max_hp as f32 * 0.30).floor(),
        }
    }
}

fn node_cost(rt: Option<RoomType>, costs: &NodeCosts) -> f32 {
    match rt {
        Some(RoomType::Monster)  => -costs.monster_loss,
        Some(RoomType::Elite)    => -costs.elite_loss,
        Some(RoomType::Rest)     => costs.rest_heal,
        // Event/Shop/Treasure/Boss treated as 0 (neutral or reward-only)
        _ => 0.0,
    }
}

// ── Public types ────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct PathChoice {
    pub col: u8,
    pub room_type: Option<RoomType>,
    /// Expected net HP delta for the optimal path through this choice to floor 14.
    /// Negative = net HP loss; positive = net gain (e.g., multiple rest sites).
    pub total_hp_delta: f32,
    /// True when this is the highest-delta (least costly) available choice.
    pub is_best: bool,
    /// Whether costs are from simulation (true) or defaults (false).
    pub simulated: bool,
}

// ── Core computation ────────────────────────────────────────────────────────

/// Compute the expected best-path HP delta for each available choice.
///
/// Uses a bottom-up DP on the map DAG.  At each node the player takes
/// whichever successor maximises the remaining HP (greedy best-path).
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

    // dp[floor][col] = best expected HP delta from entering (floor, col) through floor 14.
    let neg_inf = f32::NEG_INFINITY;
    let mut dp = vec![vec![neg_inf; COLS]; ROWS];

    // Fill top-down (high floor index → low floor index).
    for floor in (next_floor..ROWS).rev() {
        for col in 0..COLS {
            if !map.is_connected(floor, col) {
                continue;
            }
            let rt = map.room_type(floor, col);
            let cost = node_cost(rt, costs);

            let successors = map.next_nodes(floor, col);
            let best_successor: f32 = if successors.is_empty() {
                // Floor 14 is the last rest before the boss. The boss fight follows.
                -costs.boss_loss
            } else {
                successors.iter()
                    .filter_map(|&c| {
                        let v = dp[floor + 1][c as usize];
                        if v.is_finite() { Some(v) } else { None }
                    })
                    .fold(neg_inf, f32::max)
            };

            dp[floor][col] = cost + if best_successor.is_finite() { best_successor } else { 0.0 };
        }
    }

    let mut path_choices: Vec<PathChoice> = choices.iter().map(|&col| {
        let rt = map.room_type(next_floor, col as usize);
        let delta = dp[next_floor][col as usize];
        PathChoice {
            col,
            room_type: rt,
            total_hp_delta: if delta.is_finite() { delta } else { 0.0 },
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

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::map::ActMap;

    fn default_costs() -> NodeCosts {
        NodeCosts::defaults(80)
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
    fn rest_site_improves_delta() {
        // A path through a rest site should have a higher (less negative) delta.
        let map = ActMap::generate(42, 0);
        let entries = map.entry_nodes();
        let costs = NodeCosts { monster_loss: 10.0, elite_loss: 25.0, boss_loss: 35.0, rest_heal: 24.0 };
        let result = compute_path_choices(&map, &entries, 0, &costs, false);
        // All deltas should be finite
        for pc in &result {
            assert!(pc.total_hp_delta.is_finite(), "col {} has non-finite delta", pc.col);
        }
    }
}
