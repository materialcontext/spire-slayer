use crate::domain::card::{Card, CardType, Rarity};
use crate::domain::effect::{BuffType, CardEffect};

mod ids {
    // Basic / Starter
    pub const STRIKE: u32 = 1;
    pub const DEFEND: u32 = 2;
    pub const BASH: u32 = 3;
    // Common Attacks
    pub const ANGER: u32 = 10;
    pub const CLEAVE: u32 = 11;
    pub const CLOTHESLINE: u32 = 12;
    pub const HEAVY_BLADE: u32 = 13;
    pub const POMMEL_STRIKE: u32 = 14;
    pub const THUNDERCLAP: u32 = 15;
    pub const TWIN_STRIKE: u32 = 16;
    pub const WILD_STRIKE: u32 = 17;
    // Common Skills
    pub const ARMAMENTS: u32 = 20;
    pub const FLEX: u32 = 21;
    pub const HAVOC: u32 = 22;
    pub const SHRUG_IT_OFF: u32 = 23;
    pub const TRUE_GRIT: u32 = 24;
    // Common Powers
    pub const INFLAME: u32 = 30;
    pub const METALLICIZE: u32 = 31;
    // Status cards
    pub const WOUND: u32 = 200;
    pub const BURN: u32 = 201;
    pub const DAZED: u32 = 202;
}

/// Ironclad card constructors. Each function returns a fresh `Card` instance.
pub mod ironclad {
    use super::*;

    // ── Basic ─────────────────────────────────────────────────────────────

    pub fn strike() -> Card {
        Card::new(ids::STRIKE, "Strike", 1, CardType::Attack, Rarity::Basic,
            vec![CardEffect::Damage(6)])
    }

    pub fn defend() -> Card {
        Card::new(ids::DEFEND, "Defend", 1, CardType::Skill, Rarity::Basic,
            vec![CardEffect::Block(5)])
    }

    pub fn bash() -> Card {
        Card::new(ids::BASH, "Bash", 2, CardType::Attack, Rarity::Basic,
            vec![
                CardEffect::Damage(8),
                CardEffect::ApplyToEnemy { buff: BuffType::Vulnerable, stacks: 2 },
            ])
    }

    // ── Common Attacks ────────────────────────────────────────────────────

    pub fn anger() -> Card {
        Card::new(ids::ANGER, "Anger", 0, CardType::Attack, Rarity::Common,
            vec![
                CardEffect::Damage(6),
                CardEffect::Passive("Add a copy of Anger to your discard pile.".into()),
            ])
    }

    pub fn cleave() -> Card {
        Card::new(ids::CLEAVE, "Cleave", 1, CardType::Attack, Rarity::Common,
            vec![CardEffect::DamageAll(8)])
    }

    pub fn clothesline() -> Card {
        Card::new(ids::CLOTHESLINE, "Clothesline", 2, CardType::Attack, Rarity::Common,
            vec![
                CardEffect::Damage(12),
                CardEffect::ApplyToEnemy { buff: BuffType::Weak, stacks: 2 },
            ])
    }

    pub fn heavy_blade() -> Card {
        Card::new(ids::HEAVY_BLADE, "Heavy Blade", 2, CardType::Attack, Rarity::Common,
            vec![CardEffect::Damage(14)])
    }

    pub fn pommel_strike() -> Card {
        Card::new(ids::POMMEL_STRIKE, "Pommel Strike", 1, CardType::Attack, Rarity::Common,
            vec![
                CardEffect::Damage(9),
                CardEffect::Draw(1),
            ])
    }

    pub fn thunderclap() -> Card {
        Card::new(ids::THUNDERCLAP, "Thunderclap", 1, CardType::Attack, Rarity::Common,
            vec![
                CardEffect::DamageAll(4),
                CardEffect::ApplyToAllEnemies { buff: BuffType::Vulnerable, stacks: 1 },
            ])
    }

    pub fn twin_strike() -> Card {
        Card::new(ids::TWIN_STRIKE, "Twin Strike", 1, CardType::Attack, Rarity::Common,
            vec![CardEffect::DamageMulti { base: 5, hits: 2 }])
    }

    pub fn wild_strike() -> Card {
        Card::new(ids::WILD_STRIKE, "Wild Strike", 1, CardType::Attack, Rarity::Common,
            vec![
                CardEffect::Damage(12),
                CardEffect::Passive("Shuffle a Wound into your draw pile.".into()),
            ])
    }

    // ── Common Skills ─────────────────────────────────────────────────────

    pub fn armaments() -> Card {
        Card::new(ids::ARMAMENTS, "Armaments", 1, CardType::Skill, Rarity::Common,
            vec![
                CardEffect::Block(5),
                CardEffect::Passive("Upgrade a card in your hand for the rest of combat.".into()),
            ])
    }

    pub fn flex() -> Card {
        Card::new(ids::FLEX, "Flex", 0, CardType::Skill, Rarity::Common,
            vec![
                CardEffect::ApplyToSelf { buff: BuffType::Strength, stacks: 2 },
                CardEffect::Passive("At the end of your turn, lose 2 Strength.".into()),
            ])
    }

    pub fn havoc() -> Card {
        Card::new(ids::HAVOC, "Havoc", 1, CardType::Skill, Rarity::Common,
            vec![CardEffect::Passive("Play the top card of your draw pile and Exhaust it.".into())])
    }

    pub fn shrug_it_off() -> Card {
        Card::new(ids::SHRUG_IT_OFF, "Shrug It Off", 1, CardType::Skill, Rarity::Common,
            vec![
                CardEffect::Block(8),
                CardEffect::Draw(1),
            ])
    }

    pub fn true_grit() -> Card {
        Card::new(ids::TRUE_GRIT, "True Grit", 1, CardType::Skill, Rarity::Common,
            vec![
                CardEffect::Block(7),
                CardEffect::Passive("Exhaust a random card in your hand.".into()),
            ])
    }

    // ── Common Powers ─────────────────────────────────────────────────────

    pub fn inflame() -> Card {
        Card::new(ids::INFLAME, "Inflame", 1, CardType::Power, Rarity::Common,
            vec![CardEffect::ApplyToSelf { buff: BuffType::Strength, stacks: 2 }])
    }

    pub fn metallicize() -> Card {
        Card::new(ids::METALLICIZE, "Metallicize", 1, CardType::Power, Rarity::Common,
            vec![CardEffect::Passive("At the end of your turn, gain 3 Block.".into())])
    }

    // ── Deck constructors ─────────────────────────────────────────────────

    /// Ironclad's starting deck: 5× Strike, 4× Defend, 1× Bash.
    pub fn starter_deck() -> Vec<Card> {
        let mut deck = Vec::with_capacity(10);
        for _ in 0..5 { deck.push(strike()); }
        for _ in 0..4 { deck.push(defend()); }
        deck.push(bash());
        deck
    }
}

/// Status cards that can be shuffled into a player's deck.
pub mod status {
    use super::*;

    pub fn wound() -> Card {
        Card::new(ids::WOUND, "Wound", 255, CardType::Status, Rarity::Special, vec![])
    }

    pub fn burn() -> Card {
        Card::new(ids::BURN, "Burn", 255, CardType::Status, Rarity::Special,
            vec![CardEffect::Passive("At the end of your turn, take 2 damage.".into())])
    }

    pub fn dazed() -> Card {
        Card::new(ids::DAZED, "Dazed", 255, CardType::Status, Rarity::Special, vec![])
            .with_ethereal()
    }
}

#[cfg(test)]
mod tests {
    use super::ironclad;
    use crate::domain::card::CardType;
    use crate::domain::effect::{BuffType, CardEffect};

    #[test]
    fn starter_deck_has_ten_cards() {
        assert_eq!(ironclad::starter_deck().len(), 10);
    }

    #[test]
    fn starter_deck_composition() {
        let deck = ironclad::starter_deck();
        let strikes = deck.iter().filter(|c| c.id == super::ids::STRIKE).count();
        let defends = deck.iter().filter(|c| c.id == super::ids::DEFEND).count();
        let bashes = deck.iter().filter(|c| c.id == super::ids::BASH).count();
        assert_eq!(strikes, 5);
        assert_eq!(defends, 4);
        assert_eq!(bashes, 1);
    }

    #[test]
    fn strike_stats() {
        let s = ironclad::strike();
        assert_eq!(s.cost, 1);
        assert_eq!(s.card_type, CardType::Attack);
        assert_eq!(s.base_damage(), 6);
        assert_eq!(s.base_block(), 0);
    }

    #[test]
    fn bash_deals_damage_and_applies_vulnerable() {
        let b = ironclad::bash();
        assert_eq!(b.base_damage(), 8);
        assert!(b.effects.iter().any(|e| matches!(
            e,
            CardEffect::ApplyToEnemy { buff: BuffType::Vulnerable, stacks: 2 }
        )));
    }

    #[test]
    fn twin_strike_deals_ten_total_damage() {
        assert_eq!(ironclad::twin_strike().base_damage(), 10);
    }

    #[test]
    fn defend_only_provides_block() {
        let d = ironclad::defend();
        assert_eq!(d.base_block(), 5);
        assert_eq!(d.base_damage(), 0);
    }

    #[test]
    fn shrug_it_off_gives_block_and_draw() {
        let s = ironclad::shrug_it_off();
        assert_eq!(s.base_block(), 8);
        assert!(s.effects.iter().any(|e| matches!(e, CardEffect::Draw(1))));
    }

    #[test]
    fn wound_is_not_playable() {
        use super::status;
        let w = status::wound();
        assert!(!w.is_playable(3));
    }
}
