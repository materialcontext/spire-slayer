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
}

impl std::fmt::Display for PlayerClass {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let name = match self {
            Self::Ironclad   => "Ironclad",
            Self::Silent     => "Silent",
            Self::Regent     => "Regent",
            Self::Necrobinder => "Necrobinder",
            Self::Defect     => "Defect",
        };
        write!(f, "{name}")
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Relic {
    /// Canonical SCREAMING_SNAKE_CASE relic ID (e.g. "BURNING_BLOOD").
    pub id: String,
    pub name: String,
    pub description: String,
}

impl Relic {
    pub fn new(id: impl Into<String>, name: impl Into<String>, description: impl Into<String>) -> Self {
        Self { id: id.into(), name: name.into(), description: description.into() }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Potion {
    /// Canonical SCREAMING_SNAKE_CASE potion ID (e.g. "FIRE_POTION").
    pub id: String,
    pub name: String,
    pub description: String,
}

impl Potion {
    pub fn new(id: impl Into<String>, name: impl Into<String>, description: impl Into<String>) -> Self {
        Self { id: id.into(), name: name.into(), description: description.into() }
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
    /// Rare-card offset for card rewards (percentage points, f32 for A7 half-increments).
    /// Starts at -5.0, increments +1.0 per non-rare card rolled (+0.5 at A7+), cap +40, resets to -5 on rare.
    pub rare_offset: f32,
    /// Number of card removals purchased at a shop this run (affects removal price).
    pub card_removals_bought: u32,
    /// Current ascension level (0 = normal mode).
    pub ascension: u8,
    /// Whether Darv appeared as this run's Act 2 ancient (affects Act 3 pool).
    pub had_darv_act2: bool,
    /// Boss name for the current act, determined at map generation time (shown on map view).
    pub boss_name: Option<String>,
    /// History of all nodes visited this run, in order.
    #[serde(default)]
    pub history: Vec<crate::domain::run_history::HistoryEntry>,
    /// Number of Girya uses this run (max 3).
    #[serde(default)]
    pub girya_uses: u32,
    /// Permanent Strength accumulated from Girya; applied at combat start.
    #[serde(default)]
    pub girya_stacks: i32,
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
            rare_offset: -5.0,
            card_removals_bought: 0,
            ascension: 0,
            had_darv_act2: false,
            boss_name: None,
            history: Vec::new(),
            girya_uses: 0,
            girya_stacks: 0,
        }
    }

    /// Returns true if the player currently holds the relic with the given ID.
    pub fn has_relic(&self, id: &str) -> bool {
        self.relics.iter().any(|r| r.id == id)
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
    /// Returns `false` if the index is out of range or already upgraded.
    pub fn smith(&mut self, deck_idx: usize) -> bool {
        if let Some(card) = self.deck.get_mut(deck_idx) {
            if card.upgraded { return false; }
            card.upgrade();
            true
        } else {
            false
        }
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
        Relic::new("BURNING_BLOOD", "Burning Blood", "At the end of combat, heal 6 HP.")
    }

    pub fn silent() -> Relic {
        Relic::new("RING_OF_THE_SNAKE", "Ring of the Snake", "At the start of each combat, draw 2 additional cards.")
    }

    pub fn regent() -> Relic {
        Relic::new("DIVINE_RIGHT", "Divine Right", "At the start of each combat, gain 3 Stars.")
    }

    pub fn necrobinder() -> Relic {
        Relic::new("BOUND_PHYLACTERY", "Bound Phylactery", "At the start of each combat, Summon 1.")
    }

    pub fn defect() -> Relic {
        Relic::new("CRACKED_CORE", "Cracked Core", "At the start of each combat, Channel 1 Lightning.")
    }

    pub fn for_class(class: &PlayerClass) -> Relic {
        match class {
            PlayerClass::Ironclad   => ironclad(),
            PlayerClass::Silent     => silent(),
            PlayerClass::Regent     => regent(),
            PlayerClass::Necrobinder => necrobinder(),
            PlayerClass::Defect     => defect(),
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
        assert_eq!(run.deck.len(), 10);
        assert_eq!(run.relics.len(), 1);
        assert_eq!(run.potions.len(), 3);
        assert!(run.potions.iter().all(|p| p.is_none()));
    }

    #[test]
    fn has_relic_check() {
        let deck = ironclad::starter_deck();
        let relic = starting_relics::ironclad();
        let run = RunState::new(PlayerClass::Ironclad, 80, 80, deck, relic);

        assert!(run.relics.iter().any(|r| r.name == "Burning Blood"));
        assert!(!run.relics.iter().any(|r| r.name == "Anchor"));
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
        assert_eq!(back.deck.len(), 10);
    }

    #[test]
    fn smith_upgrades_card_and_prevents_double_upgrade() {
        let deck = ironclad::starter_deck();
        let relic = starting_relics::ironclad();
        let mut run = RunState::new(PlayerClass::Ironclad, 80, 80, deck, relic);

        let original_damage = run.deck[0].base_damage();
        assert!(!run.deck[0].upgraded);

        assert!(run.smith(0), "first smith should succeed");
        assert!(run.deck[0].upgraded);
        assert!(run.deck[0].base_damage() > original_damage, "upgrade should increase damage");

        assert!(!run.smith(0), "second smith on same card should fail");
    }

    #[test]
    fn starting_relics_have_correct_ids() {
        assert_eq!(starting_relics::ironclad().id, "BURNING_BLOOD");
        assert_eq!(starting_relics::silent().id, "RING_OF_THE_SNAKE");
        assert_eq!(starting_relics::defect().id, "CRACKED_CORE");
    }
}
