use crate::domain::card::{Card, CardType};
use crate::domain::run::RunState;

const HEAL_THRESHOLD: f32 = 0.60;

#[derive(Debug, Clone, PartialEq)]
pub enum RestAction {
    Heal,
    /// Upgrade the card at the given deck index; index resolved in SmithPick sub-mode.
    Smith(usize),
    /// SHOVEL: dig for a random relic.
    Dig,
    /// GIRYA: gain +1 permanent Strength (up to 3 uses).
    Lift,
    /// DREAM_CATCHER: after this rest, add a card to the deck.
    Dream,
}

#[derive(Debug, Clone)]
pub struct RestAdvice {
    pub action: RestAction,
    pub reason: String,
}

/// HP healed by the Heal option (30% max HP + REGAL_PILLOW +15 if held).
pub fn heal_amount(run: &RunState) -> u32 {
    let base = ((run.max_hp as f32) * 0.30).floor() as u32;
    if run.has_relic("REGAL_PILLOW") { base + 15 } else { base }
}

/// HP healed passively when entering the rest site (ETERNAL_FEATHER).
pub fn eternal_feather_heal(run: &RunState) -> u32 {
    if run.has_relic("ETERNAL_FEATHER") {
        (run.deck.len() as u32 / 5) * 3
    } else {
        0
    }
}

/// All rest options available at this rest site, in display order.
/// The Dream option is included here only when MINIATURE_TENT is absent
/// (with Tent, Dream is auto-applied after all other options).
pub fn available_rest_options(run: &RunState) -> Vec<RestAction> {
    let mut opts = vec![RestAction::Heal];
    if !smith_candidates(&run.deck).is_empty() {
        opts.push(RestAction::Smith(0)); // exact idx resolved in SmithPick
    }
    if run.has_relic("SHOVEL") {
        opts.push(RestAction::Dig);
    }
    if run.has_relic("GIRYA") && run.girya_uses < 3 {
        opts.push(RestAction::Lift);
    }
    if run.has_relic("DREAM_CATCHER") && !run.has_relic("MINIATURE_TENT") {
        opts.push(RestAction::Dream);
    }
    opts
}

/// Recommend the best rest action given the current run state.
pub fn advise_rest(run: &RunState) -> RestAdvice {
    // Eternal Feather heals before the player acts
    let hp_on_entry = run.hp.saturating_add(eternal_feather_heal(run)).min(run.max_hp);
    let hp_ratio = hp_on_entry as f32 / run.max_hp as f32;
    let h_amount = heal_amount(run);

    // Miniature Tent: can do everything — lead with the most urgent action
    if run.has_relic("MINIATURE_TENT") {
        let first = if hp_ratio < HEAL_THRESHOLD {
            RestAction::Heal
        } else if !smith_candidates(&run.deck).is_empty() {
            RestAction::Smith(0)
        } else {
            RestAction::Heal
        };
        return RestAdvice {
            action: first,
            reason: "Miniature Tent — all options will be applied".to_string(),
        };
    }

    // Shovel: relic > HP when health is acceptable
    if run.has_relic("SHOVEL") && hp_ratio >= 0.50 {
        return RestAdvice {
            action: RestAction::Dig,
            reason: format!("HP fine ({:.0}%) — dig for a relic", hp_ratio * 100.0),
        };
    }

    // Low HP: heal first
    if hp_ratio < HEAL_THRESHOLD {
        return RestAdvice {
            action: RestAction::Heal,
            reason: format!(
                "HP {:.0}% — heal {} ({} → {})",
                hp_ratio * 100.0, h_amount,
                hp_on_entry, (hp_on_entry + h_amount).min(run.max_hp),
            ),
        };
    }

    // Girya: permanent Strength is very strong
    if run.has_relic("GIRYA") && run.girya_uses < 3 {
        return RestAdvice {
            action: RestAction::Lift,
            reason: format!("Girya: +1 Strength ({}/{} uses)", run.girya_uses + 1, 3),
        };
    }

    // Smith if something valuable to upgrade
    match best_smith_candidate(&run.deck) {
        Some((idx, card)) => RestAdvice {
            action: RestAction::Smith(idx),
            reason: format!("HP fine ({:.0}%) — upgrade {}", hp_ratio * 100.0, card.name),
        },
        None => RestAdvice {
            action: RestAction::Heal,
            reason: format!(
                "Nothing to upgrade — heal {} ({} → {})",
                h_amount, hp_on_entry, (hp_on_entry + h_amount).min(run.max_hp),
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
            PlayerClass::Ironclad, hp, max_hp,
            ironclad::starter_deck(), starting_relics::ironclad(),
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

    #[test]
    fn regal_pillow_adds_bonus() {
        let mut run = run_at_hp(80, 80);
        run.relics.push(crate::domain::run::Relic::new("REGAL_PILLOW", "Regal Pillow", ""));
        let base = ((80_f32) * 0.30).floor() as u32;
        assert_eq!(heal_amount(&run), base + 15);
    }

    #[test]
    fn eternal_feather_heal_scales_with_deck() {
        let run = run_at_hp(80, 80); // starter deck = 10 cards
        // Without relic
        assert_eq!(eternal_feather_heal(&run), 0);
        // With relic
        let mut run2 = run_at_hp(80, 80);
        run2.relics.push(crate::domain::run::Relic::new("ETERNAL_FEATHER", "Eternal Feather", ""));
        assert_eq!(eternal_feather_heal(&run2), 6); // 10/5 * 3 = 6
    }

    #[test]
    fn shovel_recommended_at_sufficient_hp() {
        let mut run = run_at_hp(60, 80);
        run.relics.push(crate::domain::run::Relic::new("SHOVEL", "Shovel", ""));
        let advice = advise_rest(&run);
        assert_eq!(advice.action, RestAction::Dig);
    }

    #[test]
    fn girya_recommended_when_hp_fine() {
        let mut run = run_at_hp(70, 80);
        run.relics.push(crate::domain::run::Relic::new("GIRYA", "Girya", ""));
        let advice = advise_rest(&run);
        assert_eq!(advice.action, RestAction::Lift);
    }

    #[test]
    fn available_options_includes_relic_based_actions() {
        let mut run = run_at_hp(80, 80);
        run.relics.push(crate::domain::run::Relic::new("SHOVEL", "Shovel", ""));
        run.relics.push(crate::domain::run::Relic::new("GIRYA", "Girya", ""));
        let opts = available_rest_options(&run);
        assert!(opts.contains(&RestAction::Dig));
        assert!(opts.contains(&RestAction::Lift));
    }
}
