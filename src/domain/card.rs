use serde::{Deserialize, Serialize};
use crate::domain::effect::{BuffType, CardEffect};

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

/// Per-card enchantment state, carried with the card through the sim.
///
/// Static enchantments (SHARP, INSTINCT, NIMBLE, ADROIT, INKY, CORRUPTED,
/// ROYALLY_APPROVED, STEADY, SOULS_POWER, TEZCATARAS_EMBER, CLONE) are applied
/// once via `apply_enchantment` and modify Card fields or effects directly; they
/// are not stored here.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Enchantment {
    /// GLAM / SPIRAL: after the first play this combat, a copy goes to hand.
    Replay { used: bool },
    /// MOMENTUM: each play permanently boosts this card's attack damage by `per_play`.
    Momentum { per_play: u32 },
    /// SLITHER: when drawn, randomize cost 0–3.
    Slither,
    /// SLUMBERING_ESSENCE: while in hand at end of turn, cost drops by 1; resets on play.
    SlumberingEssence { discount: u8 },
    /// SOWN: first play this combat, gain `energy` energy.
    Sown { energy: u32, used: bool },
    /// SWIFT: first play this combat, draw `cards` cards.
    Swift { cards: u32, used: bool },
    /// VIGOROUS: first play this combat, deal `bonus` additional damage.
    Vigorous { bonus: u32, used: bool },
    /// IMBUED: auto-played at combat start (no per-play state needed).
    Imbued,
    /// PERFECT_FIT: when shuffled into draw pile, placed on top.
    PerfectFit,
    /// GOOPY: gains Exhaust; each play permanently raises this card's Block by 1.
    /// `cumulative` is the total bonus accumulated across plays (for display).
    Goopy { cumulative: u32 },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Card {
    pub id: CardId,
    pub name: String,
    /// Energy cost; 255 = unplayable / X cost.
    pub cost: u8,
    pub card_type: CardType,
    pub rarity: Rarity,
    pub effects: Vec<CardEffect>,
    pub exhausts: bool,
    pub ethereal: bool,
    pub innate: bool,
    /// Card stays in hand at end of turn instead of being discarded.
    pub retain: bool,
    /// Damage is dealt by Osty (the companion), not the player.
    /// Player's Strength and Weak debuff do not apply; enemy Vulnerable still does.
    pub osty_attack: bool,
    /// Star cost (Regent's secondary resource). 0 means no star cost.
    pub star_cost: u8,
    /// Whether this card has been upgraded at a rest site.
    pub upgraded: bool,
    /// TEZCATARAS_EMBER: card returns to hand after being played instead of going to discard.
    #[serde(default)]
    pub eternal: bool,
    /// Dynamic enchantments that carry per-combat runtime state.
    #[serde(default)]
    pub enchantments: Vec<Enchantment>,
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
            effects,
            exhausts: false,
            ethereal: false,
            innate: false,
            retain: false,
            osty_attack: false,
            star_cost: 0,
            upgraded: false,
            eternal: false,
            enchantments: Vec::new(),
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

    pub fn with_retain(mut self) -> Self {
        self.retain = true;
        self
    }

    pub fn with_osty_attack(mut self) -> Self {
        self.osty_attack = true;
        self
    }

    pub fn with_eternal(mut self) -> Self {
        self.eternal = true;
        self
    }

    /// Apply an enchantment to this card.
    ///
    /// Static enchantments are folded directly into card fields or `effects`.
    /// Dynamic enchantments are pushed onto `self.enchantments` for runtime
    /// processing in the sim.
    ///
    /// `amount` carries the numeric parameter for variable enchantments
    /// (e.g. SHARP+3, MOMENTUM+2, SOWN+1 energy). Pass 0 for enchantments
    /// that have no numeric parameter (STEADY, ROYALLY_APPROVED, etc.).
    pub fn apply_enchantment(&mut self, id: &str, amount: u32) {
        match id {
            // ── Static: modify card fields/effects immediately ───────────
            "SHARP" => {
                for eff in &mut self.effects {
                    match eff {
                        CardEffect::Damage(d) | CardEffect::DamageAll(d) => *d += amount,
                        CardEffect::DamageMulti { base, .. } => *base += amount,
                        _ => {}
                    }
                }
            }
            "INSTINCT" => {
                for eff in &mut self.effects {
                    match eff {
                        CardEffect::Damage(d) | CardEffect::DamageAll(d) => *d *= 2,
                        CardEffect::DamageMulti { base, .. } => *base *= 2,
                        _ => {}
                    }
                }
            }
            "ADROIT" => {
                self.effects.push(CardEffect::Block(amount));
            }
            "NIMBLE" => {
                for eff in &mut self.effects {
                    if let CardEffect::Block(b) = eff {
                        *b += amount;
                    }
                }
            }
            "INKY" => {
                for eff in &mut self.effects {
                    match eff {
                        CardEffect::Damage(d) | CardEffect::DamageAll(d) => *d += 2,
                        CardEffect::DamageMulti { base, .. } => *base += 2,
                        _ => {}
                    }
                }
                self.effects.push(CardEffect::ApplyToAllEnemies {
                    buff: BuffType::Weak,
                    stacks: 1,
                });
            }
            "CORRUPTED" => {
                for eff in &mut self.effects {
                    match eff {
                        CardEffect::Damage(d) | CardEffect::DamageAll(d) => *d = *d * 3 / 2,
                        CardEffect::DamageMulti { base, .. } => *base = *base * 3 / 2,
                        _ => {}
                    }
                }
                self.effects.push(CardEffect::LoseHp(2));
            }
            "ROYALLY_APPROVED" => {
                self.innate = true;
                self.retain = true;
            }
            "STEADY" => {
                self.retain = true;
            }
            "SOULS_POWER" => {
                self.exhausts = false;
            }
            "TEZCATARAS_EMBER" => {
                self.cost = 0;
                self.eternal = true;
            }
            "CLONE" => {} // Rest-site duplication; no sim effect.
            // ── Dynamic: stored in enchantments for runtime processing ───
            "GLAM" | "SPIRAL" => {
                self.enchantments.push(Enchantment::Replay { used: false });
            }
            "MOMENTUM" => {
                self.enchantments.push(Enchantment::Momentum { per_play: amount.max(1) });
            }
            "SLITHER" => {
                self.enchantments.push(Enchantment::Slither);
            }
            "SLUMBERING_ESSENCE" => {
                self.enchantments.push(Enchantment::SlumberingEssence { discount: 0 });
            }
            "SOWN" => {
                self.enchantments.push(Enchantment::Sown { energy: amount.max(1), used: false });
            }
            "SWIFT" => {
                self.enchantments.push(Enchantment::Swift { cards: amount.max(1), used: false });
            }
            "VIGOROUS" => {
                self.enchantments.push(Enchantment::Vigorous { bonus: amount.max(1), used: false });
            }
            "IMBUED" => {
                self.enchantments.push(Enchantment::Imbued);
            }
            "PERFECT_FIT" => {
                self.enchantments.push(Enchantment::PerfectFit);
            }
            "GOOPY" => {
                self.exhausts = true;
                self.enchantments.push(Enchantment::Goopy { cumulative: 0 });
            }
            _ => {}
        }
    }

    /// Reset per-combat enchantment state (call when initialising a new combat).
    ///
    /// This does NOT reset Goopy's cumulative block bonus, which is intentionally
    /// persistent across combats (it represents permanent card upgrades).
    pub fn reset_combat_enchantments(&mut self) {
        for enc in &mut self.enchantments {
            match enc {
                Enchantment::Replay { used } => *used = false,
                Enchantment::Sown { used, .. } => *used = false,
                Enchantment::Swift { used, .. } => *used = false,
                Enchantment::Vigorous { used, .. } => *used = false,
                Enchantment::SlumberingEssence { discount } => *discount = 0,
                _ => {}
            }
        }
    }

    pub fn is_playable(&self, energy: u8) -> bool {
        if matches!(self.card_type, CardType::Status | CardType::Curse) {
            return false;
        }
        // X-cost (255): playable whenever there is at least 1 energy
        if self.cost == 255 {
            return energy > 0;
        }
        self.cost <= energy
    }

    /// Upgrade this card (rest site smith): boosts Damage/Block effects by 3.
    /// No-op if already upgraded.
    pub fn upgrade(&mut self) {
        if self.upgraded { return; }
        self.upgraded = true;
        for effect in &mut self.effects {
            match effect {
                CardEffect::Damage(d) | CardEffect::DamageAll(d) | CardEffect::Block(d) => {
                    *d += 3;
                }
                CardEffect::DamageMulti { base, .. } => {
                    *base += 1;
                }
                _ => {}
            }
        }
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
