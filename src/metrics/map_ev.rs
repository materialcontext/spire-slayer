use rand::Rng;

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
    /// Expected HP change from the best event option, averaged over the act event pool.
    pub event_hp_delta: f32,
    /// Expected HP equivalent from a treasure relic (rarity-weighted heuristic).
    pub treasure_hp: f32,
    /// Expected HP saved by using the shop for card removal (differential simulation).
    pub shop_hp_value: f32,
}

// ── Event filtering ───────────────────────────────────────────────────────────

/// Map a sub-act name to the game's act number (1, 2, or 3).
/// Both Act 1 sub-acts (overgrowth + underdocks) are act number 1.
fn sub_act_to_act_number(sub_act: &str) -> u8 {
    match sub_act {
        "overgrowth" | "underdocks" => 1,
        "hive"                      => 2,
        "glory"                     => 3,
        _                           => 0,
    }
}

/// Returns `false` if any precondition string explicitly restricts to a different act
/// or requires multiplayer.
/// Non-act, non-MP preconditions (gold, HP, potions, etc.) are intentionally ignored —
/// we can only gate by act/mode without a full RunState.
fn passes_act_preconditions(preconditions: &[String], act_number: u8) -> bool {
    for prec in preconditions {
        let lower = prec.to_lowercase();
        // Multiplayer-only events: exclude from solo simulation
        if lower.contains("more than one character") || lower.contains("requires multiple") {
            return false;
        }
        // Match "act N only", "act N–M only", "act N+", "act N–M"
        let Some(rest) = lower.strip_prefix("act ") else { continue };
        // Strip trailing " only" if present
        let rest = rest.trim_end_matches(" only").trim();

        if let Some(min_str) = rest.strip_suffix('+') {
            // "N+" — requires act ≥ N
            if let Ok(min) = min_str.trim().parse::<u8>() {
                if act_number < min { return false; }
            }
        } else if rest.contains('\u{2013}') || rest.contains('-') {
            // "N–M" or "N-M" — requires N ≤ act ≤ M
            let sep = if rest.contains('\u{2013}') { '\u{2013}' } else { '-' };
            let mut parts = rest.splitn(2, sep);
            let min = parts.next().and_then(|s| s.trim().parse::<u8>().ok()).unwrap_or(0);
            let max = parts.next().and_then(|s| s.trim().parse::<u8>().ok()).unwrap_or(99);
            if act_number < min || act_number > max { return false; }
        } else if let Ok(n) = rest.parse::<u8>() {
            // "N only"
            if act_number != n { return false; }
        }
    }
    true
}

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
/// events, and Shared (null-act) events whose act preconditions permit this act.
pub fn events_for_sub_act<'a>(sub_act: &str, events: &'a [SpireApiEvent]) -> Vec<&'a SpireApiEvent> {
    let act_number = sub_act_to_act_number(sub_act);
    events
        .iter()
        .filter(|e| {
            // Ancient-type events only appear at act transitions, never in event rooms.
            if e.event_type.as_deref() == Some("Ancient") {
                return false;
            }
            // Enforce act-based preconditions (e.g. "Act 2+", "Act 1 only").
            // Non-act preconditions (gold, HP, etc.) are not checked here.
            if !passes_act_preconditions(&e.preconditions, act_number) {
                return false;
            }
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

/// Expected remaining combat encounters in the current act.
///
/// Act 3 (glory) is shorter than acts 1/2; everything else is ≈5 fights.
pub(crate) fn remaining_fights(sub_act: &str) -> f32 {
    match sub_act {
        "glory" => 3.0,
        _       => 5.0,
    }
}

/// Strip trailing punctuation so "damage." compares equal to "damage".
fn strip_punct(w: &str) -> &str {
    w.trim_end_matches(|c: char| matches!(c, '.' | ',' | ';' | ':' | '!' | '?'))
}

/// Parse HP-equivalent value for a single (already-lowercased, tag-stripped) option
/// description.
///
/// In addition to numeric HP/damage patterns, recognises:
/// - Gold gains → HP at shop conversion rate (50g ≈ 3 HP)
/// - Relic rewards → 6 HP (same as treasure room)
/// - Card removal → `removal_hp` (caller pre-computes once)
/// - Card upgrade → `upgrade_hp` (caller pre-computes once)
/// - Curse/junk cards added to deck → −5 HP each
fn parse_option_hp_delta(plain: &str, removal_hp: f32, upgrade_hp: f32) -> f32 {
    const GOLD_HP_RATE: f32 = 3.0 / 50.0; // 50 gold ≈ 3 HP at marginal shop value
    const RELIC_HP: f32 = 6.0;
    const CURSE_HP: f32 = 5.0;

    let words: Vec<&str> = plain.split_whitespace().collect();
    let mut delta = 0.0_f32;

    // ── Numeric window scan: HP, damage, gold ─────────────────────────────
    for i in 0..words.len() {
        let Ok(n) = words[i].parse::<f32>() else { continue };
        let prev  = strip_punct(if i > 0             { words[i-1] } else { "" });
        let next  = strip_punct(if i+1 < words.len() { words[i+1] } else { "" });
        let next2 = strip_punct(if i+2 < words.len() { words[i+2] } else { "" });
        let is_hp   = next  == "hp" || next  == "health";
        let is_dmg  = next  == "damage" || next  == "dmg";
        let is_hp2  = !is_hp  && (next2 == "hp" || next2 == "health");
        let is_dmg2 = !is_dmg && (next2 == "damage" || next2 == "dmg");
        let is_gold = next == "gold" || next2 == "gold";
        // Guard: "enemy takes X damage" and "deal X damage" mean the player is
        // dealing damage — don't count those as HP loss to the player.
        let subject = if i >= 2 { strip_punct(words[i - 2]) } else { "" };
        let enemy_subject = matches!(subject, "enemy" | "enemies" | "it" | "they");
        let dealer_subject = matches!(prev, "deal" | "deals");

        match prev {
            "heal" | "heals" | "restore" | "restores" => delta += n,
            "gain" if is_hp || is_hp2  => delta += n,
            "gain" if is_gold          => delta += n * GOLD_HP_RATE,
            "lose" | "loses" if is_hp || is_hp2  => delta -= n,
            "take" | "takes" if (is_dmg || is_dmg2) && !enemy_subject => delta -= n,
            _ if dealer_subject && (is_dmg || is_dmg2) && !enemy_subject => delta -= n,
            _ => {}
        }
    }

    // ── Keyword checks on the full description ─────────────────────────────
    if plain.contains("relic") {
        delta += RELIC_HP;
    }
    if plain.contains("remov") && (plain.contains(" card") || plain.contains("deck")) {
        delta += removal_hp;
    }
    if plain.contains("upgrade") {
        delta += upgrade_hp;
    }
    // Junk cards added: require "add"/"shuffle" near a known status/curse name.
    const JUNK_NAMES: &[&str] = &[" decay", " wound", " dazed", " burn ", " slimed", " curse"];
    if plain.contains("add ") || plain.contains("shuffle") {
        for name in JUNK_NAMES {
            if plain.contains(name) {
                delta -= CURSE_HP;
                break; // count at most one junk card per option
            }
        }
    }

    delta
}

// ── Card upgrade simulation ────────────────────────────────────────────────────

/// Return a copy of `card` with its primary combat effect boosted (+2 dmg / +3 block).
/// Returns `None` when the card has no boostable effect.
fn upgrade_card_copy(card: &Card) -> Option<Card> {
    if card.upgraded { return None; }
    // Only return Some when there are effects that upgrade actually changes.
    let upgradeable = card.effects.iter().any(|e| matches!(
        e,
        crate::domain::effect::CardEffect::Damage(_)
            | crate::domain::effect::CardEffect::DamageAll(_)
            | crate::domain::effect::CardEffect::Block(_)
            | crate::domain::effect::CardEffect::DamageMulti { .. }
    ));
    if !upgradeable { return None; }
    let mut c = card.clone();
    c.upgrade();
    Some(c)
}

/// Estimate the HP value of upgrading the best card in the deck over the remaining act.
///
/// Accepts a precomputed `baseline_loss` (and `has_data` flag) so the caller
/// can share the deck-stats computation already done for event parsing.
/// Falls back to `UPGRADE_HP_HEURISTIC` when no encounter data is available.
fn best_upgrade_hp_value(
    deck: &[Card],
    hp: u32,
    max_hp: u32,
    sub_act: &str,
    all_encounters: &[SpireApiEncounter],
    all_monsters: &[SpireApiMonster],
    relics: &[String],
    baseline_loss: f32,
    has_data: bool,
    ascension: u8,
    rng: &mut impl Rng,
) -> f32 {
    const UPGRADE_HP_HEURISTIC: f32 = 5.0;
    let _ = baseline_loss; // only used for the has_data guard; chain sim recomputes
    if deck.is_empty() || !has_data {
        return UPGRADE_HP_HEURISTIC;
    }
    let best_idx = deck
        .iter()
        .enumerate()
        .filter(|(_, c)| c.cost < 255 && upgrade_card_copy(c).is_some())
        .max_by(|(_, a), (_, b)| {
            card_value_score(a)
                .partial_cmp(&card_value_score(b))
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .map(|(i, _)| i);
    let Some(idx) = best_idx else {
        return UPGRADE_HP_HEURISTIC;
    };
    let Some(upgraded_card) = upgrade_card_copy(&deck[idx]) else {
        return UPGRADE_HP_HEURISTIC;
    };
    let mut upgraded_deck = deck.to_vec();
    upgraded_deck[idx] = upgraded_card;
    let n_fights = remaining_fights(sub_act) as u32;
    let base_chain = crate::metrics::deck_dash::simulate_chained_hp_loss(
        deck, hp, max_hp, sub_act, all_encounters, all_monsters, relics, n_fights, 4, ascension, rng,
    );
    let upgraded_chain = crate::metrics::deck_dash::simulate_chained_hp_loss(
        &upgraded_deck, hp, max_hp, sub_act, all_encounters, all_monsters, relics, n_fights, 4, ascension, rng,
    );
    (base_chain - upgraded_chain).max(0.0)
}

/// Compute the mean best-option HP delta across the act event pool.
///
/// Pre-computes once per call:
/// - `removal_hp` — HP value of removing the deck's worst card
/// - `upgrade_hp` — HP value of upgrading the deck's best card (simulated when possible)
///
/// Both values are used by `parse_option_hp_delta` for options that mention card
/// removal or upgrade.
pub fn compute_event_hp_delta(
    events: &[SpireApiEvent],
    sub_act: &str,
    deck: &[Card],
    hp: u32,
    max_hp: u32,
    all_encounters: &[SpireApiEncounter],
    all_monsters: &[SpireApiMonster],
    relics: &[String],
    ascension: u8,
    rng: &mut impl Rng,
) -> f32 {
    let pool = events_for_sub_act(sub_act, events);
    if pool.is_empty() {
        return -2.0;
    }

    // Pre-compute deck-dependent values once, shared across all options.
    let baseline = compute_deck_stats(deck, hp, max_hp, sub_act, all_encounters, all_monsters, relics, ascension, rng);
    let has_data = baseline.encounter_count > 0;

    let removal_hp = crate::metrics::shop_ev::compute_removal_hp_value(
        deck, hp, max_hp, sub_act, all_encounters, all_monsters, relics,
        baseline.mean_hp_loss, has_data, ascension, rng,
    );

    // Only simulate upgrade value when at least one event in the pool mentions it.
    let pool_text_mentions_upgrade = pool.iter().any(|e| {
        e.options.iter().any(|o| {
            o.description.as_deref().unwrap_or("").to_lowercase().contains("upgrade")
        })
    });
    let upgrade_hp = if pool_text_mentions_upgrade {
        best_upgrade_hp_value(deck, hp, max_hp, sub_act, all_encounters, all_monsters, relics,
            baseline.mean_hp_loss, has_data, ascension, rng)
    } else {
        5.0 // heuristic fallback; never used unless an option mentions "upgrade"
    };

    // For each event, find the best available option's HP delta; average over pool.
    let total: f32 = pool.iter().map(|event| {
        event.options.iter()
            .map(|opt| {
                let raw = opt.description.as_deref().unwrap_or("");
                let plain = strip_tags(raw).to_lowercase();
                parse_option_hp_delta(&plain, removal_hp, upgrade_hp)
            })
            .fold(f32::NEG_INFINITY, f32::max)
            .max(-50.0)
    }).sum::<f32>();
    total / pool.len() as f32
}

// ── Card value scoring ────────────────────────────────────────────────────────

pub(crate) fn card_value_score(card: &Card) -> f32 {
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
/// Below this ascension level the ancient heals to full; at or above it heals 80% of missing HP.
pub const POST_ACT_HEAL_ASCENSION_THRESHOLD: u8 = 2;

pub fn post_act_heal_hp(current_hp: u32, max_hp: u32, ascension: u8) -> f32 {
    if ascension < POST_ACT_HEAL_ASCENSION_THRESHOLD {
        max_hp as f32  // full heal
    } else {
        // A2+: Ancients heal 80% of missing HP
        let missing = max_hp.saturating_sub(current_hp) as f32;
        (current_hp as f32 + (missing * 0.80).floor()).min(max_hp as f32)
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
    relics: &[String],
    class_cards: &[Card],
    rng: &mut impl Rng,
) -> MapEvData {
    // Pre-filter by sub-act once; room-type sub-filters below are then cheap.
    let act_enc: Vec<&SpireApiEncounter> = all_encounters
        .iter()
        .filter(|e| e.act.as_deref().map(crate::domain::encounter::normalize_act).unwrap_or("other") == sub_act)
        .collect();

    let by_room = |room: &str| -> Vec<SpireApiEncounter> {
        act_enc.iter()
            .filter(|e| e.room_type.as_deref() == Some(room))
            .map(|e| (*e).clone())
            .collect()
    };

    // Simulate vs. normal (Monster) encounters for this sub-act.
    let normal_enc = by_room("Monster");
    let normal_stats =
        compute_deck_stats(deck, hp, max_hp, sub_act, &normal_enc, all_monsters, relics, ascension, rng);

    // Simulate vs. elite encounters — elites give a guaranteed relic so the
    // star bonus reflects the relic reward even at moderate HP risk.
    let elite_enc = by_room("Elite");
    let elite_stats =
        compute_deck_stats(deck, hp, max_hp, sub_act, &elite_enc, all_monsters, relics, ascension, rng);

    // Simulate vs. boss encounters for this sub-act.
    let boss_enc = by_room("Boss");
    let boss_stats =
        compute_deck_stats(deck, hp, max_hp, sub_act, &boss_enc, all_monsters, relics, ascension, rng);

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

    let event_hp_delta = compute_event_hp_delta(
        all_events, sub_act, deck, hp, max_hp, all_encounters, all_monsters, relics, ascension, rng,
    );
    let treasure_hp = 6.0_f32;
    let shop_hp_value = crate::metrics::shop_ev::compute_shop_total_value(
        deck, hp, max_hp, sub_act, all_encounters, all_monsters, gold, relics, class_cards, ascension, rng,
    );

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
        use crate::data::api::SpireApiEvent;
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
    fn events_for_sub_act_excludes_act_gated_shared_events() {
        use crate::data::api::SpireApiEvent;
        fn make_shared(id: &str, preconditions: Vec<String>) -> SpireApiEvent {
            SpireApiEvent {
                id: id.into(),
                name: id.into(),
                event_type: Some("Shared".into()),
                act: None,
                description: None,
                preconditions,
                options: vec![],
                pages: vec![],
                relics: vec![],
                epithet: None,
                dialogue: None,
                image_url: None,
            }
        }
        let events = vec![
            make_shared("ALL_ACTS",   vec![]),
            make_shared("ACT2_PLUS",  vec!["Act 2+".into()]),
            make_shared("ACT1_2",     vec!["Act 1\u{2013}2 only".into()]),
            make_shared("ACT2_ONLY",  vec!["Act 2 only".into()]),
            make_shared("ACT1_ONLY",  vec!["Act 1 only".into()]),
        ];

        // Act 1 (overgrowth): ALL_ACTS + ACT1_2 + ACT1_ONLY = 3
        let og = events_for_sub_act("overgrowth", &events);
        assert_eq!(og.len(), 3, "overgrowth got {:?}", og.iter().map(|e| &e.id).collect::<Vec<_>>());

        // Act 2 (hive): ALL_ACTS + ACT2_PLUS + ACT1_2 + ACT2_ONLY = 4
        let hive = events_for_sub_act("hive", &events);
        assert_eq!(hive.len(), 4, "hive got {:?}", hive.iter().map(|e| &e.id).collect::<Vec<_>>());

        // Act 3 (glory): ALL_ACTS + ACT2_PLUS = 2
        let glory = events_for_sub_act("glory", &events);
        assert_eq!(glory.len(), 2, "glory got {:?}", glory.iter().map(|e| &e.id).collect::<Vec<_>>());
    }

    #[test]
    fn compute_map_ev_no_data() {
        use rand::SeedableRng;
        use rand::rngs::StdRng;
        let mut rng = StdRng::seed_from_u64(1);
        let deck = crate::domain::catalog::ironclad::starter_deck();
        let data = compute_map_ev(&deck, 80, 80, 0, "overgrowth", &[], &[], &[], 0, &[], &[], &mut rng);
        assert_eq!(data.sub_act, "overgrowth");
        assert_eq!(data.treasure.stars, 3);
        assert_eq!(data.events.len(), 0);
    }

    // ── parse_option_hp_delta tests ──────────────────────────────────────────

    #[test]
    fn parse_heal_hp() {
        assert_eq!(parse_option_hp_delta("heal 10 hp", 0.0, 0.0), 10.0);
    }

    #[test]
    fn parse_damage_taken() {
        assert_eq!(parse_option_hp_delta("take 8 damage", 0.0, 0.0), -8.0);
    }

    #[test]
    fn parse_gold_gain() {
        // 50g × (3/50) = 3.0 HP
        let v = parse_option_hp_delta("gain 50 gold", 0.0, 0.0);
        assert!((v - 3.0).abs() < 0.01, "expected ~3.0, got {v}");
    }

    #[test]
    fn parse_relic_reward() {
        let v = parse_option_hp_delta("obtain a relic", 0.0, 0.0);
        assert_eq!(v, 6.0);
    }

    #[test]
    fn parse_relic_and_damage() {
        // "take 5 damage. gain a relic." → -5 + 6 = 1
        let v = parse_option_hp_delta("take 5 damage. gain a relic.", 0.0, 0.0);
        assert!((v - 1.0).abs() < 0.01, "expected ~1.0, got {v}");
    }

    #[test]
    fn parse_card_removal() {
        let v = parse_option_hp_delta("remove a card from your deck", 8.0, 0.0);
        assert_eq!(v, 8.0);
    }

    #[test]
    fn parse_card_upgrade() {
        let v = parse_option_hp_delta("upgrade a card in your deck", 0.0, 5.0);
        assert_eq!(v, 5.0);
    }

    #[test]
    fn parse_curse_added() {
        let v = parse_option_hp_delta("add a decay to your deck", 0.0, 0.0);
        assert_eq!(v, -5.0);
    }

    #[test]
    fn parse_curse_wound() {
        let v = parse_option_hp_delta("shuffle a wound into your deck", 0.0, 0.0);
        assert_eq!(v, -5.0);
    }

    #[test]
    fn parse_no_keywords_returns_zero() {
        let v = parse_option_hp_delta("you feel a chill in the air.", 0.0, 0.0);
        assert_eq!(v, 0.0);
    }

    #[test]
    fn upgrade_card_copy_boosts_damage() {
        use crate::domain::card::{CardType, Rarity};
        use crate::domain::effect::CardEffect;
        let card = crate::domain::card::Card::new(
            1, "Strike", 1, CardType::Attack, Rarity::Basic,
            vec![CardEffect::Damage(6)],
        );
        let upgraded = upgrade_card_copy(&card).unwrap();
        assert_eq!(upgraded.base_damage(), 9);
    }

    #[test]
    fn upgrade_card_copy_boosts_block() {
        use crate::domain::card::{CardType, Rarity};
        use crate::domain::effect::CardEffect;
        let card = crate::domain::card::Card::new(
            2, "Defend", 1, CardType::Skill, Rarity::Basic,
            vec![CardEffect::Block(5)],
        );
        let upgraded = upgrade_card_copy(&card).unwrap();
        assert_eq!(upgraded.base_block(), 8);
    }

    #[test]
    fn upgrade_card_copy_returns_none_for_passive() {
        use crate::domain::card::{CardType, Rarity};
        use crate::domain::effect::CardEffect;
        let card = crate::domain::card::Card::new(
            3, "Flex", 1, CardType::Skill, Rarity::Common,
            vec![CardEffect::Passive("At the start of your turn, gain 2 Strength.".into())],
        );
        assert!(upgrade_card_copy(&card).is_none());
    }
}
