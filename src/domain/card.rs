use serde::{Deserialize, Serialize};
use crate::domain::effect::CardEffect;

pub type CardId = u32;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum CardType {
    Attack,
    Skill,
    Power,
    Status,
    Curse,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Rarity {
    Basic,
    Common,
    Uncommon,
    Rare,
    /// Relic-only rarity; obtainable only from the ancient event at the start of each act.
    /// Never appears on cards or in card reward screens.
    Ancient,
    /// Catch-all for Status, Curse, Token, Event, Quest rarities.
    Special,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Card {
    pub id: CardId,
    pub name: String,
    /// Energy cost; 255 = unplayable / X cost.
    pub cost: u8,
    pub card_type: CardType,
    pub rarity: Rarity,
    pub upgraded: bool,
    pub effects: Vec<CardEffect>,
    pub exhausts: bool,
    pub ethereal: bool,
    pub innate: bool,
}

impl Card {
    pub fn new(
        id: CardId,
        name: impl Into<String>,
        cost: u8,
        card_type: CardType,
        rarity: Rarity,
        effects: Vec<CardEffect>,
    ) -> Self {
        Self {
            id,
            name: name.into(),
            cost,
            card_type,
            rarity,
            upgraded: false,
            effects,
            exhausts: false,
            ethereal: false,
            innate: false,
        }
    }

    pub fn with_exhausts(mut self) -> Self {
        self.exhausts = true;
        self
    }

    pub fn with_ethereal(mut self) -> Self {
        self.ethereal = true;
        self
    }

    pub fn with_innate(mut self) -> Self {
        self.innate = true;
        self
    }

    pub fn upgrade(mut self) -> Self {
        self.upgraded = true;
        self
    }

    pub fn is_playable(&self, energy: u8) -> bool {
        !matches!(self.card_type, CardType::Status | CardType::Curse)
            && self.cost != 255
            && self.cost <= energy
    }

    /// Sum of direct damage effects (ignoring buffs/debuffs).
    pub fn base_damage(&self) -> u32 {
        self.effects.iter().map(|e| e.damage_value()).sum()
    }

    /// Sum of block effects.
    pub fn base_block(&self) -> u32 {
        self.effects.iter().map(|e| e.block_value()).sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::effect::CardEffect;

    fn make_strike() -> Card {
        Card::new(1, "Strike", 1, CardType::Attack, Rarity::Basic, vec![CardEffect::Damage(6)])
    }

    #[test]
    fn strike_is_playable_with_enough_energy() {
        let c = make_strike();
        assert!(c.is_playable(1));
        assert!(c.is_playable(3));
        assert!(!c.is_playable(0));
    }

    #[test]
    fn status_cards_are_never_playable() {
        let wound = Card::new(100, "Wound", 0, CardType::Status, Rarity::Special, vec![]);
        assert!(!wound.is_playable(3));
    }

    #[test]
    fn base_damage_sums_effects() {
        let twin = Card::new(
            2, "Twin Strike", 1, CardType::Attack, Rarity::Common,
            vec![CardEffect::DamageMulti { base: 5, hits: 2 }],
        );
        assert_eq!(twin.base_damage(), 10);
    }

    #[test]
    fn serialization_round_trip() {
        let c = make_strike();
        let json = serde_json::to_string(&c).unwrap();
        let back: Card = serde_json::from_str(&json).unwrap();
        assert_eq!(c, back);
    }
}
