use crate::domain::card::{Card, CardType};
use crate::domain::run::RunState;

const HEAL_THRESHOLD: f32 = 0.60;

#[derive(Debug, Clone)]
pub enum RestAction {
    Heal,
    /// Upgrade the card at the given deck index.
    Smith(usize),
}

#[derive(Debug, Clone)]
pub struct RestAdvice {
    pub action: RestAction,
    pub reason: String,
}

/// Recommend Heal or Smith given the current run state.
pub fn advise_rest(run: &RunState) -> RestAdvice {
    let hp_ratio = run.hp as f32 / run.max_hp as f32;
    let heal_amount = ((run.max_hp as f32) * 0.30).floor() as u32;

    if hp_ratio < HEAL_THRESHOLD {
        return RestAdvice {
            action: RestAction::Heal,
            reason: format!(
                "HP at {:.0}% — heal {} ({} → {})",
                hp_ratio * 100.0,
                heal_amount,
                run.hp,
                (run.hp + heal_amount).min(run.max_hp),
            ),
        };
    }

    match best_smith_candidate(&run.deck) {
        Some((idx, card)) => RestAdvice {
            action: RestAction::Smith(idx),
            reason: format!(
                "HP fine ({:.0}%) — upgrade {}",
                hp_ratio * 100.0,
                card.name,
            ),
        },
        None => RestAdvice {
            action: RestAction::Heal,
            reason: format!(
                "Nothing to upgrade — heal {} ({} → {})",
                heal_amount,
                run.hp,
                (run.hp + heal_amount).min(run.max_hp),
            ),
        },
    }
}

pub fn smith_candidates(deck: &[Card]) -> Vec<(usize, &Card)> {
    let mut candidates: Vec<(usize, &Card)> = deck.iter()
        .enumerate()
        .filter(|(_, c)| !c.upgraded && !matches!(c.card_type, CardType::Status | CardType::Curse))
        .collect();
    candidates.sort_by(|a, b| smith_priority(b.1).cmp(&smith_priority(a.1)));
    candidates
}

fn best_smith_candidate(deck: &[Card]) -> Option<(usize, &Card)> {
    deck.iter()
        .enumerate()
        .filter(|(_, c)| !matches!(c.card_type, CardType::Status | CardType::Curse))
        .max_by_key(|(_, c)| smith_priority(c))
}

pub fn smith_priority(card: &Card) -> u32 {
    let base = card.base_damage() + card.base_block();
    let type_bonus: u32 = match card.card_type {
        CardType::Attack => 100,
        CardType::Skill  => 50,
        CardType::Power  => 25,
        _ => 0,
    };
    type_bonus + base
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::run::{PlayerClass, RunState, starting_relics};
    use crate::domain::catalog::ironclad;

    fn run_at_hp(hp: u32, max_hp: u32) -> RunState {
        RunState::new(
            PlayerClass::Ironclad,
            hp,
            max_hp,
            ironclad::starter_deck(),
            starting_relics::ironclad(),
        )
    }

    #[test]
    fn low_hp_recommends_heal() {
        let run = run_at_hp(40, 80);
        let advice = advise_rest(&run);
        assert!(matches!(advice.action, RestAction::Heal));
        assert!(!advice.reason.is_empty());
    }

    #[test]
    fn high_hp_recommends_smith() {
        let run = run_at_hp(75, 80);
        let advice = advise_rest(&run);
        assert!(matches!(advice.action, RestAction::Smith(_)));
    }

    #[test]
    fn empty_deck_falls_back_to_heal() {
        let mut run = run_at_hp(80, 80);
        run.deck.clear();
        let advice = advise_rest(&run);
        assert!(matches!(advice.action, RestAction::Heal));
    }

    #[test]
    fn smith_prefers_attacks() {
        let run = run_at_hp(80, 80);
        let advice = advise_rest(&run);
        if let RestAction::Smith(idx) = advice.action {
            assert_eq!(run.deck[idx].card_type, crate::domain::card::CardType::Attack);
        }
    }
}
