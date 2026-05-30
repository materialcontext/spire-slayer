use crate::domain::run::Potion;

pub fn potion_hp_value(id: &str) -> f32 {
    match id {
        "ATTACK_POTION" | "FIRE_POTION" | "EXPLOSIVE_AMPOULE" |
        "STRENGTH_POTION" | "SWIFT_POTION" | "CULTIST_POTION" => 18.0,
        "POISON_POTION" | "FEAR_POTION" | "POWER_POTION" | "ANCIENT_POTION" => 12.0,
        "BLOCK_POTION" | "DEXTERITY_POTION" | "ENTROPIC_BREW" => 10.0,
        "FRUIT_JUICE" => 8.0,
        "FAIRY_IN_A_BOTTLE" => 50.0,
        _ => 6.0,
    }
}

/// Returns a recommendation string if the potion should be used now, or None to save it.
pub fn advise_potion_use(
    potion: &Potion,
    hp: u32, max_hp: u32,
    is_boss: bool, is_elite: bool,
) -> Option<String> {
    // Fairy auto-triggers on death — never recommend consuming
    if potion.id == "FAIRY_IN_A_BOTTLE" { return None; }

    let value = potion_hp_value(&potion.id);
    let hp_ratio = hp as f32 / max_hp as f32;
    let is_defensive = matches!(potion.id.as_str(),
        "BLOCK_POTION" | "DEXTERITY_POTION" | "FRUIT_JUICE" | "ENTROPIC_BREW");

    if hp_ratio < 0.25 && is_defensive {
        return Some(format!("Use {} — HP critical ({}/{})", potion.name, hp, max_hp));
    }
    if (is_boss || is_elite) && value >= 12.0 {
        let ctx = if is_boss { "boss" } else { "elite" };
        return Some(format!("Use {} — high value in {} fight", potion.name, ctx));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::run::Potion;

    #[test]
    fn fairy_never_recommended() {
        let p = Potion { id: "FAIRY_IN_A_BOTTLE".into(), name: "Fairy in a Bottle".into(), description: String::new() };
        assert!(advise_potion_use(&p, 10, 80, true, false).is_none());
    }

    #[test]
    fn offensive_potion_recommended_vs_boss() {
        let p = Potion { id: "FIRE_POTION".into(), name: "Fire Potion".into(), description: String::new() };
        assert!(advise_potion_use(&p, 60, 80, true, false).is_some());
    }

    #[test]
    fn offensive_potion_not_recommended_vs_normal() {
        let p = Potion { id: "FIRE_POTION".into(), name: "Fire Potion".into(), description: String::new() };
        assert!(advise_potion_use(&p, 60, 80, false, false).is_none());
    }
}
