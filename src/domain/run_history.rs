use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TurnRecord {
    pub turn: u32,
    pub cards_played: Vec<String>,
    pub damage_dealt: u32,
    pub block_gained: u32,
    pub player_hp_before: u32,
    pub player_hp_after: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CombatEntry {
    pub encounter_name: String,
    pub hp_lost: u32,
    pub gold_gained: u32,
    pub turns: Vec<TurnRecord>,
    pub card_picked: Option<String>,
    pub relic_gained: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShopPurchase {
    pub name: String,
    pub cost: u32,
    pub kind: String, // "Card", "Relic", "Potion"
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShopEntry {
    pub gold_spent: u32,
    pub purchases: Vec<ShopPurchase>,
    pub card_removed: Option<String>,
    pub removal_cost: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventEntry {
    pub event_name: String,
    pub option_chosen: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RestEntry {
    pub hp_gained: u32,       // >0 if healed, 0 if smithed
    pub card_smithed: Option<String>,
    #[serde(default)]
    pub max_hp_gained: u32,       // STONE_HUMIDIFIER
    #[serde(default)]
    pub relic_dug: Option<String>, // SHOVEL
    #[serde(default)]
    pub strength_lifted: u32,      // GIRYA
    #[serde(default)]
    pub card_dreamed: Option<String>, // DREAM_CATCHER
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TreasureEntry {
    pub relic_offered: String,
    pub taken: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AncientEntry {
    pub boon_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum EntryDetail {
    Combat(CombatEntry),
    Shop(ShopEntry),
    Event(EventEntry),
    Rest(RestEntry),
    Treasure(TreasureEntry),
    AncientBoon(AncientEntry),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub floor: u8,
    pub act: u8,
    pub sub_act: String,
    pub room_label: String,
    pub hp_before: u32,
    pub hp_after: u32,
    pub gold_before: u32,
    pub gold_after: u32,
    pub detail: EntryDetail,
}
