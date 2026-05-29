//! Card catalog backed by the spire-codex.com API.
//!
//! All card data is fetched from `https://spire-codex.com/api/cards` and
//! cached at `~/.cache/spire-slayer/cards.json`. The cache is refreshed
//! automatically whenever it is more than one day old.

use crate::data::api::{self, SpireApiCard};
use crate::domain::card::{Card, CardId, CardType, Rarity};
use crate::domain::effect::{BuffType, CardEffect};

// ── Helpers ────────────────────────────────────────────────────────────────

/// Stable u32 card ID derived from the API string ID via djb2.
fn djb2(s: &str) -> CardId {
    s.bytes()
        .fold(5381u32, |h, b| h.wrapping_mul(33).wrapping_add(b as u32))
}

fn map_card_type(s: &str) -> CardType {
    match s {
        "Attack" => CardType::Attack,
        "Skill" => CardType::Skill,
        "Power" => CardType::Power,
        "Status" => CardType::Status,
        "Curse" => CardType::Curse,
        _ => CardType::Skill,
    }
}

fn map_rarity(s: &str) -> Rarity {
    match s {
        "Basic" => Rarity::Basic,
        "Common" => Rarity::Common,
        "Uncommon" => Rarity::Uncommon,
        "Rare" => Rarity::Rare,
        "Ancient" => Rarity::Ancient,
        _ => Rarity::Special,
    }
}

fn map_buff(power: &str) -> Option<BuffType> {
    match power {
        "Vulnerable" => Some(BuffType::Vulnerable),
        "Weak" => Some(BuffType::Weak),
        "Frail" => Some(BuffType::Frail),
        "Poison" => Some(BuffType::Poison),
        "Strength" => Some(BuffType::Strength),
        "Dexterity" => Some(BuffType::Dexterity),
        "Thorns" => Some(BuffType::Thorns),
        "Metallicize" | "Plating" => Some(BuffType::Plating),
        "Intangible" => Some(BuffType::Intangible),
        "Barricade" => Some(BuffType::Barricade),
        "NoxiousFumes" | "Noxious Fumes" => Some(BuffType::NoxiousFumes),
        _ => None,
    }
}

fn api_to_domain(api: &SpireApiCard) -> Card {
    let id = djb2(&api.id);
    let card_type = api
        .card_type
        .as_deref()
        .map(map_card_type)
        .unwrap_or(CardType::Skill);
    let rarity = api
        .rarity
        .as_deref()
        .map(map_rarity)
        .unwrap_or(Rarity::Special);

    let cost: u8 = if api.is_x_cost || api.is_x_star_cost {
        255
    } else {
        api.cost.map(|c| c.clamp(0, 254) as u8).unwrap_or(255)
    };

    let target = api.target.as_deref().unwrap_or("SingleEnemy");
    let hits = api.hit_count.unwrap_or(1).max(1) as u32;

    let mut effects: Vec<CardEffect> = Vec::new();

    if let Some(dmg) = api.damage.filter(|&d| d > 0) {
        let base = dmg as u32;
        effects.push(match target {
            "AllEnemies" if hits > 1 => CardEffect::DamageMulti { base, hits },
            "AllEnemies" => CardEffect::DamageAll(base),
            _ if hits > 1 => CardEffect::DamageMulti { base, hits },
            _ => CardEffect::Damage(base),
        });
    }

    if let Some(b) = api.block.filter(|&b| b > 0) {
        effects.push(CardEffect::Block(b as u32));
    }

    if let Some(d) = api.cards_draw.filter(|&d| d > 0) {
        effects.push(CardEffect::Draw(d as u32));
    }

    if let Some(e) = api.energy_gain.filter(|&e| e > 0) {
        effects.push(CardEffect::GainEnergy(e as u32));
    }

    if let Some(h) = api.hp_loss.filter(|&h| h > 0) {
        effects.push(CardEffect::LoseHp(h as u32));
    }

    for pw in &api.powers_applied {
        let stacks = pw.amount.unwrap_or(1);
        let effect = if let Some(buff) = map_buff(&pw.power) {
            match target {
                "Self" | "None" => CardEffect::ApplyToSelf { buff, stacks },
                "AllEnemies" => CardEffect::ApplyToAllEnemies { buff, stacks },
                _ => CardEffect::ApplyToEnemy { buff, stacks },
            }
        } else {
            CardEffect::Passive(format!("Apply {} {}.", stacks, pw.power))
        };
        effects.push(effect);
    }

    // Special-case cards whose API structured fields don't capture their primary mechanic.
    match api.id.as_str() {
        "BODY_SLAM" => {
            // Deal damage equal to current Block (api.damage is 0 and gets filtered out)
            effects.push(CardEffect::DamageEqBlock);
        }
        "NOXIOUS_FUMES" => {
            // At the start of each turn, apply 2 Poison to all enemies
            effects.push(CardEffect::ApplyToSelf {
                buff: BuffType::NoxiousFumes,
                stacks: 2,
            });
        }
        "BARRICADE" => {
            // Block is not removed at the start of your turn
            effects.push(CardEffect::ApplyToSelf {
                buff: BuffType::Barricade,
                stacks: 1,
            });
        }
        _ => {}
    }

    // For cards with no structured effects, use the description as a passive.
    if effects.is_empty() {
        if let Some(desc) = api.description.as_deref().filter(|s| !s.is_empty()) {
            effects.push(CardEffect::Passive(desc.to_owned()));
        }
    }

    let mut card = Card::new(id, &api.name, cost, card_type, rarity, effects);

    for kw in &api.keywords_key {
        match kw.as_str() {
            "exhaust" => card = card.with_exhausts(),
            "ethereal" => card = card.with_ethereal(),
            "innate" => card = card.with_innate(),
            _ => {}
        }
    }

    card
}

fn load_api_cards() -> Vec<SpireApiCard> {
    api::load_cards().unwrap_or_else(|e| {
        eprintln!("spire-slayer: failed to load card data from spire-codex.com: {e}");
        Vec::new()
    })
}

/// Convert a camelCase card ID ("StrikeIronclad") to the API's
/// SCREAMING_SNAKE format ("STRIKE_IRONCLAD").
fn camel_to_screaming_snake(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 4);
    for (i, c) in s.char_indices() {
        if i > 0 && c.is_uppercase() {
            out.push('_');
        }
        out.extend(c.to_uppercase());
    }
    out
}

/// Build a deck from character starting-deck card IDs (camelCase format as
/// returned by the `/api/characters` endpoint).  Any IDs not found in the
/// API card pool are silently skipped.
pub fn deck_from_ids(ids: &[String]) -> Vec<Card> {
    let api_cards = load_api_cards();
    let by_id: std::collections::HashMap<&str, &SpireApiCard> =
        api_cards.iter().map(|c| (c.id.as_str(), c)).collect();

    ids.iter()
        .filter_map(|id| {
            let snake = camel_to_screaming_snake(id);
            by_id.get(snake.as_str()).map(|c| api_to_domain(c))
        })
        .collect()
}

/// All cards for a character identified by their color string
/// (e.g. "ironclad", "silent", "regent", "necrobinder", "defect").
pub fn cards_for_character(color: &str) -> Vec<Card> {
    match color {
        "ironclad"   => ironclad::all_cards(),
        "silent"     => silent::all_cards(),
        "regent"     => regent::all_cards(),
        "necrobinder" => necrobinder::all_cards(),
        "defect"     => defect::all_cards(),
        _            => colorless::all_cards(),
    }
}

// ── Public modules ─────────────────────────────────────────────────────────

/// Cards for the Ironclad character.
pub mod ironclad {
    use super::*;

    /// All Ironclad cards from the API, converted to domain `Card` objects.
    pub fn all_cards() -> Vec<Card> {
        load_api_cards()
            .iter()
            .filter(|c| c.color.as_deref() == Some("ironclad"))
            .map(api_to_domain)
            .collect()
    }

    /// Ironclad's starting deck: 5× Strike, 4× Defend, 1× Bash.
    pub fn starter_deck() -> Vec<Card> {
        let api_cards = load_api_cards();
        let ironclad: Vec<&SpireApiCard> = api_cards
            .iter()
            .filter(|c| c.color.as_deref() == Some("ironclad"))
            .collect();

        let find = |id: &str| {
            ironclad
                .iter()
                .find(|c| c.id.eq_ignore_ascii_case(id))
                .map(|c| api_to_domain(c))
        };

        let mut deck = Vec::with_capacity(10);
        if let Some(s) = find("STRIKE_IRONCLAD") {
            for _ in 0..5 {
                deck.push(s.clone());
            }
        }
        if let Some(d) = find("DEFEND_IRONCLAD") {
            for _ in 0..4 {
                deck.push(d.clone());
            }
        }
        if let Some(b) = find("BASH") {
            deck.push(b);
        }
        deck
    }
}

/// Cards for the Silent character.
pub mod silent {
    use super::*;

    pub fn all_cards() -> Vec<Card> {
        load_api_cards()
            .iter()
            .filter(|c| c.color.as_deref() == Some("silent"))
            .map(api_to_domain)
            .collect()
    }
}

/// Cards for the Defect character.
pub mod defect {
    use super::*;

    pub fn all_cards() -> Vec<Card> {
        load_api_cards()
            .iter()
            .filter(|c| c.color.as_deref() == Some("defect"))
            .map(api_to_domain)
            .collect()
    }
}

/// Cards for the Regent character.
pub mod regent {
    use super::*;

    pub fn all_cards() -> Vec<Card> {
        load_api_cards()
            .iter()
            .filter(|c| c.color.as_deref() == Some("regent"))
            .map(api_to_domain)
            .collect()
    }
}

/// Cards for the Necrobinder character.
pub mod necrobinder {
    use super::*;

    pub fn all_cards() -> Vec<Card> {
        load_api_cards()
            .iter()
            .filter(|c| c.color.as_deref() == Some("necrobinder"))
            .map(api_to_domain)
            .collect()
    }
}

/// Colorless cards available to all characters.
pub mod colorless {
    use super::*;

    pub fn all_cards() -> Vec<Card> {
        load_api_cards()
            .iter()
            .filter(|c| c.color.as_deref() == Some("colorless"))
            .map(api_to_domain)
            .collect()
    }
}


#[cfg(test)]
mod tests {
    use super::ironclad;

    #[test]
    fn starter_deck_has_ten_cards() {
        let deck = ironclad::starter_deck();
        assert_eq!(deck.len(), 10, "Expected 10 starter cards, got {}", deck.len());
    }

    #[test]
    fn starter_deck_composition() {
        use crate::domain::card::CardType;
        let deck = ironclad::starter_deck();
        let bashes = deck.iter().filter(|c| c.name == "Bash").count();
        let attacks = deck.iter().filter(|c| c.card_type == CardType::Attack).count();
        assert_eq!(bashes, 1, "Expected 1 Bash");
        assert!(attacks >= 6, "Expected at least 6 attack cards (5 Strikes + 1 Bash)");
    }

    #[test]
    fn ironclad_cards_not_empty() {
        let cards = ironclad::all_cards();
        assert!(!cards.is_empty(), "Ironclad card pool should not be empty");
    }
}
