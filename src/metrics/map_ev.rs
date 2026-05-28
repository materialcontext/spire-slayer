use rand::Rng;
use rand::seq::SliceRandom;

use crate::data::api::{SpireApiEncounter, SpireApiEvent, SpireApiMonster};
use crate::domain::card::{Card, Rarity};
use crate::domain::effect::CardEffect;
use super::deck_dash::{compute_deck_stats, DeckStats};

// ── Public types ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct NodeEv {
    pub label: &'static str,
    pub mean_hp_loss: f32,
    pub survival_rate: f32,
    pub encounter_count: usize,
    pub reward: &'static str,
    pub stars: u8,
}

#[derive(Debug, Clone)]
pub struct EventSummary {
    pub name: String,
    pub act: String,
    pub description: String,
    pub option_titles: Vec<String>,
    pub is_shared: bool,
}

#[derive(Debug, Clone)]
pub struct MapEvData {
    pub sub_act: String,
    pub normal: NodeEv,
    pub elite: NodeEv,
    pub boss: NodeEv,
    pub treasure: NodeEv,
    pub rest: NodeEv,
    pub shop: NodeEv,
    pub event_node: NodeEv,
    pub events: Vec<EventSummary>,
    pub shared_event_count: usize,
    /// HP the player will have at the start of the next act (post-act heal).
    /// Full heal at ascension < 6; 80 % of max HP at ascension ≥ 6.
    pub post_act_heal_hp: f32,
    /// Expected HP change from the best event option, averaged over the act event pool.
    pub event_hp_delta: f32,
    /// Expected HP equivalent from a treasure relic (rarity-weighted heuristic).
    pub treasure_hp: f32,
    /// Expected HP saved by using the shop for card removal (differential simulation).
    pub shop_hp_value: f32,
}

// ── Event filtering ───────────────────────────────────────────────────────────

/// Returns all canonical sub-act names that an event `act` string covers.
/// Handles both single-act ("Act 1 - Overgrowth") and cross-sub-act
/// ("Act 1 - Overgrowth / Underdocks") formats.
fn event_sub_acts(act: &str) -> Vec<&'static str> {
    let s = act.to_lowercase();
    let mut acts = Vec::new();
    if s.contains("overgrowth") { acts.push("overgrowth"); }
    if s.contains("underdock")  { acts.push("underdocks"); }
    if s.contains("hive")       { acts.push("hive"); }
    if s.contains("glory")      { acts.push("glory"); }
    if s.contains("boss")       { acts.push("boss"); }
    if acts.is_empty()          { acts.push("other"); }
    acts
}

/// Return events relevant to `sub_act`: sub-act-specific events, cross-sub-act
/// events, and Shared (null-act) events that appear in every act.
pub fn events_for_sub_act<'a>(sub_act: &str, events: &'a [SpireApiEvent]) -> Vec<&'a SpireApiEvent> {
    events
        .iter()
        .filter(|e| {
            if e.event_type.as_deref() == Some("Shared") || e.act.is_none() {
                return true;
            }
            if let Some(act) = &e.act {
                return event_sub_acts(act).contains(&sub_act);
            }
            false
        })
        .collect()
}

/// Strip STS2 colour/formatting tags like [green]…[/green] from a string.
pub fn strip_tags(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_tag = false;
    for c in s.chars() {
        match c {
            '[' => in_tag = true,
            ']' if in_tag => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    out
}

// ── HP delta parsing ──────────────────────────────────────────────────────────

fn parse_hp_delta(plain: &str) -> f32 {
    let words: Vec<&str> = plain.split_whitespace().collect();
    let mut delta = 0.0_f32;
    for i in 0..words.len() {
        let Ok(n) = words[i].parse::<f32>() else { continue };
        let prev  = if i > 0             { words[i-1] } else { "" };
        let next  = if i+1 < words.len() { words[i+1] } else { "" };
        let next2 = if i+2 < words.len() { words[i+2] } else { "" };
        let is_hp  = next.eq_ignore_ascii_case("hp") || next.eq_ignore_ascii_case("health");
        let is_dmg = next.eq_ignore_ascii_case("damage") || next.eq_ignore_ascii_case("dmg");
        let is_hp2  = !is_hp  && (next2.eq_ignore_ascii_case("hp") || next2.eq_ignore_ascii_case("health"));
        let is_dmg2 = !is_dmg && (next2.eq_ignore_ascii_case("damage") || next2.eq_ignore_ascii_case("dmg"));
        match prev.to_lowercase().as_str() {
            "heal" | "heals" | "restore" | "restores" => delta += n,
            "gain" if is_hp || is_hp2   => delta += n,
            "lose" | "loses" if is_hp || is_hp2 => delta -= n,
            "take" | "takes" if is_dmg || is_dmg2 => delta -= n,
            "deal" | "deals" if is_dmg || is_dmg2 => delta -= n,
            _ => {}
        }
    }
    delta
}

/// Compute the mean best-option HP delta across the act event pool.
pub fn compute_event_hp_delta(
    events: &[SpireApiEvent],
    sub_act: &str,
    _hp_ratio: f32,
) -> f32 {
    let pool = events_for_sub_act(sub_act, events);
    if pool.is_empty() {
        return -2.0;
    }
    // For each event, find the best available option's HP delta; average over pool.
    let total: f32 = pool.iter().map(|event| {
        event.options.iter()
            .map(|opt| {
                let raw = opt.description.as_deref().unwrap_or("");
                let plain = strip_tags(raw).to_lowercase();
                parse_hp_delta(&plain)
            })
            .fold(f32::NEG_INFINITY, f32::max)
            .max(-50.0) // clamp runaway negatives
    }).sum::<f32>();
    total / pool.len() as f32
}

// ── Card value scoring ────────────────────────────────────────────────────────

fn card_value_score(card: &Card) -> f32 {
    if card.cost == 255 { return -100.0; }
    let mut score = 0.0f32;
    for effect in &card.effects {
        match effect {
            CardEffect::Damage(d) => score += *d as f32 / card.cost.max(1) as f32,
            CardEffect::Block(b)  => score += *b as f32 * 0.8 / card.cost.max(1) as f32,
            CardEffect::Draw(n)   => score += *n as f32 * 2.0,
            _                     => score += 1.0,
        }
    }
    if matches!(card.rarity, Rarity::Basic) { score *= 0.7; }
    score
}

/// Estimate the HP value of removing the worst card via the shop.
pub fn compute_shop_hp_value(
    deck: &[Card],
    hp: u32,
    max_hp: u32,
    sub_act: &str,
    all_encounters: &[SpireApiEncounter],
    all_monsters: &[SpireApiMonster],
    gold: u32,
    rng: &mut impl Rng,
) -> f32 {
    const REMOVAL_COST: u32 = 75;
    const REMAINING_FIGHTS: f32 = 5.0;
    if gold < REMOVAL_COST || deck.len() <= 1 { return 0.0; }
    let worst_idx = deck.iter().enumerate()
        .min_by(|(_, a), (_, b)|
            card_value_score(a).partial_cmp(&card_value_score(b))
                .unwrap_or(std::cmp::Ordering::Equal))
        .map(|(i, _)| i)
        .unwrap_or(0);
    let deck_reduced: Vec<Card> = deck.iter().enumerate()
        .filter(|(i, _)| *i != worst_idx)
        .map(|(_, c)| c.clone())
        .collect();
    let full_stats    = compute_deck_stats(deck, hp, max_hp, sub_act, all_encounters, all_monsters, rng);
    let reduced_stats = compute_deck_stats(&deck_reduced, hp, max_hp, sub_act, all_encounters, all_monsters, rng);
    if full_stats.encounter_count == 0 {
        return if deck.iter().any(|c| c.cost == 255) { 12.0 } else { 3.0 };
    }
    (full_stats.mean_hp_loss - reduced_stats.mean_hp_loss).max(0.0) * REMAINING_FIGHTS
}

// ── Core computation ──────────────────────────────────────────────────────────

fn combat_node_ev(
    label: &'static str,
    reward: &'static str,
    stats: &DeckStats,
    star_bonus: i8,
) -> NodeEv {
    let base: u8 = if stats.encounter_count == 0 {
        2
    } else if stats.survival_rate >= 0.85 {
        3
    } else if stats.survival_rate >= 0.60 {
        2
    } else {
        1
    };
    NodeEv {
        label,
        mean_hp_loss: stats.mean_hp_loss,
        survival_rate: stats.survival_rate,
        encounter_count: stats.encounter_count,
        reward,
        stars: (base as i8 + star_bonus).clamp(1, 3) as u8,
    }
}

/// Post-act heal target HP.
/// Below this ascension level the ancient heals to full; at or above it heals to 80 %.
pub const POST_ACT_HEAL_ASCENSION_THRESHOLD: u8 = 6;

pub fn post_act_heal_hp(max_hp: u32, ascension: u8) -> f32 {
    if ascension < POST_ACT_HEAL_ASCENSION_THRESHOLD {
        max_hp as f32
    } else {
        (max_hp as f32 * 0.80).floor()
    }
}

pub fn compute_map_ev(
    deck: &[Card],
    hp: u32,
    max_hp: u32,
    ascension: u8,
    sub_act: &str,
    all_encounters: &[SpireApiEncounter],
    all_monsters: &[SpireApiMonster],
    all_events: &[SpireApiEvent],
    gold: u32,
    rng: &mut impl Rng,
) -> MapEvData {
    // Simulate vs. normal (Monster) encounters for this sub-act.
    let normal_enc: Vec<SpireApiEncounter> = all_encounters
        .iter()
        .filter(|e| e.room_type.as_deref() == Some("Monster"))
        .cloned()
        .collect();
    let normal_stats =
        compute_deck_stats(deck, hp, max_hp, sub_act, &normal_enc, all_monsters, rng);

    // Simulate vs. elite encounters — elites give a guaranteed relic so the
    // star bonus reflects the relic reward even at moderate HP risk.
    let elite_enc: Vec<SpireApiEncounter> = all_encounters
        .iter()
        .filter(|e| e.room_type.as_deref() == Some("Elite"))
        .cloned()
        .collect();
    let elite_stats =
        compute_deck_stats(deck, hp, max_hp, sub_act, &elite_enc, all_monsters, rng);

    // Simulate vs. boss encounters for this sub-act.
    let boss_enc: Vec<SpireApiEncounter> = all_encounters
        .iter()
        .filter(|e| e.room_type.as_deref() == Some("Boss"))
        .cloned()
        .collect();
    let boss_stats =
        compute_deck_stats(deck, hp, max_hp, sub_act, &boss_enc, all_monsters, rng);

    let normal = combat_node_ev("Normal", "gold + card", &normal_stats, 0);
    let elite_star_bonus: i8 = if elite_stats.survival_rate >= 0.70 { 1 } else { 0 };
    let elite = combat_node_ev("Elite", "relic + card + gold", &elite_stats, elite_star_bonus);
    let boss = combat_node_ev("Boss", "relic + 3 rare cards", &boss_stats, -1);

    let hp_frac = hp as f32 / max_hp.max(1) as f32;
    let rest_stars: u8 = if hp_frac < 0.60 { 3 } else { 2 };

    let treasure = NodeEv {
        label: "Treasure",
        mean_hp_loss: 0.0,
        survival_rate: 1.0,
        encounter_count: 0,
        reward: "free relic",
        stars: 3,
    };
    let rest = NodeEv {
        label: "Rest",
        mean_hp_loss: 0.0,
        survival_rate: 1.0,
        encounter_count: 0,
        reward: "heal 30% or forge",
        stars: rest_stars,
    };
    let shop = NodeEv {
        label: "Shop",
        mean_hp_loss: 0.0,
        survival_rate: 1.0,
        encounter_count: 0,
        reward: "buy cards / remove",
        stars: 2,
    };

    let event_refs = events_for_sub_act(sub_act, all_events);
    let shared_event_count = event_refs
        .iter()
        .filter(|e| e.event_type.as_deref() == Some("Shared") || e.act.is_none())
        .count();

    let events: Vec<EventSummary> = event_refs
        .into_iter()
        .map(|e| {
            let option_titles: Vec<String> = e
                .options
                .iter()
                .map(|o| o.title.clone().unwrap_or_else(|| o.id.clone()))
                .collect();
            let description = strip_tags(e.description.as_deref().unwrap_or(""));
            EventSummary {
                name: e.name.clone(),
                act: e.act.clone().unwrap_or_else(|| "Shared".to_string()),
                description,
                option_titles,
                is_shared: e.event_type.as_deref() == Some("Shared") || e.act.is_none(),
            }
        })
        .collect();

    let event_node = NodeEv {
        label: "Event (?)",
        mean_hp_loss: 0.0,
        survival_rate: 1.0,
        encounter_count: events.len(), // repurposed: total events for this sub-act
        reward: "varies",
        stars: 2,
    };

    let hp_ratio = hp as f32 / max_hp.max(1) as f32;
    let event_hp_delta = compute_event_hp_delta(all_events, sub_act, hp_ratio);
    let treasure_hp = 6.0_f32;
    let shop_hp_value = compute_shop_hp_value(deck, hp, max_hp, sub_act, all_encounters, all_monsters, gold, rng);

    MapEvData {
        sub_act: sub_act.to_string(),
        normal,
        elite,
        boss,
        treasure,
        rest,
        shop,
        event_node,
        events,
        shared_event_count,
        post_act_heal_hp: post_act_heal_hp(max_hp, ascension),
        event_hp_delta,
        treasure_hp,
        shop_hp_value,
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_tags_removes_colour_codes() {
        assert_eq!(strip_tags("[green]2[/green] HP"), "2 HP");
        assert_eq!(strip_tags("plain text"), "plain text");
        assert_eq!(strip_tags("[red][jitter]fire[/jitter][/red]"), "fire");
    }

    #[test]
    fn events_for_sub_act_includes_shared() {
        use crate::data::api::{ApiEventOption, ApiEventPage, SpireApiEvent};
        let events = vec![
            SpireApiEvent {
                id: "S".into(),
                name: "Shared".into(),
                event_type: Some("Shared".into()),
                act: None,
                description: None,
                preconditions: vec![],
                options: vec![],
                pages: vec![],
                relics: vec![],
                epithet: None,
                dialogue: None,
                image_url: None,
            },
            SpireApiEvent {
                id: "OG".into(),
                name: "Overgrowth Only".into(),
                event_type: Some("Event".into()),
                act: Some("Act 1 - Overgrowth".into()),
                description: None,
                preconditions: vec![],
                options: vec![],
                pages: vec![],
                relics: vec![],
                epithet: None,
                dialogue: None,
                image_url: None,
            },
            SpireApiEvent {
                id: "UD".into(),
                name: "Underdocks Only".into(),
                event_type: Some("Event".into()),
                act: Some("Underdocks".into()),
                description: None,
                preconditions: vec![],
                options: vec![],
                pages: vec![],
                relics: vec![],
                epithet: None,
                dialogue: None,
                image_url: None,
            },
            SpireApiEvent {
                id: "CROSS".into(),
                name: "Cross Act".into(),
                event_type: Some("Event".into()),
                act: Some("Act 1 - Overgrowth / Underdocks".into()),
                description: None,
                preconditions: vec![],
                options: vec![],
                pages: vec![],
                relics: vec![],
                epithet: None,
                dialogue: None,
                image_url: None,
            },
        ];

        let og = events_for_sub_act("overgrowth", &events);
        assert_eq!(og.len(), 3); // Shared + OG + CROSS

        let ud = events_for_sub_act("underdocks", &events);
        assert_eq!(ud.len(), 3); // Shared + UD + CROSS

        let hive = events_for_sub_act("hive", &events);
        assert_eq!(hive.len(), 1); // Shared only
    }

    #[test]
    fn compute_map_ev_no_data() {
        use rand::SeedableRng;
        use rand::rngs::StdRng;
        let mut rng = StdRng::seed_from_u64(1);
        let deck = crate::domain::catalog::ironclad::starter_deck();
        let data = compute_map_ev(&deck, 80, 80, 0, "overgrowth", &[], &[], &[], 0, &mut rng);
        assert_eq!(data.sub_act, "overgrowth");
        assert_eq!(data.treasure.stars, 3);
        assert_eq!(data.events.len(), 0);
    }
}
