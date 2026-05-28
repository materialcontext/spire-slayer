use rand::Rng;

use crate::data::api::{SpireApiEncounter, SpireApiEvent, SpireApiMonster};
use crate::domain::card::Card;
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
    pub treasure: NodeEv,
    pub rest: NodeEv,
    pub shop: NodeEv,
    pub event_node: NodeEv,
    pub events: Vec<EventSummary>,
    pub shared_event_count: usize,
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

pub fn compute_map_ev(
    deck: &[Card],
    hp: u32,
    max_hp: u32,
    sub_act: &str,
    all_encounters: &[SpireApiEncounter],
    all_monsters: &[SpireApiMonster],
    all_events: &[SpireApiEvent],
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

    let normal = combat_node_ev("Normal", "gold + card", &normal_stats, 0);
    let elite_star_bonus: i8 = if elite_stats.survival_rate >= 0.70 { 1 } else { 0 };
    let elite = combat_node_ev("Elite", "relic + card + gold", &elite_stats, elite_star_bonus);

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

    MapEvData {
        sub_act: sub_act.to_string(),
        normal,
        elite,
        treasure,
        rest,
        shop,
        event_node,
        events,
        shared_event_count,
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
        let data = compute_map_ev(&deck, 80, 80, "overgrowth", &[], &[], &[], &mut rng);
        assert_eq!(data.sub_act, "overgrowth");
        assert_eq!(data.treasure.stars, 3);
        assert_eq!(data.events.len(), 0);
    }
}
