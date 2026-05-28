//! Act map generation — STS reverse-engineered algorithm.
//!
//! Algorithm source:
//! https://www.reddit.com/r/slaythespire/comments/ndqweh/
//!
//! Key rules (STS1 baseline, assumed unchanged in STS2):
//!   - 7 columns × 15 rows, fixed for all acts
//!   - 6 paths generated bottom-to-top; no crossing edges; first 2 starts differ
//!   - Floor-0→1 convergent edges are trimmed
//!   - Floor 1 = monsters, floor 9 = treasure, floor 15 = rest (pre-assigned, 1-indexed)
//!   - Remaining rooms filled from a shuffled type bucket with adjacency constraints

use rand::seq::SliceRandom;
use rand::{Rng, SeedableRng};
use rand::rngs::StdRng;
use serde::{Deserialize, Serialize};

pub const COLS: usize = 7;
pub const ROWS: usize = 15;
const PATHS: usize = 6;
const TREASURE_FLOOR: usize = 8; // floor 9 (1-indexed), same as STS1
const MIN_SPECIAL_FLOOR: usize = 5; // Elite/Rest locked below floor 6 (1-indexed)
const NO_REST_FLOOR: usize = ROWS - 2; // Rest banned on floor 14 (1-indexed)

const PCT_SHOP: f64 = 0.05;
const PCT_REST: f64 = 0.12;
const PCT_EVENT: f64 = 0.22;
const PCT_ELITE: f64 = 0.08;
const PCT_ELITE_A1: f64 = 0.128; // 0.08 * 1.6 at ascension >= 1

// ── Types ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum RoomType {
    Monster,
    Elite,
    Event,
    Shop,
    Rest,
    Treasure,
    Boss,
}

impl RoomType {
    pub fn label(self) -> &'static str {
        match self {
            Self::Monster  => "M",
            Self::Elite    => "E",
            Self::Event    => "?",
            Self::Shop     => "$",
            Self::Rest     => "R",
            Self::Treasure => "T",
            Self::Boss     => "B",
        }
    }
}

/// Player's current position in the map grid.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct MapPos {
    pub floor: usize,
    pub col: usize,
}

/// A fully generated act map.
///
/// Indexed as `[floor][col]` where `floor ∈ 0..rows` and `col ∈ 0..COLS`.
/// Cells with `None` room type are unreachable.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActMap {
    pub rows: usize,
    /// `room_types[floor][col]` — `None` = not part of any path.
    pub room_types: Vec<Vec<Option<RoomType>>>,
    /// `next[floor][col]` — destination columns on `floor + 1`.
    pub next: Vec<Vec<Vec<u8>>>,
}

impl ActMap {
    /// Generate a new act map deterministically from `seed`.
    ///
    /// - `ascension`: current ascension level (≥1 boosts elite %)
    pub fn generate(seed: u64, ascension: u8) -> Self {
        let rows = ROWS;
        let mut rng = StdRng::seed_from_u64(seed);

        // ── Phase 1: Build path adjacency ────────────────────────────────────
        // next_of[floor][col] = list of destination cols on floor+1
        let mut next_of: Vec<Vec<Vec<usize>>> = vec![vec![vec![]; COLS]; rows];

        let mut first_start: Option<usize> = None;

        for path_idx in 0..PATHS {
            // First two starting columns must differ.
            let start = loop {
                let c = rng.gen_range(0..COLS);
                if path_idx == 1 && Some(c) == first_start { continue; }
                break c;
            };
            if path_idx == 0 { first_start = Some(start); }

            let mut cur = start;
            for floor in 0..(rows - 1) {
                let nxt = choose_next(cur, floor, &next_of, &mut rng);
                add_edge(&mut next_of, floor, cur, nxt);
                cur = nxt;
            }
        }

        // ── Phase 2: Trim convergent floor-0 → floor-1 edges ─────────────────
        trim_convergence(&mut next_of);

        // ── Phase 3: Build reverse (prev) adjacency ───────────────────────────
        let mut prev_of: Vec<Vec<Vec<usize>>> = vec![vec![vec![]; COLS]; rows];
        for floor in 0..(rows - 1) {
            for col in 0..COLS {
                for &dst in &next_of[floor][col] {
                    if !prev_of[floor + 1][dst].contains(&col) {
                        prev_of[floor + 1][dst].push(col);
                    }
                }
            }
        }

        let is_conn = |f: usize, c: usize| {
            !next_of[f][c].is_empty() || !prev_of[f][c].is_empty()
        };

        // ── Phase 4: Pre-assign fixed floors ─────────────────────────────────
        let mut room_types: Vec<Vec<Option<RoomType>>> = vec![vec![None; COLS]; rows];
        for col in 0..COLS {
            if is_conn(0, col)              { room_types[0][col]              = Some(RoomType::Monster);  }
            if is_conn(TREASURE_FLOOR, col) { room_types[TREASURE_FLOOR][col] = Some(RoomType::Treasure); }
            if is_conn(rows - 1, col)       { room_types[rows - 1][col]       = Some(RoomType::Rest);     }
        }

        // ── Phase 5: Build type bucket ────────────────────────────────────────
        let connected_count: usize = (0..rows)
            .flat_map(|f| (0..COLS).map(move |c| (f, c)))
            .filter(|&(f, c)| is_conn(f, c))
            .count();

        let pre_assigned: usize = (0..rows)
            .flat_map(|f| (0..COLS).map(move |c| (f, c)))
            .filter(|&(f, c)| room_types[f][c].is_some())
            .count();

        let unassigned_count = connected_count - pre_assigned;

        let elite_pct = if ascension >= 1 { PCT_ELITE_A1 } else { PCT_ELITE };
        let mut bucket: Vec<RoomType> = Vec::new();
        for _ in 0..(connected_count as f64 * PCT_SHOP ).round() as usize { bucket.push(RoomType::Shop); }
        for _ in 0..(connected_count as f64 * PCT_REST ).round() as usize { bucket.push(RoomType::Rest); }
        for _ in 0..(connected_count as f64 * PCT_EVENT).round() as usize { bucket.push(RoomType::Event); }
        for _ in 0..(connected_count as f64 * elite_pct).round() as usize { bucket.push(RoomType::Elite); }

        while bucket.len() < unassigned_count { bucket.push(RoomType::Monster); }
        bucket.truncate(unassigned_count);
        bucket.shuffle(&mut rng);

        // ── Phase 6: Assign types with constraints ────────────────────────────
        let candidates: Vec<(usize, usize)> = (0..rows)
            .flat_map(|f| (0..COLS).map(move |c| (f, c)))
            .filter(|&(f, c)| is_conn(f, c) && room_types[f][c].is_none())
            .collect();

        'outer: for (floor, col) in candidates {
            for i in 0..bucket.len() {
                let rt = bucket[i];
                if is_compatible(rt, floor, col, &room_types, &prev_of, &next_of) {
                    room_types[floor][col] = Some(rt);
                    bucket.remove(i);
                    continue 'outer;
                }
            }
            // No compatible type found — will become Monster in phase 7.
        }

        // ── Phase 7: Fill leftover connected typeless rooms with monsters ─────
        for floor in 0..rows {
            for col in 0..COLS {
                if is_conn(floor, col) && room_types[floor][col].is_none() {
                    room_types[floor][col] = Some(RoomType::Monster);
                }
            }
        }

        // Convert next_of to u8 for compact storage.
        let next: Vec<Vec<Vec<u8>>> = next_of
            .into_iter()
            .map(|row| row.into_iter().map(|dsts| dsts.into_iter().map(|d| d as u8).collect()).collect())
            .collect();

        ActMap { rows, room_types, next }
    }

    pub fn room_type(&self, floor: usize, col: usize) -> Option<RoomType> {
        self.room_types.get(floor).and_then(|r| r.get(col)).and_then(|&t| t)
    }

    pub fn next_nodes(&self, floor: usize, col: usize) -> &[u8] {
        self.next
            .get(floor)
            .and_then(|r| r.get(col))
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    pub fn is_connected(&self, floor: usize, col: usize) -> bool {
        self.room_types.get(floor).and_then(|r| r.get(col)).map(Option::is_some).unwrap_or(false)
    }

    /// Columns on floor 0 that have outgoing paths.
    pub fn entry_nodes(&self) -> Vec<u8> {
        (0..COLS).filter(|&c| self.is_connected(0, c)).map(|c| c as u8).collect()
    }

    /// Reachable columns on the next floor from `pos`.
    pub fn choices_from(&self, pos: MapPos) -> &[u8] {
        self.next_nodes(pos.floor, pos.col)
    }
}

// ── Algorithm helpers ────────────────────────────────────────────────────────

fn add_edge(next_of: &mut [Vec<Vec<usize>>], floor: usize, src: usize, dst: usize) {
    let row = &mut next_of[floor][src];
    if !row.contains(&dst) {
        row.push(dst);
    }
}

/// Two edges (src→dst) and (other_src→other_dst) cross when they interleave:
/// one source is left of the other but its destination is to the right.
fn would_cross(src: usize, dst: usize, floor: usize, next_of: &[Vec<Vec<usize>>]) -> bool {
    for other_src in 0..COLS {
        for &other_dst in &next_of[floor][other_src] {
            if other_src == src && other_dst == dst { continue; }
            if (src < other_src && dst > other_dst) || (src > other_src && dst < other_dst) {
                return true;
            }
        }
    }
    false
}

/// Pick the next column for a path step: choose from {cur-1, cur, cur+1},
/// randomised, taking the first option that doesn't create a crossing.
fn choose_next(cur: usize, floor: usize, next_of: &[Vec<Vec<usize>>], rng: &mut impl Rng) -> usize {
    let mut candidates: Vec<usize> = Vec::with_capacity(3);
    if cur > 0           { candidates.push(cur - 1); }
    candidates.push(cur);
    if cur < COLS - 1    { candidates.push(cur + 1); }

    candidates.shuffle(rng);
    for &c in &candidates {
        if !would_cross(cur, c, floor, next_of) {
            return c;
        }
    }
    cur // every candidate would cross — stay put (very rare edge case)
}

/// Remove floor-0→1 edges that share a destination, keeping the leftmost source.
fn trim_convergence(next_of: &mut [Vec<Vec<usize>>]) {
    if next_of.is_empty() { return; }
    let mut seen = std::collections::HashSet::new();
    for col in 0..COLS {
        next_of[0][col].retain(|dst| seen.insert(*dst));
    }
}

fn is_compatible(
    rt: RoomType,
    floor: usize,
    col: usize,
    room_types: &[Vec<Option<RoomType>>],
    prev_of: &[Vec<Vec<usize>>],
    next_of: &[Vec<Vec<usize>>],
) -> bool {
    // Elite and Rest are locked out of early floors (below floor 6, 1-indexed).
    if matches!(rt, RoomType::Elite | RoomType::Rest) && floor < MIN_SPECIAL_FLOOR {
        return false;
    }
    // Rest is banned on the second-to-last floor (floor 14, 1-indexed).
    if rt == RoomType::Rest && floor == NO_REST_FLOOR {
        return false;
    }

    if floor == 0 { return true; } // no parents to check

    let check_parent = matches!(rt, RoomType::Elite | RoomType::Shop | RoomType::Rest);

    for &prev_col in &prev_of[floor][col] {
        // Rule 1: parent can't be the same type (for Elite/Shop/Rest).
        if check_parent && room_types[floor - 1][prev_col] == Some(rt) {
            return false;
        }
        // Rule 2: siblings (sharing this parent) can't be the same type.
        for &sibling in &next_of[floor - 1][prev_col] {
            if sibling != col && room_types[floor][sibling] == Some(rt) {
                return false;
            }
        }
    }

    true
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn assert_no_crossings(map: &ActMap) {
        for floor in 0..(map.rows - 1) {
            for col_a in 0..COLS {
                for &dst_a in map.next_nodes(floor, col_a) {
                    for col_b in (col_a + 1)..COLS {
                        for &dst_b in map.next_nodes(floor, col_b) {
                            // col_a < col_b; crossing iff dst_a > dst_b
                            assert!(
                                (dst_a as usize) <= (dst_b as usize),
                                "floor {floor}: {col_a}→{dst_a} crosses {col_b}→{dst_b}"
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn generates_correct_dimensions() {
        let map = ActMap::generate(42, 0);
        assert_eq!(map.rows, ROWS);
        assert_eq!(map.room_types.len(), ROWS);
        for row in &map.room_types { assert_eq!(row.len(), COLS); }
    }

    #[test]
    fn floor_zero_all_monsters() {
        let map = ActMap::generate(42, 0);
        for col in 0..COLS {
            if map.is_connected(0, col) {
                assert_eq!(map.room_type(0, col), Some(RoomType::Monster));
            }
        }
    }

    #[test]
    fn treasure_floor_correct() {
        let map = ActMap::generate(42, 0);
        for col in 0..COLS {
            if map.is_connected(TREASURE_FLOOR, col) {
                assert_eq!(map.room_type(TREASURE_FLOOR, col), Some(RoomType::Treasure));
            }
        }
    }

    #[test]
    fn last_floor_is_rest() {
        let map = ActMap::generate(42, 0);
        let last = map.rows - 1;
        for col in 0..COLS {
            if map.is_connected(last, col) {
                assert_eq!(map.room_type(last, col), Some(RoomType::Rest));
            }
        }
    }

    #[test]
    fn at_least_two_entry_nodes() {
        for seed in 0u64..20 {
            let map = ActMap::generate(seed, 0);
            assert!(map.entry_nodes().len() >= 2, "seed {seed}: fewer than 2 entry nodes");
        }
    }

    #[test]
    fn no_crossing_edges() {
        for seed in 0u64..30 {
            assert_no_crossings(&ActMap::generate(seed, 0));
        }
    }

    #[test]
    fn all_connected_nodes_have_type() {
        let map = ActMap::generate(42, 0);
        for floor in 0..map.rows {
            for col in 0..COLS {
                if map.is_connected(floor, col) {
                    assert!(map.room_type(floor, col).is_some(),
                        "connected ({floor},{col}) has no type");
                }
            }
        }
    }

    #[test]
    fn no_elite_on_early_floors() {
        let map = ActMap::generate(42, 0);
        for floor in 0..MIN_SPECIAL_FLOOR {
            for col in 0..COLS {
                assert_ne!(map.room_type(floor, col), Some(RoomType::Elite),
                    "Elite found at floor {floor}");
            }
        }
    }

    #[test]
    fn no_rest_on_second_to_last_floor() {
        let map = ActMap::generate(42, 0);
        for col in 0..COLS {
            assert_ne!(map.room_type(NO_REST_FLOOR, col), Some(RoomType::Rest),
                "Rest found at floor {NO_REST_FLOOR} (second-to-last)");
        }
    }

    #[test]
    fn no_convergent_floor0_edges() {
        let map = ActMap::generate(42, 0);
        let mut seen_dsts: HashSet<u8> = HashSet::new();
        for col in 0..COLS {
            for &dst in map.next_nodes(0, col) {
                assert!(seen_dsts.insert(dst),
                    "floor-0→1: destination col {dst} reachable from multiple sources");
            }
        }
    }

    #[test]
    fn choices_from_returns_next_nodes() {
        let map = ActMap::generate(42, 0);
        if let Some(start) = map.entry_nodes().first().copied() {
            let pos = MapPos { floor: 0, col: start as usize };
            let choices = map.choices_from(pos);
            assert!(!choices.is_empty());
        }
    }
}
