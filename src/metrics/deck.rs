use crate::domain::card::{Card, CardType};
use crate::domain::effect::{BuffType, CardEffect};

// ── Synergy axis detection ─────────────────────────────────────────────────

/// Count of cards sharing each synergy axis that has ≥2 cards.
/// Axes: damage amplification (Vulnerable/Weak application),
///       block scaling, draw engine, Strength ramp.
pub fn synergy_score(deck: &[Card]) -> u32 {
    let mut damage_amp: u32 = 0;
    let mut block_present: u32 = 0;
    let mut draw_engine: u32 = 0;
    let mut strength_ramp: u32 = 0;

    for card in deck {
        let mut counted_amp = false;
        let mut counted_str = false;

        for effect in &card.effects {
            match effect {
                CardEffect::ApplyToEnemy {
                    buff: BuffType::Vulnerable | BuffType::Weak,
                    ..
                }
                | CardEffect::ApplyToAllEnemies {
                    buff: BuffType::Vulnerable | BuffType::Weak,
                    ..
                } if !counted_amp => {
                    damage_amp += 1;
                    counted_amp = true;
                }
                CardEffect::Block(_) => {
                    block_present += 1;
                    break;
                }
                CardEffect::Draw(_) => {
                    draw_engine += 1;
                    break;
                }
                CardEffect::ApplyToSelf {
                    buff: BuffType::Strength,
                    ..
                } if !counted_str => {
                    strength_ramp += 1;
                    counted_str = true;
                }
                _ => {}
            }
        }
    }

    [damage_amp, block_present, draw_engine, strength_ramp]
        .iter()
        .filter(|&&n| n >= 2)
        .sum()
}

// ── Deck statistics ────────────────────────────────────────────────────────

/// Average energy cost of playable cards (cost 255 = unplayable, excluded).
pub fn mean_cost(deck: &[Card]) -> f32 {
    let playable: Vec<_> = deck.iter().filter(|c| c.cost != 255).collect();
    if playable.is_empty() {
        return 0.0;
    }
    playable.iter().map(|c| c.cost as f32).sum::<f32>() / playable.len() as f32
}

/// Fraction of cards that are attacks.
pub fn attack_ratio(deck: &[Card]) -> f32 {
    if deck.is_empty() {
        return 0.0;
    }
    deck.iter().filter(|c| c.card_type == CardType::Attack).count() as f32 / deck.len() as f32
}

/// True if at least `n` cards in the deck provide block.
pub fn has_block_density(deck: &[Card], n: usize) -> bool {
    deck.iter().filter(|c| c.base_block() > 0).count() >= n
}

/// Heuristic deck quality score, 0.0–1.0.
///
/// Act 1 weights block density and low cost more heavily.
/// Later acts weight synergy density more.
pub fn deck_score(deck: &[Card], act: u8) -> f32 {
    if deck.is_empty() {
        return 0.0;
    }

    let block_score = if has_block_density(deck, 3) { 0.3 } else { 0.1 };
    let synergy = (synergy_score(deck) as f32 / 10.0).min(0.4);
    let cost_score = (1.0 - mean_cost(deck) / 3.0).clamp(0.0, 1.0) * 0.2;
    let attack_s = if attack_ratio(deck) >= 0.3 { 0.1 } else { 0.0 };

    let raw = match act {
        1 => block_score * 1.5 + cost_score * 1.5 + synergy * 0.5 + attack_s,
        _ => synergy * 1.5 + block_score + cost_score + attack_s,
    };

    raw.min(1.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::card::{Card, CardType, Rarity};
    use crate::domain::effect::{BuffType, CardEffect};

    fn attack(damage: u32) -> Card {
        Card::new(
            0,
            "Strike",
            1,
            CardType::Attack,
            Rarity::Basic,
            vec![CardEffect::Damage(damage)],
        )
    }

    fn blocker() -> Card {
        Card::new(
            0,
            "Defend",
            1,
            CardType::Skill,
            Rarity::Basic,
            vec![CardEffect::Block(5)],
        )
    }

    fn vulnerable_applier() -> Card {
        Card::new(
            0,
            "Bash",
            2,
            CardType::Attack,
            Rarity::Basic,
            vec![
                CardEffect::Damage(8),
                CardEffect::ApplyToEnemy {
                    buff: BuffType::Vulnerable,
                    stacks: 2,
                },
            ],
        )
    }

    fn drawer() -> Card {
        Card::new(
            0,
            "Pommel Strike",
            1,
            CardType::Attack,
            Rarity::Common,
            vec![CardEffect::Damage(9), CardEffect::Draw(1)],
        )
    }

    #[test]
    fn synergy_empty_deck() {
        assert_eq!(synergy_score(&[]), 0);
    }

    #[test]
    fn synergy_no_overlap() {
        let deck = vec![attack(6), blocker()];
        // Only 1 of each axis — no axis meets the ≥2 threshold
        assert_eq!(synergy_score(&deck), 0);
    }

    #[test]
    fn synergy_two_blockers() {
        let deck = vec![blocker(), blocker(), attack(6)];
        // block_present=2 → counts as 2
        assert_eq!(synergy_score(&deck), 2);
    }

    #[test]
    fn synergy_damage_amp_axis() {
        let deck = vec![vulnerable_applier(), vulnerable_applier(), attack(6)];
        // damage_amp=2 → counts
        assert_eq!(synergy_score(&deck), 2);
    }

    #[test]
    fn synergy_draw_engine() {
        let deck = vec![drawer(), drawer(), attack(6)];
        assert_eq!(synergy_score(&deck), 2);
    }

    #[test]
    fn mean_cost_basic() {
        let deck = vec![attack(6), blocker()]; // both cost 1
        assert!((mean_cost(&deck) - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn mean_cost_excludes_unplayable() {
        let unplayable = Card::new(
            0,
            "Wound",
            255,
            CardType::Status,
            Rarity::Special,
            vec![],
        );
        let deck = vec![attack(6), unplayable]; // only Strike (cost 1) counts
        assert!((mean_cost(&deck) - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn attack_ratio_all_attacks() {
        let deck = vec![attack(6), attack(8)];
        assert!((attack_ratio(&deck) - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn has_block_density_true() {
        let deck = vec![blocker(), blocker(), blocker(), attack(6)];
        assert!(has_block_density(&deck, 3));
    }

    #[test]
    fn has_block_density_false() {
        let deck = vec![blocker(), blocker(), attack(6)];
        assert!(!has_block_density(&deck, 3));
    }

    #[test]
    fn deck_score_not_zero_for_normal_deck() {
        let deck = vec![attack(6), attack(6), blocker(), blocker(), blocker()];
        let score = deck_score(&deck, 1);
        assert!(score > 0.0 && score <= 1.0);
    }

    #[test]
    fn deck_score_empty_is_zero() {
        assert_eq!(deck_score(&[], 1), 0.0);
    }
}
