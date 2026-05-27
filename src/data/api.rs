use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::SystemTime;

const API_URL: &str = "https://spire-codex.com/api/cards";
const CACHE_TTL_SECS: u64 = 86_400;

/// Bundled card data used when the live API is unreachable.
const SEED_JSON: &str = include_str!("../../data/cards_seed.json");

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

fn cache_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    [&home, ".cache", "spire-slayer", "cards.json"]
        .iter()
        .collect()
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

/// Load all STS2 cards from the spire-codex.com API.
///
/// Resolution order:
/// 1. Local cache (`~/.cache/spire-slayer/cards.json`) if less than one day old.
/// 2. Live API — refreshes the cache on success.
/// 3. Bundled seed data compiled into the binary.
pub fn load_cards() -> Result<Vec<SpireApiCard>> {
    let cache = cache_path();

    if cache_is_fresh(&cache) {
        if let Ok(data) = std::fs::read_to_string(&cache) {
            if let Ok(cards) = serde_json::from_str(&data) {
                return Ok(cards);
            }
        }
    }

    match fetch_from_api() {
        Ok((body, cards)) => {
            if let Some(parent) = cache.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let _ = std::fs::write(&cache, &body);
            return Ok(cards);
        }
        Err(e) => {
            eprintln!("spire-slayer: API unavailable, using bundled card data: {e}");
        }
    }

    Ok(serde_json::from_str(SEED_JSON)?)
}

fn fetch_from_api() -> Result<(String, Vec<SpireApiCard>)> {
    let body = ureq::get(API_URL).call()?.into_string()?;
    let cards = serde_json::from_str(&body)?;
    Ok((body, cards))
}
