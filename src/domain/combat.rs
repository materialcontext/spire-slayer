use std::collections::{HashMap, HashSet};
use serde::{Deserialize, Serialize};
use crate::domain::ai::{AiRuntime, EnemyAiScript};
use crate::domain::card::Card;
use crate::domain::effect::BuffType;

pub type Buffs = HashMap<BuffType, i32>;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlayerState {
    pub hp: u32,
    pub max_hp: u32,
    pub block: u32,
    pub buffs: Buffs,
}

impl PlayerState {
    pub fn new(hp: u32, max_hp: u32) -> Self {
        Self { hp, max_hp, block: 0, buffs: HashMap::new() }
    }

    pub fn buff(&self, b: &BuffType) -> i32 {
        *self.buffs.get(b).unwrap_or(&0)
    }

    pub fn is_alive(&self) -> bool {
        self.hp > 0
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Intent {
    Attack(u32),
    AttackMulti { damage: u32, hits: u32 },
    Block,
    Buff,
    DebuffPlayer,
    Escape,
    Unknown,
}


#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnemyState {
    pub name: String,
    pub hp: u32,
    pub max_hp: u32,
    pub block: u32,
    pub intent: Intent,
    pub buffs: Buffs,
    pub ai_script: Option<EnemyAiScript>,
    pub ai_runtime: AiRuntime,
}

impl EnemyState {
    pub fn new(name: impl Into<String>, hp: u32, intent: Intent) -> Self {
        Self {
            name: name.into(),
            hp,
            max_hp: hp,
            block: 0,
            intent,
            buffs: HashMap::new(),
            ai_script: None,
            ai_runtime: AiRuntime::default(),
        }
    }

    pub fn buff(&self, b: &BuffType) -> i32 {
        *self.buffs.get(b).unwrap_or(&0)
    }

    pub fn is_alive(&self) -> bool {
        self.hp > 0
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CombatState {
    pub player: PlayerState,
    pub enemies: Vec<EnemyState>,
    pub hand: Vec<Card>,
    /// Index 0 = top of deck (next card drawn).
    pub draw_pile: Vec<Card>,
    pub discard_pile: Vec<Card>,
    pub exhaust_pile: Vec<Card>,
    pub energy: u8,
    pub energy_max: u8,
    pub hand_size: u8,
    pub turn: u32,
    /// Canonical relic IDs the player holds (SCREAMING_SNAKE_CASE).
    pub relics: HashSet<String>,
    /// True for Elite combats (affects relics like Sling of Courage).
    pub is_elite: bool,
    // Per-turn counters, reset at start of each turn
    pub attacks_this_turn: u32,
    pub skills_this_turn: u32,
    // Per-combat counters
    pub attacks_this_combat: u32,
    pub hp_lost_this_combat: bool,
    pub hp_lost_this_turn: u32,
    pub lizard_tail_triggered: bool,
}

impl CombatState {
    pub fn new(player: PlayerState, enemies: Vec<EnemyState>, deck: Vec<Card>) -> Self {
        Self {
            player,
            enemies,
            hand: Vec::new(),
            draw_pile: deck,
            discard_pile: Vec::new(),
            exhaust_pile: Vec::new(),
            energy: 3,
            energy_max: 3,
            hand_size: 5,
            turn: 1,
            relics: HashSet::new(),
            is_elite: false,
            attacks_this_turn: 0,
            skills_this_turn: 0,
            attacks_this_combat: 0,
            hp_lost_this_combat: false,
            hp_lost_this_turn: 0,
            lizard_tail_triggered: false,
        }
    }

    pub fn has_relic(&self, id: &str) -> bool {
        self.relics.contains(id)
    }

    pub fn is_won(&self) -> bool {
        self.enemies.iter().all(|e| !e.is_alive())
    }

    pub fn is_lost(&self) -> bool {
        !self.player.is_alive()
    }

    pub fn is_over(&self) -> bool {
        self.is_won() || self.is_lost()
    }

}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::catalog::ironclad;

    #[test]
    fn new_combat_starts_correctly() {
        let player = PlayerState::new(80, 80);
        let enemy = EnemyState::new("Cultist", 50, Intent::Attack(9));
        let deck = ironclad::starter_deck();
        let state = CombatState::new(player, vec![enemy], deck);

        assert_eq!(state.player.hp, 80);
        assert_eq!(state.enemies.len(), 1);
        assert_eq!(state.draw_pile.len(), 10);
        assert_eq!(state.energy, 3);
        assert_eq!(state.turn, 1);
        assert!(!state.is_over());
    }

    #[test]
    fn win_condition_all_enemies_dead() {
        let player = PlayerState::new(80, 80);
        let mut enemy = EnemyState::new("Cultist", 50, Intent::Attack(9));
        enemy.hp = 0;
        let state = CombatState::new(player, vec![enemy], vec![]);

        assert!(state.is_won());
        assert!(state.is_over());
        assert!(!state.is_lost());
    }

    #[test]
    fn loss_condition_player_dead() {
        let mut player = PlayerState::new(80, 80);
        player.hp = 0;
        let enemy = EnemyState::new("Cultist", 50, Intent::Attack(9));
        let state = CombatState::new(player, vec![enemy], vec![]);

        assert!(state.is_lost());
        assert!(state.is_over());
        assert!(!state.is_won());
    }

    #[test]
    fn total_cards_counts_all_zones() {
        let player = PlayerState::new(80, 80);
        let enemy = EnemyState::new("Cultist", 50, Intent::Unknown);
        let deck = ironclad::starter_deck();
        let mut state = CombatState::new(player, vec![enemy], deck);
        // Simulate drawing 5 cards
        for _ in 0..5 {
            let card = state.draw_pile.remove(0);
            state.hand.push(card);
        }
        assert_eq!(state.hand.len() + state.draw_pile.len() + state.discard_pile.len(), 10);
    }
}
