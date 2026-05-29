use rand::Rng;
use rand::seq::SliceRandom;

use crate::data::api::{SpireApiEncounter, SpireApiMonster};
use crate::domain::card::Card;
use crate::domain::encounter::normalize_act;
use crate::domain::map::{ActMap, RoomType, ROWS};
use crate::metrics::deck_dash::compute_deck_stats_vs_pool;
use crate::metrics::map_ev::card_value_score;
use crate::metrics::path_ev::{build_dp_table, NodeCosts};

// ── Simulation constants ───────────────────────────────────────────────────────

// Lighter than deck_dash (3×3 = 9 playouts per fight vs 5×5 = 25); fast enough
// to run once per selectable node across 3 acts.
const SIM_COMBATS: u32 = 3;
const SIM_ENCOUNTERS: usize = 3;

// HP-equivalent credited for acquiring a relic of each tier.
const RELIC_HP_COMMON: f32   =  5.0;
const RELIC_HP_UNCOMMON: f32 = 10.0;
const RELIC_HP_RARE: f32     = 18.0;
const RELIC_HP_BOON: f32     =  8.0; // ancient boon (mix of tiers)

// Shop item prices (gold).
const SHOP_CARD_PRICE: u32    =  75;
const SHOP_RELIC_PRICE: u32   = 210;
const SHOP_REMOVAL_PRICE: u32 =  75;

// ── Internal state ─────────────────────────────────────────────────────────────

#[derive(Clone)]
struct SimState {
    hp:           u32,
    max_hp:       u32,
    deck:         Vec<Card>,
    relics:       Vec<String>,
    gold:         u32,
    ascension:    u8,
    gold_earned:  u32,
    gold_spent:   u32,
}

// ── Act result (internal) ──────────────────────────────────────────────────────

struct ActResult {
    boss_entry_hp: u32,
    boss_defeated: bool,
    /// Human-readable cause of death, e.g. "F9" or "Boss". None = survived.
    death_note: Option<String>,
}

// ── Public output ──────────────────────────────────────────────────────────────

/// Full 3-act run projection for one selectable map node.
#[derive(Debug, Clone)]
pub struct PathProjection {
    /// Column index this projection starts from.
    pub col: u8,
    /// Where the player dies, e.g. `Some("A1 F9")`, `Some("A2 Boss")`. None = WIN.
    pub death_label: Option<String>,
    /// True when all acts are projected to be won.
    pub likely_win: bool,
    /// HP entering each act's boss fight. 0 = dead before that boss.
    pub boss_entry_hp: [u32; 3],
    /// Whether each act's boss is projected to be defeated.
    pub boss_defeated: [bool; 3],
    /// Total gold earned across the projected run.
    pub gold_earned: u32,
    /// Total gold spent across the projected run.
    pub gold_spent: u32,
}

// ── Entry point ────────────────────────────────────────────────────────────────

/// Compute a forward-simulated run projection for each currently selectable node.
///
/// For the **current act** the simulation traces the DP-optimal path through the
/// real map, threading HP / deck / relic / gold state sequentially. For **future
/// acts** it generates a random representative map and simulates it the same way.
///
/// At each combat node the deck is re-evaluated at the *current* HP (not the
/// player's HP at run start), so damage estimates degrade naturally as the player
/// is worn down. After every fight the deck is improved with a blind card pick.
/// After elites, treasures, bosses, and act ancients the player gains relics
/// (credited as HP-equivalent value). At shop nodes the player buys greedily
/// within budget.
#[allow(clippy::too_many_arguments)]
pub fn simulate_path_choices(
    map:            &ActMap,
    choices:        &[u8],
    next_floor:     usize,
    hp:             u32,
    max_hp:         u32,
    deck:           &[Card],
    relics:         &[String],
    gold:           u32,
    ascension:      u8,
    sub_act:        &str,
    all_encounters: &[SpireApiEncounter],
    all_monsters:   &[SpireApiMonster],
    class_cards:    &[Card],
    event_hp_delta: f32,
    rng:            &mut impl Rng,
) -> Vec<PathProjection> {
    let future_acts = crate::metrics::run_ev::acts_after(sub_act);

    choices.iter().map(|&col| {
        let mut state = SimState {
            hp, max_hp, deck: deck.to_vec(), relics: relics.to_vec(),
            gold, ascension, gold_earned: 0, gold_spent: 0,
        };

        // ── Act 1: real map ────────────────────────────────────────────────────
        let act1 = simulate_act_map(
            &mut state, map, col, next_floor, sub_act,
            all_encounters, all_monsters, class_cards, event_hp_delta, rng,
        );

        let mut boss_entry  = [act1.boss_entry_hp, 0, 0];
        let mut boss_beaten = [act1.boss_defeated, false, false];

        if !act1.boss_defeated {
            return PathProjection {
                col,
                death_label: act1.death_note.map(|n| format!("A1 {n}")),
                likely_win: false,
                boss_entry_hp: boss_entry,
                boss_defeated: boss_beaten,
                gold_earned: state.gold_earned,
                gold_spent:  state.gold_spent,
            };
        }

        // Ancient heals to full + boon relic.
        state.hp = state.max_hp;
        credit_hp(&mut state, RELIC_HP_BOON);

        // ── Future acts: generated maps ────────────────────────────────────────
        for (i, &(future_sub, _)) in future_acts.iter().enumerate().take(2) {
            let seed: u64 = rng.r#gen();
            let fut_map   = ActMap::generate(seed, ascension);

            // Best entry node for the generated map.
            let costs       = NodeCosts::defaults(max_hp, ascension);
            let dp          = build_dp_table(&fut_map, 0, &costs);
            let entries     = fut_map.entry_nodes();
            let best_entry  = entries.iter().copied()
                .max_by(|&a, &b| dp[0][a as usize].partial_cmp(&dp[0][b as usize])
                    .unwrap_or(std::cmp::Ordering::Equal))
                .unwrap_or(*entries.first().unwrap_or(&3));

            let act = simulate_act_map(
                &mut state, &fut_map, best_entry, 0, future_sub,
                all_encounters, all_monsters, class_cards, event_hp_delta, rng,
            );

            boss_entry [i + 1] = act.boss_entry_hp;
            boss_beaten[i + 1] = act.boss_defeated;

            if !act.boss_defeated {
                return PathProjection {
                    col,
                    death_label: act.death_note.map(|n| format!("A{} {n}", i + 2)),
                    likely_win: false,
                    boss_entry_hp: boss_entry,
                    boss_defeated: boss_beaten,
                    gold_earned: state.gold_earned,
                    gold_spent:  state.gold_spent,
                };
            }

            // Heal to full + ancient boon before next act.
            state.hp = state.max_hp;
            credit_hp(&mut state, RELIC_HP_BOON);
        }

        // Survived everything.
        PathProjection {
            col,
            death_label: None,
            likely_win:  true,
            boss_entry_hp: boss_entry,
            boss_defeated: boss_beaten,
            gold_earned: state.gold_earned,
            gold_spent:  state.gold_spent,
        }
    }).collect()
}

// ── Single-act simulation ──────────────────────────────────────────────────────

fn simulate_act_map(
    state:          &mut SimState,
    map:            &ActMap,
    start_col:      u8,
    start_floor:    usize,
    sub_act:        &str,
    encounters:     &[SpireApiEncounter],
    monsters:       &[SpireApiMonster],
    class_cards:    &[Card],
    event_hp_delta: f32,
    rng:            &mut impl Rng,
) -> ActResult {
    // Pre-filter encounter pools once per act.
    let by_room = |room: &str| -> Vec<&SpireApiEncounter> {
        encounters.iter()
            .filter(|e| {
                let a = e.act.as_deref().map(normalize_act).unwrap_or("other");
                a == sub_act && e.room_type.as_deref() == Some(room)
            })
            .collect()
    };
    let monster_pool = by_room("Monster");
    let elite_pool   = by_room("Elite");
    let boss_pool    = by_room("Boss");

    // DP table for path selection (default costs — accurate for act topology).
    let costs = NodeCosts::defaults(state.max_hp, state.ascension);
    let dp    = build_dp_table(map, start_floor, &costs);

    let mut floor = start_floor;
    let mut col   = start_col as usize;

    loop {
        let room_type = map.room_type(floor, col);

        let alive = apply_room(
            state, room_type, floor as u8, sub_act,
            &monster_pool, &elite_pool, monsters,
            class_cards, event_hp_delta, rng,
        );

        if !alive {
            return ActResult {
                boss_entry_hp: 0,
                boss_defeated: false,
                death_note: Some(format!("F{}", floor + 1)),
            };
        }

        let succs = map.next_nodes(floor, col);
        if succs.is_empty() {
            // Leaf node — implicit boss fight next.
            break;
        }

        // Follow DP-optimal successor.
        let next = succs.iter().copied()
            .filter(|&c| floor + 1 < ROWS && dp[floor + 1][c as usize].is_finite())
            .max_by(|&a, &b| {
                dp[floor + 1][a as usize]
                    .partial_cmp(&dp[floor + 1][b as usize])
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .unwrap_or(succs[0]);

        floor += 1;
        col   = next as usize;
    }

    // Boss fight.
    let boss_entry_hp = state.hp;
    let boss_loss = sim_combat_loss(state, &boss_pool, sub_act, monsters, rng, 35);
    if boss_loss >= state.hp {
        state.hp = 0;
        return ActResult { boss_entry_hp, boss_defeated: false, death_note: Some("Boss".into()) };
    }
    state.hp -= boss_loss;

    // Boss rewards: gold + card pick + rare relic.
    let boss_gold = if state.ascension >= 3 { 75 } else { 100 };
    state.gold        += boss_gold;
    state.gold_earned += boss_gold;
    blind_card_pick(&mut state.deck, class_cards, rng);
    credit_hp(state, RELIC_HP_RARE);

    ActResult { boss_entry_hp, boss_defeated: true, death_note: None }
}

// ── Room resolution ────────────────────────────────────────────────────────────

fn apply_room(
    state:          &mut SimState,
    room_type:      Option<RoomType>,
    floor:          u8,
    sub_act:        &str,
    monster_pool:   &[&SpireApiEncounter],
    elite_pool:     &[&SpireApiEncounter],
    monsters:       &[SpireApiMonster],
    class_cards:    &[Card],
    event_hp_delta: f32,
    rng:            &mut impl Rng,
) -> bool /* alive */ {
    let _ = floor; // available for future per-floor logic
    match room_type {
        Some(RoomType::Monster) => {
            let loss = sim_combat_loss(state, monster_pool, sub_act, monsters, rng, 8);
            if loss >= state.hp { state.hp = 0; return false; }
            state.hp -= loss;
            let gold = if state.ascension >= 3 { 11 } else { 15 };
            state.gold += gold; state.gold_earned += gold;
            blind_card_pick(&mut state.deck, class_cards, rng);
            true
        }
        Some(RoomType::Elite) => {
            let loss = sim_combat_loss(state, elite_pool, sub_act, monsters, rng, 20);
            if loss >= state.hp { state.hp = 0; return false; }
            state.hp -= loss;
            let gold = if state.ascension >= 3 { 30 } else { 40 };
            state.gold += gold; state.gold_earned += gold;
            blind_card_pick(&mut state.deck, class_cards, rng);
            // Elite drops an uncommon relic on average.
            credit_hp(state, RELIC_HP_UNCOMMON);
            true
        }
        Some(RoomType::Rest) => {
            // Heal if below 75% of max; otherwise upgrade would be stronger but
            // we treat healing as the conservative safe choice for simulation.
            let deficit   = state.max_hp.saturating_sub(state.hp);
            let full_heal = (state.max_hp as f32 * 0.30).floor() as u32;
            state.hp = (state.hp + full_heal.min(deficit)).min(state.max_hp);
            true
        }
        Some(RoomType::Event) => {
            if event_hp_delta >= 0.0 {
                state.hp = (state.hp + event_hp_delta as u32).min(state.max_hp);
            } else {
                let loss = (-event_hp_delta) as u32;
                if loss >= state.hp { state.hp = 0; return false; }
                state.hp -= loss;
            }
            true
        }
        Some(RoomType::Treasure) => {
            let gold = if state.ascension >= 3 { 36 } else { 47 };
            state.gold += gold; state.gold_earned += gold;
            // Treasure chest: mix of common/uncommon relics → ~8 HP average.
            credit_hp(state, (RELIC_HP_COMMON + RELIC_HP_UNCOMMON) / 2.0);
            true
        }
        Some(RoomType::Shop) => {
            apply_shop(state, class_cards, rng);
            true
        }
        _ => true,
    }
}

// ── Shop ───────────────────────────────────────────────────────────────────────

fn apply_shop(state: &mut SimState, class_cards: &[Card], rng: &mut impl Rng) {
    // Buy greedily by HP/gold ratio:
    //   Card  : ~7 HP @ 75g  = 0.093 ratio (best)
    //   Relic : ~8 HP @ 210g = 0.038 ratio
    //   Removal: depends on deck; model as ~5 HP improvement @ 75g if deck is large
    if state.gold >= SHOP_CARD_PRICE {
        blind_card_pick(&mut state.deck, class_cards, rng);
        state.gold        -= SHOP_CARD_PRICE;
        state.gold_spent  += SHOP_CARD_PRICE;
    }
    if state.gold >= SHOP_RELIC_PRICE {
        credit_hp(state, RELIC_HP_UNCOMMON); // shop relics tend to be shop/uncommon tier
        state.gold       -= SHOP_RELIC_PRICE;
        state.gold_spent += SHOP_RELIC_PRICE;
    }
    // Removal improves deck if it has grown beyond 12 cards.
    if state.gold >= SHOP_REMOVAL_PRICE && state.deck.len() > 12 {
        remove_worst_card(&mut state.deck);
        state.gold       -= SHOP_REMOVAL_PRICE;
        state.gold_spent += SHOP_REMOVAL_PRICE;
    }
}

// ── Combat helpers ─────────────────────────────────────────────────────────────

/// Run a lightweight combat simulation against a pre-filtered encounter pool.
/// Returns expected HP lost, capped at `state.hp` (can't lose more than you have).
/// Falls back to `default_loss` when the pool is empty.
fn sim_combat_loss(
    state:        &SimState,
    pool:         &[&SpireApiEncounter],
    sub_act:      &str,
    monsters:     &[SpireApiMonster],
    rng:          &mut impl Rng,
    default_loss: u32,
) -> u32 {
    if pool.is_empty() {
        return default_loss.min(state.hp);
    }
    let stats = compute_deck_stats_vs_pool(
        &state.deck, state.hp, state.max_hp, sub_act,
        pool, SIM_ENCOUNTERS, SIM_COMBATS,
        monsters, &state.relics, rng,
    );
    (stats.mean_hp_loss.round() as u32).min(state.hp)
}

// ── Deck / relic improvement ───────────────────────────────────────────────────

/// Add the best card (by heuristic score) from a random sample of 3 class cards.
fn blind_card_pick(deck: &mut Vec<Card>, class_cards: &[Card], rng: &mut impl Rng) {
    if class_cards.is_empty() { return; }
    // Sample 3 cards (with replacement) and take the highest-value one.
    let sample: Vec<&Card> = (0..3).filter_map(|_| class_cards.choose(rng)).collect();
    if let Some(best) = sample.iter().max_by(|a, b| {
        card_value_score(a)
            .partial_cmp(&card_value_score(b))
            .unwrap_or(std::cmp::Ordering::Equal)
    }) {
        deck.push((*best).clone());
    }
}

/// Credit a relic gain as direct HP value (capped at max HP).
fn credit_hp(state: &mut SimState, value: f32) {
    let gain = value as u32;
    state.hp = (state.hp + gain).min(state.max_hp);
}

/// Remove the worst card from the deck (by heuristic score).
fn remove_worst_card(deck: &mut Vec<Card>) {
    if deck.is_empty() { return; }
    let worst = deck.iter().enumerate()
        .min_by(|(_, a), (_, b)| {
            card_value_score(a)
                .partial_cmp(&card_value_score(b))
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .map(|(i, _)| i)
        .unwrap_or(0);
    deck.remove(worst);
}

// ── Tests ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use rand::SeedableRng;
    use rand::rngs::StdRng;
    use crate::domain::map::ActMap;

    fn rng() -> StdRng { StdRng::seed_from_u64(77) }
    fn deck() -> Vec<Card> { crate::domain::catalog::ironclad::starter_deck() }
    fn class_cards() -> Vec<Card> { crate::domain::catalog::cards_for_character("ironclad") }

    #[test]
    fn projection_count_matches_choices() {
        let map = ActMap::generate(42, 0);
        let entries = map.entry_nodes();
        let mut r = rng();
        let projs = simulate_path_choices(
            &map, &entries, 0,
            80, 80, &deck(), &[], 100, 0,
            "overgrowth", &[], &[], &class_cards(), -2.0, &mut r,
        );
        assert_eq!(projs.len(), entries.len());
    }

    #[test]
    fn dead_choice_has_zero_boss_entry() {
        // Zero HP entering = instant death at first fight.
        let map = ActMap::generate(42, 0);
        let entries = map.entry_nodes();
        let mut r = rng();
        let projs = simulate_path_choices(
            &map, &entries, 0,
            1, 80, &deck(), &[], 0, 0, // 1 HP = die on first hit
            "overgrowth", &[], &[], &class_cards(), 0.0, &mut r,
        );
        for p in &projs {
            assert!(!p.likely_win);
        }
    }

    #[test]
    fn no_encounters_uses_defaults_and_does_not_panic() {
        let map = ActMap::generate(99, 0);
        let entries = map.entry_nodes();
        let mut r = rng();
        let projs = simulate_path_choices(
            &map, &entries, 0,
            80, 80, &deck(), &[], 150, 0,
            "overgrowth", &[], &[], &class_cards(), -2.0, &mut r,
        );
        assert!(!projs.is_empty());
        for p in &projs {
            assert!(p.boss_entry_hp[0] == 0 || p.boss_entry_hp[0] <= 80);
        }
    }

    #[test]
    fn blind_card_pick_grows_deck() {
        let mut deck = deck();
        let n_before = deck.len();
        let cards = class_cards();
        let mut r = rng();
        blind_card_pick(&mut deck, &cards, &mut r);
        assert_eq!(deck.len(), n_before + 1);
    }

    #[test]
    fn remove_worst_card_shrinks_deck() {
        let mut deck = deck();
        let n_before = deck.len();
        remove_worst_card(&mut deck);
        assert_eq!(deck.len(), n_before - 1);
    }

    #[test]
    fn credit_hp_capped_at_max() {
        let mut state = SimState {
            hp: 70, max_hp: 80, deck: vec![], relics: vec![],
            gold: 0, ascension: 0, gold_earned: 0, gold_spent: 0,
        };
        credit_hp(&mut state, 100.0);
        assert_eq!(state.hp, 80);
    }

    #[test]
    fn sim_combat_loss_capped_at_entry_hp() {
        // With an empty pool, default_loss applies; capped at state.hp.
        let state = SimState {
            hp: 5, max_hp: 80, deck: deck(), relics: vec![],
            gold: 0, ascension: 0, gold_earned: 0, gold_spent: 0,
        };
        let mut r = rng();
        let loss = sim_combat_loss(&state, &[], "overgrowth", &[], &mut r, 999);
        assert_eq!(loss, 5, "loss must not exceed current HP");
    }
}
