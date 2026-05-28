use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::SystemTime;

const CARDS_API_URL: &str = "https://spire-codex.com/api/cards";
const MONSTERS_API_URL: &str = "https://spire-codex.com/api/monsters";
const ENCOUNTERS_API_URL: &str = "https://spire-codex.com/api/encounters";
const EVENTS_API_URL: &str = "https://spire-codex.com/api/events";
const CACHE_TTL_SECS: u64 = 86_400;

const SEED_JSON: &str = include_str!("../../data/cards_seed.json");
const MONSTERS_SEED_JSON: &str = include_str!("../../data/monsters_seed.json");
const ENCOUNTERS_SEED_JSON: &str = include_str!("../../data/encounters_seed.json");
const EVENTS_SEED_JSON: &str = include_str!("../../data/events_seed.json");

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ApiPower {
    pub power: String,
    pub amount: Option<i32>,
}

/// Subset of the spire-codex.com `/api/cards` response fields we use.
/// Unknown JSON fields are silently ignored by serde.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SpireApiCard {
    pub id: String,
    pub name: String,
    pub cost: Option<i32>,
    #[serde(default, deserialize_with = "null_as_default")]
    pub is_x_cost: bool,
    #[serde(default, deserialize_with = "null_as_default")]
    pub is_x_star_cost: bool,
    #[serde(rename = "type")]
    pub card_type: Option<String>,
    pub rarity: Option<String>,
    pub target: Option<String>,
    pub color: Option<String>,
    pub damage: Option<i32>,
    pub block: Option<i32>,
    pub hit_count: Option<i32>,
    /// `null` in JSON is treated as an empty list.
    #[serde(default, deserialize_with = "null_as_default")]
    pub powers_applied: Vec<ApiPower>,
    pub cards_draw: Option<i32>,
    pub energy_gain: Option<i32>,
    pub hp_loss: Option<i32>,
    /// Lower-cased keyword slugs (e.g. "exhaust", "ethereal", "innate").
    #[serde(default, deserialize_with = "null_as_default")]
    pub keywords_key: Vec<String>,
    pub description: Option<String>,
}

fn null_as_default<'de, D, T>(de: D) -> Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Default + Deserialize<'de>,
{
    Ok(Option::<T>::deserialize(de)?.unwrap_or_default())
}

fn cache_is_fresh(path: &std::path::Path) -> bool {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .map(|t| {
            SystemTime::now()
                .duration_since(t)
                .unwrap_or_default()
                .as_secs()
                < CACHE_TTL_SECS
        })
        .unwrap_or(false)
}

// ── Monster API ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ApiMoveDamage {
    pub normal: Option<i32>,
    pub ascension: Option<i32>,
    pub hit_count: Option<i32>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ApiMovePower {
    pub power_id: String,
    pub target: Option<String>,
    pub amount: Option<i32>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ApiMonsterMove {
    pub id: String,
    pub name: Option<String>,
    pub intent: Option<String>,
    pub damage: Option<ApiMoveDamage>,
    pub block: Option<i32>,
    pub heal: Option<i32>,
    #[serde(default, deserialize_with = "null_as_default")]
    pub powers: Vec<ApiMovePower>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ApiInnatePower {
    pub power_id: String,
    pub amount: Option<i32>,
    pub amount_ascension: Option<i32>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SpireApiMonster {
    pub id: String,
    pub name: String,
    #[serde(rename = "type")]
    pub monster_type: Option<String>,
    pub min_hp: Option<i32>,
    pub max_hp: Option<i32>,
    pub min_hp_ascension: Option<i32>,
    pub max_hp_ascension: Option<i32>,
    #[serde(default, deserialize_with = "null_as_default")]
    pub moves: Vec<ApiMonsterMove>,
    #[serde(default, deserialize_with = "null_as_default")]
    pub innate_powers: Vec<ApiInnatePower>,
    pub attack_pattern: Option<SpireApiAttackPattern>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SpireApiAttackPattern {
    #[serde(rename = "type")]
    pub pattern_type: Option<String>,
    pub initial_move: Option<String>,
    #[serde(default, deserialize_with = "null_as_default")]
    pub states: Vec<SpireApiAiState>,
    pub description: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SpireApiAiState {
    pub id: String,
    #[serde(rename = "type")]
    pub state_type: Option<String>,
    pub move_id: Option<String>,
    pub must_perform_once: Option<bool>,
    pub next: Option<String>,
    #[serde(default, deserialize_with = "null_as_default")]
    pub branches: Vec<SpireApiAiBranch>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SpireApiAiBranch {
    pub move_id: Option<String>,
    pub weight: Option<f32>,
    pub repeat: Option<String>,
    pub max_times: Option<i32>,
    pub condition: Option<String>,
}

// ── Encounter API ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ApiEncounterMonster {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SpireApiEncounter {
    pub id: String,
    pub name: String,
    pub room_type: Option<String>,
    pub is_weak: Option<bool>,
    pub act: Option<String>,
    #[serde(default, deserialize_with = "null_as_default")]
    pub tags: Vec<String>,
    #[serde(default, deserialize_with = "null_as_default")]
    pub monsters: Vec<ApiEncounterMonster>,
    pub loss_text: Option<String>,
}

// ── Event API ─────────────────────────────────────────────────────────────────

/// A single outcome/choice branch within an event.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ApiEventChoice {
    pub id: Option<String>,
    pub text: Option<String>,
    pub description: Option<String>,
    /// Outcomes this choice can produce (e.g. "gold", "hp_loss", "card", "relic").
    #[serde(default, deserialize_with = "null_as_default")]
    pub outcomes: Vec<ApiEventOutcome>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ApiEventOutcome {
    #[serde(rename = "type")]
    pub outcome_type: Option<String>,
    pub amount: Option<i32>,
    pub description: Option<String>,
    /// If true this outcome leads to combat.
    pub combat: Option<bool>,
    pub encounter_id: Option<String>,
}

/// A spire-codex event entry.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SpireApiEvent {
    pub id: String,
    pub name: String,
    pub act: Option<String>,
    pub room_type: Option<String>,
    pub description: Option<String>,
    #[serde(default, deserialize_with = "null_as_default")]
    pub choices: Vec<ApiEventChoice>,
    #[serde(default, deserialize_with = "null_as_default")]
    pub tags: Vec<String>,
}

fn named_cache_path(name: &str) -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    [&home, ".cache", "spire-slayer", &format!("{name}.json")]
        .iter()
        .collect()
}

fn fetch_json<T: for<'de> Deserialize<'de>>(url: &str) -> Result<(String, T)> {
    let body = ureq::get(url).call()?.into_string()?;
    let data = serde_json::from_str(&body)?;
    Ok((body, data))
}

fn load_cached<T>(url: &str, cache_name: &str, seed: &str) -> Vec<T>
where
    T: for<'de> Deserialize<'de> + Serialize,
{
    let cache = named_cache_path(cache_name);

    if cache_is_fresh(&cache) {
        if let Ok(data) = std::fs::read_to_string(&cache) {
            if let Ok(items) = serde_json::from_str(&data) {
                return items;
            }
        }
    }

    match fetch_json::<Vec<T>>(url) {
        Ok((body, items)) => {
            if let Some(parent) = cache.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let _ = std::fs::write(&cache, &body);
            return items;
        }
        Err(e) => {
            eprintln!("spire-slayer: API unavailable ({e}), using bundled {cache_name} data");
        }
    }

    serde_json::from_str(seed).unwrap_or_default()
}

pub fn load_monsters() -> Vec<SpireApiMonster> {
    load_cached(MONSTERS_API_URL, "monsters", MONSTERS_SEED_JSON)
}

pub fn load_encounters() -> Vec<SpireApiEncounter> {
    load_cached(ENCOUNTERS_API_URL, "encounters", ENCOUNTERS_SEED_JSON)
}

pub fn load_events() -> Vec<SpireApiEvent> {
    load_cached(EVENTS_API_URL, "events", EVENTS_SEED_JSON)
}

// Refactor load_cards to use the same helper (keep original signature for compatibility)
pub fn load_cards() -> Result<Vec<SpireApiCard>> {
    Ok(load_cached(CARDS_API_URL, "cards", SEED_JSON))
}
