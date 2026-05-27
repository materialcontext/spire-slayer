use crate::domain::card::{Card, Rarity};
use crate::domain::run::RunState;
use super::deck::synergy_score;

#[derive(Debug, Clone)]
pub struct CardAdvice {
    /// Index into the offered slice.
    pub card_index: usize,
    /// Higher is better.
    pub score: f32,
    /// Human-readable one-liner explaining the top scoring factor.
    pub reason: String,
}

/// Score a single card against an existing deck.
pub fn score_single(card: &Card, deck: &[Card], _act: u8) -> f32 {
    // Rarity bonus
    let rarity_weight: f32 = match card.rarity {
        Rarity::Ancient | Rarity::Rare => 0.30,
        Rarity::Uncommon => 0.20,
        Rarity::Common => 0.10,
        Rarity::Basic => 0.05,
        Rarity::Special => 0.0,
    };

    // Synergy delta: how much adding this card raises synergy score
    let base_synergy = synergy_score(deck) as i32;
    let mut extended: Vec<Card> = Vec::with_capacity(deck.len() + 1);
    extended.extend_from_slice(deck);
    extended.push(card.clone());
    let new_synergy = synergy_score(&extended) as i32;
    let synergy_delta = ((new_synergy - base_synergy).max(0) as f32) * 0.10;

    // Cost efficiency: (damage + block) per energy
    let cost = card.cost.clamp(1, 254) as f32; // treat 0-cost as 1 for ratio
    let output = (card.base_damage() + card.base_block()) as f32;
    let efficiency = if output > 0.0 {
        (output / cost / 20.0).min(0.30)
    } else {
        // Power or passive — flat heuristic
        0.10
    };

    // Dilution penalty: adding to a larger deck hurts draw consistency
    let dilution_bonus = (1.0 / (deck.len() as f32 + 1.0)).min(0.15);

    rarity_weight + synergy_delta + efficiency + dilution_bonus
}

/// Rank the offered cards best-first against the current run state.
pub fn pick_score(offered: &[Card], run: &RunState) -> Vec<CardAdvice> {
    let deck = &run.deck;
    let act = run.act;

    let mut advice: Vec<CardAdvice> = offered
        .iter()
        .enumerate()
        .map(|(i, card)| {
            let score = score_single(card, deck, act);
            let reason = build_reason(card, deck);
            CardAdvice { card_index: i, score, reason }
        })
        .collect();

    advice.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
    advice
}

fn build_reason(card: &Card, deck: &[Card]) -> String {
    let dmg = card.base_damage();
    let blk = card.base_block();

    // Synergy delta
    let mut extended = deck.to_vec();
    extended.push(card.clone());
    let delta = synergy_score(&extended) as i32 - synergy_score(deck) as i32;

    if delta > 0 {
        return format!("adds to {} synergy axis", delta);
    }

    if dmg > 0 && card.cost != 255 {
        let per_e = dmg / card.cost.max(1) as u32;
        return format!("{} dmg at {} energy ({}/e)", dmg, card.cost, per_e);
    }

    if blk > 0 {
        return format!("{} block", blk);
    }

    "passive effect".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::card::{Card, CardType, Rarity};
    use crate::domain::combat::{PlayerState};
    use crate::domain::effect::CardEffect;
    use crate::domain::run::{RunState, PlayerClass, starting_relics};

    fn strike() -> Card {
        Card::new(1, "Strike", 1, CardType::Attack, Rarity::Basic, vec![CardEffect::Damage(6)])
    }

    fn rare_card() -> Card {
        Card::new(
            99,
            "Demon Form",
            3,
            CardType::Power,
            Rarity::Rare,
            vec![CardEffect::Passive("At the start of each turn, gain 2 Strength.".into())],
        )
    }

    fn empty_run() -> RunState {
        RunState::new(
            PlayerClass::Ironclad,
            80,
            80,
            vec![],
            starting_relics::ironclad(),
        )
    }

    #[test]
    fn rare_scores_higher_than_basic() {
        let run = empty_run();
        let advice = pick_score(&[strike(), rare_card()], &run);
        // Rare should rank first (index 1 in original, card_index=1)
        assert_eq!(advice[0].card_index, 1, "Rare should rank higher");
    }

    #[test]
    fn all_cards_get_a_reason() {
        let run = empty_run();
        let offered = vec![strike(), rare_card()];
        let advice = pick_score(&offered, &run);
        for a in &advice {
            assert!(!a.reason.is_empty());
        }
    }

    #[test]
    fn results_sorted_descending() {
        let run = empty_run();
        let advice = pick_score(&[strike(), strike(), rare_card()], &run);
        for w in advice.windows(2) {
            assert!(w[0].score >= w[1].score);
        }
    }

    #[test]
    fn score_single_positive() {
        let score = score_single(&strike(), &[], 1);
        assert!(score > 0.0);
    }
}
