use serde::{Deserialize, Serialize};
use crate::domain::card::Card;
use crate::domain::map::{ActMap, MapPos};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PlayerClass {
    Ironclad,
    Silent,
    Regent,
    Necrobinder,
    Defect,
    Watcher,
}

impl std::fmt::Display for PlayerClass {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let name = match self {
            Self::Ironclad   => "Ironclad",
            Self::Silent     => "Silent",
            Self::Regent     => "Regent",
            Self::Necrobinder => "Necrobinder",
            Self::Defect     => "Defect",
            Self::Watcher    => "Watcher",
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
    /// Game floor number (0-indexed): 0 = ancient event, 1–15 = map grid, 16 = boss.
    pub floor: u8,
    pub act: u8,
    pub hp: u32,
    pub max_hp: u32,
    pub gold: u32,
    pub deck: Vec<Card>,
    pub relics: Vec<Relic>,
    /// Potion slots; None = empty slot.
    pub potions: Vec<Option<Potion>>,
    /// Generated map for the current sub-act, if available.
    pub map: Option<ActMap>,
    /// Current node on the map (floor, col).
    pub map_pos: Option<MapPos>,
    /// Current sub-act name, e.g. "overgrowth".
    pub sub_act: String,
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
            map: None,
            map_pos: None,
            sub_act: "overgrowth".to_string(),
        }
    }

    pub fn deck_size(&self) -> usize {
        self.deck.len()
    }

    pub fn has_relic(&self, name: &str) -> bool {
        self.relics.iter().any(|r| r.name == name)
    }

    /// Generate (or regenerate) the map for the current sub-act.
    pub fn generate_map(&mut self, seed: u64, ascension: u8) {
        self.map = Some(ActMap::generate(seed, ascension));
        self.map_pos = None;
    }

    /// Heal at a rest site: restore 30% max HP (floored), capped at max.
    pub fn heal(&mut self) {
        let amount = ((self.max_hp as f32) * 0.30).floor() as u32;
        self.hp = (self.hp + amount).min(self.max_hp);
    }

    /// Smith at a rest site: upgrade the card at `deck_idx`.
    /// Returns `false` if the index is out of range or the card is already upgraded.
    pub fn smith(&mut self, deck_idx: usize) -> bool {
        let Some(card) = self.deck.get_mut(deck_idx) else { return false; };
        if card.upgraded { return false; }
        card.upgraded = true;
        true
    }

    /// Move to a new node, returning `false` if the move is illegal.
    pub fn move_to(&mut self, col: usize) -> bool {
        let Some(ref map) = self.map else { return false; };
        match self.map_pos {
            None => {
                if map.entry_nodes().contains(&(col as u8)) {
                    self.map_pos = Some(MapPos { floor: 0, col });
                    true
                } else {
                    false
                }
            }
            Some(pos) => {
                if map.next_nodes(pos.floor, pos.col).contains(&(col as u8)) {
                    self.map_pos = Some(MapPos { floor: pos.floor + 1, col });
                    self.floor = (pos.floor + 1) as u8;
                    true
                } else {
                    false
                }
            }
        }
    }
}

pub mod starting_relics {
    use super::{PlayerClass, Relic};

    pub fn ironclad() -> Relic {
        Relic::new("Burning Blood", "At the end of combat, heal 6 HP.")
    }

    pub fn silent() -> Relic {
        Relic::new("Ring of the Snake", "At the start of each combat, draw 2 additional cards.")
    }

    pub fn regent() -> Relic {
        Relic::new("Divine Right", "At the start of each combat, gain 3 Stars.")
    }

    pub fn necrobinder() -> Relic {
        Relic::new("Bound Phylactery", "At the start of each combat, Summon 1.")
    }

    pub fn defect() -> Relic {
        Relic::new("Cracked Core", "At the start of each combat, Channel 1 Lightning.")
    }

    pub fn watcher() -> Relic {
        Relic::new("Pure Water", "At the start of each combat, add 1 Miracle to your hand.")
    }

    pub fn for_class(class: &PlayerClass) -> Relic {
        match class {
            PlayerClass::Ironclad   => ironclad(),
            PlayerClass::Silent     => silent(),
            PlayerClass::Regent     => regent(),
            PlayerClass::Necrobinder => necrobinder(),
            PlayerClass::Defect     => defect(),
            PlayerClass::Watcher    => watcher(),
        }
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
