use crate::data::api::ApiEventOption;
use crate::metrics::map_ev::strip_tags;

#[derive(Debug, Clone)]
pub struct EventOptionAdvice {
    /// Index into the event's `options` vec.
    pub option_idx: usize,
    pub score: f32,
    pub reason: String,
}

/// Score each option in an event heuristically.
///
/// `hp_ratio` = current HP / max HP (0.0–1.0).
/// Returns a vec sorted best-first.
pub fn advise_event(options: &[ApiEventOption], hp_ratio: f32) -> Vec<EventOptionAdvice> {
    let mut advice: Vec<EventOptionAdvice> = options
        .iter()
        .enumerate()
        .map(|(i, opt)| {
            let raw = opt.description.as_deref().unwrap_or("");
            let plain = strip_tags(raw).to_lowercase();
            let (score, reason) = score_option(&plain, hp_ratio);
            EventOptionAdvice { option_idx: i, score, reason }
        })
        .collect();

    advice.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
    advice
}

fn score_option(plain: &str, hp_ratio: f32) -> (f32, String) {
    let mut score = 0.0_f32;
    let mut tags: Vec<&str> = Vec::new();

    // Healing / HP gain — more valuable at low HP.
    if plain.contains("heal") {
        let gain = 1.5 * (2.0 - hp_ratio);
        score += gain;
        tags.push("heals");
    }
    if (plain.contains("gain") || plain.contains("increase")) && plain.contains("max hp") {
        score += 1.2;
        tags.push("+max hp");
    } else if plain.contains("gain") && plain.contains("hp") {
        score += 1.0 * (2.0 - hp_ratio);
        tags.push("+hp");
    }

    // Gold / relics.
    if plain.contains("gain") && plain.contains("gold") {
        score += 0.5;
        tags.push("+gold");
    }
    if plain.contains("relic") && !plain.contains("lose") {
        score += 1.5;
        tags.push("+relic");
    }

    // Deck manipulation.
    if plain.contains("remove") && plain.contains("card") {
        score += 1.2;
        tags.push("remove card");
    }
    if plain.contains("upgrade") {
        score += 0.9;
        tags.push("upgrade");
    }
    if plain.contains("choose") && plain.contains("card") && !plain.contains("remove") {
        score += 0.4;
        tags.push("+card");
    }

    // Damage / HP loss — worse at low HP.
    if plain.contains("take") && plain.contains("damage") {
        let penalty = 1.2 * (1.0 + (1.0 - hp_ratio));
        score -= penalty;
        tags.push("take dmg");
    }
    if (plain.contains("lose") || plain.contains("loss")) && plain.contains("hp") {
        let penalty = 1.0 * (1.0 + (1.0 - hp_ratio));
        score -= penalty;
        tags.push("lose hp");
    }
    if plain.contains("lose") && plain.contains("gold") {
        score -= 0.4;
        tags.push("lose gold");
    }

    // Curses / permanent downsides.
    if plain.contains("curse") {
        score -= 2.0;
        tags.push("curse");
    }

    let reason = if tags.is_empty() {
        "no clear outcome".to_string()
    } else {
        tags.join(", ")
    };

    (score, reason)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::api::ApiEventOption;

    fn opt(id: &str, desc: &str) -> ApiEventOption {
        ApiEventOption {
            id: id.into(),
            title: Some(id.into()),
            description: Some(desc.into()),
        }
    }

    #[test]
    fn heal_preferred_at_low_hp() {
        let options = vec![
            opt("A", "Heal [green]10[/green] HP."),
            opt("B", "Take [red]5[/red] damage."),
        ];
        let advice = advise_event(&options, 0.30);
        assert_eq!(advice[0].option_idx, 0, "Heal should rank first at low HP");
    }

    #[test]
    fn curse_ranks_last() {
        let options = vec![
            opt("A", "Gain a Curse."),
            opt("B", "Lose 5 gold."),
        ];
        let advice = advise_event(&options, 0.80);
        assert_eq!(advice.last().unwrap().option_idx, 0, "Curse should rank last");
    }

    #[test]
    fn relic_ranks_high() {
        let options = vec![
            opt("A", "Gain 30 gold."),
            opt("B", "Gain a relic."),
        ];
        let advice = advise_event(&options, 0.80);
        assert_eq!(advice[0].option_idx, 1, "Relic should rank first over gold");
    }

    #[test]
    fn sorted_descending() {
        let options = vec![
            opt("A", "Gain a Curse."),
            opt("B", "Heal 5 HP."),
            opt("C", "Gain a relic."),
        ];
        let advice = advise_event(&options, 0.70);
        for w in advice.windows(2) {
            assert!(w[0].score >= w[1].score);
        }
    }

    #[test]
    fn empty_options_returns_empty() {
        let advice = advise_event(&[], 0.80);
        assert!(advice.is_empty());
    }
}
