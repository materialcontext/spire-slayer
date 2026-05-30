use rand::Rng;
use rand::seq::SliceRandom;

use crate::domain::card::{Card, Rarity};

pub const INITIAL_RARE_OFFSET: i32 = -5;
const MAX_RARE_OFFSET: i32 = 40;

/// Where the card reward comes from; determines base rarity weights.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RewardKind {
    Monster,
    Elite,
    Boss,
    #[allow(dead_code)]
    Shop,
}

/// Generate a 3-card offer from `pool`, updating `rare_offset` in place.
///
/// Each card's rarity is rolled independently.  The offset increments by +1
/// for every non-rare card rolled and resets to INITIAL_RARE_OFFSET when a
/// rare is rolled.  This matches the STS2 spec: the counter changes at offer
/// generation time, so a skipped reward still advances the counter.
pub fn sample_offer(
    pool: &[Card],
    kind: RewardKind,
    rare_offset: &mut i32,
    ascension: u8,
    rng: &mut impl Rng,
) -> Vec<Card> {
    let by_rarity = ByRarity::new(pool);
    (0..3)
        .filter_map(|_| {
            let rarity = roll_rarity(kind, rare_offset, ascension, rng);
            by_rarity.pick(rarity, rng).cloned()
        })
        .collect()
}

/// Roll the rarity of a single card, updating the offset.
fn roll_rarity(kind: RewardKind, rare_offset: &mut i32, ascension: u8, rng: &mut impl Rng) -> Rarity {
    if kind == RewardKind::Boss {
        // Boss rewards are always Rare; offset resets.
        *rare_offset = INITIAL_RARE_OFFSET;
        return Rarity::Rare;
    }

    let (base_rare, base_uncommon) = base_weights(kind);
    // A7+: rare cards are less common (halve rare chance)
    let base_rare = if ascension >= 7 { base_rare / 2 } else { base_rare };
    let effective_rare = (base_rare + *rare_offset).max(0);
    // Roll 0..100 (integer percentages); use gen_range for determinism.
    let roll: i32 = rng.gen_range(0..100);

    if roll < effective_rare {
        *rare_offset = INITIAL_RARE_OFFSET;
        Rarity::Rare
    } else if roll < effective_rare + base_uncommon {
        *rare_offset = (*rare_offset + 1).min(MAX_RARE_OFFSET);
        Rarity::Uncommon
    } else {
        *rare_offset = (*rare_offset + 1).min(MAX_RARE_OFFSET);
        Rarity::Common
    }
}

/// Base (pre-offset) rare% and uncommon% for each reward source.
fn base_weights(kind: RewardKind) -> (i32, i32) {
    match kind {
        RewardKind::Monster => (3,  37),
        RewardKind::Elite   => (10, 40),
        RewardKind::Boss    => (100, 0),
        RewardKind::Shop    => (9,  37),
    }
}

/// Pre-filtered card slices by rarity so we don't rebuild them per card.
struct ByRarity<'a> {
    rares:     Vec<&'a Card>,
    uncommons: Vec<&'a Card>,
    commons:   Vec<&'a Card>,
}

impl<'a> ByRarity<'a> {
    fn new(pool: &'a [Card]) -> Self {
        Self {
            // Ancient rarity is for relics only; it never appears in card rewards.
            rares:     pool.iter().filter(|c| c.rarity == Rarity::Rare).collect(),
            uncommons: pool.iter().filter(|c| c.rarity == Rarity::Uncommon).collect(),
            commons:   pool.iter().filter(|c| c.rarity == Rarity::Common).collect(),
        }
    }

    /// Pick a random card of `target` rarity, falling back down the ladder if
    /// that tier is empty.
    fn pick(&self, target: Rarity, rng: &mut impl Rng) -> Option<&'a Card> {
        match target {
            Rarity::Rare => {
                self.rares.choose(rng)
                    .or_else(|| self.uncommons.choose(rng))
                    .or_else(|| self.commons.choose(rng))
            }
            Rarity::Uncommon => {
                self.uncommons.choose(rng)
                    .or_else(|| self.commons.choose(rng))
            }
            _ => self.commons.choose(rng),
        }
        .copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::SeedableRng;
    use rand::rngs::StdRng;
    use crate::domain::catalog::ironclad;

    fn rng() -> StdRng { StdRng::seed_from_u64(42) }

    #[test]
    fn sample_offer_returns_three_cards() {
        let pool = ironclad::all_cards();
        let mut offset = INITIAL_RARE_OFFSET;
        let offer = sample_offer(&pool, RewardKind::Monster, &mut offset, 0, &mut rng());
        assert_eq!(offer.len(), 3);
    }

    #[test]
    fn boss_rewards_always_rare() {
        let pool = ironclad::all_cards();
        let mut offset = INITIAL_RARE_OFFSET;
        let mut rng = rng();
        for _ in 0..20 {
            let offer = sample_offer(&pool, RewardKind::Boss, &mut offset, 0, &mut rng);
            assert!(offer.iter().all(|c| c.rarity == Rarity::Rare));
        }
    }

    #[test]
    fn offset_resets_to_minus5_after_rare() {
        // Force a rare: offset >= 97 makes effective_rare >= 100, guaranteeing rare.
        let mut offset = 97_i32;
        let mut rng = rng();
        let result = roll_rarity(RewardKind::Monster, &mut offset, 0, &mut rng);
        // effective_rare = 3 + 97 = 100, so any roll < 100 hits rare.
        assert_eq!(result, Rarity::Rare);
        assert_eq!(offset, INITIAL_RARE_OFFSET);
    }

    #[test]
    fn offset_increments_on_non_rare() {
        // Force a common: effective_rare = 3 + (-3) = 0, so rare impossible.
        let mut offset = -3_i32;
        let mut rng = rng();
        let result = roll_rarity(RewardKind::Monster, &mut offset, 0, &mut rng);
        // roll is in [0,100); effective_rare=0 means nothing hits rare.
        // Uncommon threshold = 0 + 37 = 37.
        match result {
            Rarity::Rare => panic!("should not be rare with 0% effective chance"),
            _ => assert_eq!(offset, -3 + 1), // incremented by 1
        }
    }

    #[test]
    fn offset_caps_at_40() {
        let pool = ironclad::all_cards();
        let mut offset = 39_i32;
        let mut rng = StdRng::seed_from_u64(1);
        // Run many offers to push offset past the cap.
        for _ in 0..30 {
            sample_offer(&pool, RewardKind::Monster, &mut offset, 0, &mut rng);
            assert!(offset <= MAX_RARE_OFFSET);
        }
    }

    #[test]
    fn first_reward_cannot_be_rare_for_monster() {
        // offset starts at -5, so effective rare = 3 + (-5) = -2 → clamped to 0%.
        let pool = ironclad::all_cards();
        let mut rng = StdRng::seed_from_u64(99);
        for _ in 0..100 {
            let mut offset = INITIAL_RARE_OFFSET;
            let offer = sample_offer(&pool, RewardKind::Monster, &mut offset, 0, &mut rng);
            assert!(
                offer.iter().all(|c| c.rarity != Rarity::Rare),
                "first monster offer should never contain a rare (offset starts at -5)"
            );
        }
    }

    #[test]
    fn elite_has_higher_rare_chance_than_monster() {
        let pool = ironclad::all_cards();
        let mut rng = StdRng::seed_from_u64(7);
        let n = 500;

        let mut elite_rares = 0u32;
        let mut monster_rares = 0u32;

        for _ in 0..n {
            let mut offset = 10_i32; // same starting offset for both
            let elite_offer = sample_offer(&pool, RewardKind::Elite, &mut offset, 0, &mut rng);
            let mut offset2 = 10_i32;
            let monster_offer = sample_offer(&pool, RewardKind::Monster, &mut offset2, 0, &mut rng);
            elite_rares   += elite_offer.iter().filter(|c| c.rarity == Rarity::Rare).count() as u32;
            monster_rares += monster_offer.iter().filter(|c| c.rarity == Rarity::Rare).count() as u32;
        }

        assert!(
            elite_rares > monster_rares,
            "elite ({elite_rares}) should produce more rares than monster ({monster_rares})"
        );
    }
}
