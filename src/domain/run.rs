use serde::{Deserialize, Serialize};
use crate::domain::card::Card;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PlayerClass {
    Ironclad,
    Silent,
    Defect,
    Watcher,
}

impl std::fmt::Display for PlayerClass {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let name = match self {
            Self::Ironclad => "Ironclad",
            Self::Silent => "Silent",
            Self::Defect => "Defect",
            Self::Watcher => "Watcher",
        };
        write!(f, "{name}")
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Relic {
    pub name: String,
    pub description: String,
}

impl Relic {
    pub fn new(name: impl Into<String>, description: impl Into<String>) -> Self {
        Self { name: name.into(), description: description.into() }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Potion {
    pub name: String,
}

impl Potion {
    pub fn new(name: impl Into<String>) -> Self {
        Self { name: name.into() }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunState {
    pub class: PlayerClass,
    pub floor: u8,
    pub act: u8,
    pub hp: u32,
    pub max_hp: u32,
    pub gold: u32,
    pub deck: Vec<Card>,
    pub relics: Vec<Relic>,
    /// Potion slots; None = empty slot.
    pub potions: Vec<Option<Potion>>,
}

impl RunState {
    pub fn new(
        class: PlayerClass,
        hp: u32,
        max_hp: u32,
        deck: Vec<Card>,
        starting_relic: Relic,
    ) -> Self {
        Self {
            class,
            floor: 0,
            act: 1,
            hp,
            max_hp,
            gold: 99,
            deck,
            relics: vec![starting_relic],
            potions: vec![None, None, None],
        }
    }

    pub fn deck_size(&self) -> usize {
        self.deck.len()
    }

    pub fn has_relic(&self, name: &str) -> bool {
        self.relics.iter().any(|r| r.name == name)
    }
}

pub mod starting_relics {
    use super::Relic;

    pub fn ironclad() -> Relic {
        Relic::new("Burning Blood", "At the end of combat, heal 6 HP.")
    }

    pub fn silent() -> Relic {
        Relic::new("Ring of the Snake", "At the start of each combat, draw 2 additional cards.")
    }

    pub fn defect() -> Relic {
        Relic::new("Cracked Core", "At the start of each combat, Channel 1 Lightning.")
    }

    pub fn watcher() -> Relic {
        Relic::new("Pure Water", "At the start of each combat, add 1 Miracle to your hand.")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::catalog::ironclad;

    #[test]
    fn new_ironclad_run() {
        let deck = ironclad::starter_deck();
        let relic = starting_relics::ironclad();
        let run = RunState::new(PlayerClass::Ironclad, 80, 80, deck, relic);

        assert_eq!(run.class, PlayerClass::Ironclad);
        assert_eq!(run.floor, 0);
        assert_eq!(run.act, 1);
        assert_eq!(run.hp, 80);
        assert_eq!(run.gold, 99);
        assert_eq!(run.deck_size(), 10);
        assert_eq!(run.relics.len(), 1);
        assert_eq!(run.potions.len(), 3);
        assert!(run.potions.iter().all(|p| p.is_none()));
    }

    #[test]
    fn has_relic_check() {
        let deck = ironclad::starter_deck();
        let relic = starting_relics::ironclad();
        let run = RunState::new(PlayerClass::Ironclad, 80, 80, deck, relic);

        assert!(run.has_relic("Burning Blood"));
        assert!(!run.has_relic("Anchor"));
    }

    #[test]
    fn display_player_class() {
        assert_eq!(PlayerClass::Ironclad.to_string(), "Ironclad");
        assert_eq!(PlayerClass::Silent.to_string(), "Silent");
    }

    #[test]
    fn serialization_round_trip() {
        let deck = ironclad::starter_deck();
        let relic = starting_relics::ironclad();
        let run = RunState::new(PlayerClass::Ironclad, 80, 80, deck, relic);
        let json = serde_json::to_string(&run).unwrap();
        let back: RunState = serde_json::from_str(&json).unwrap();
        assert_eq!(back.class, PlayerClass::Ironclad);
        assert_eq!(back.deck_size(), 10);
    }
}
