use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum BuffType {
    // Player/enemy positive buffs
    Strength,
    Dexterity,
    Thorns,
    Plating,
    Ritual,
    /// Defect's Focus: adds to all orb passive and evoke values.
    Focus,
    // Player-only persistent power buffs
    /// Block is not cleared at the start of your turn.
    Barricade,
    /// Incoming damage is capped at 1 per hit; decrements by 1 per turn.
    Intangible,
    /// Apply (stacks) Poison to all enemies at the start of each player turn.
    NoxiousFumes,
    // Debuffs
    Vulnerable,
    Weak,
    Frail,
    Poison,
}

/// Defect orb types. Dark tracks accumulated damage value; Glass tracks passive value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum OrbType {
    Lightning,
    Frost,
    /// Accumulated damage (starts at 6, grows by 6+focus per passive trigger).
    Dark(u32),
    Plasma,
    /// Current passive damage per turn (evoke = 2×this; decrements by 1 per trigger).
    Glass(u32),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum CardEffect {
    /// Deal damage to the target enemy.
    Damage(u32),
    /// Deal damage to all enemies.
    DamageAll(u32),
    /// Deal damage N times to the target enemy.
    DamageMulti { base: u32, hits: u32 },
    /// Deal damage equal to the player's current Block (e.g. Body Slam).
    DamageEqBlock,
    /// Gain block.
    Block(u32),
    /// Draw cards.
    Draw(u32),
    /// Gain energy.
    GainEnergy(u32),
    /// Lose HP (e.g. Hemokinesis, Offering). Player self-damage.
    LoseHp(u32),
    /// Enemy loses HP directly — bypasses block and is unaffected by Strength,
    /// Weak, or Vulnerable (e.g. Capture Spirit, Haunt, Strangle per-card tick).
    EnemyLoseHp(u32),
    /// Apply a buff/debuff to the target enemy.
    ApplyToEnemy { buff: BuffType, stacks: i32 },
    /// Apply a buff/debuff to all enemies.
    ApplyToAllEnemies { buff: BuffType, stacks: i32 },
    /// Apply a buff/debuff to self (player).
    ApplyToSelf { buff: BuffType, stacks: i32 },
    /// Placeholder for passive/triggered effects not yet simulated.
    Passive(String),
    /// Add Stars (Regent's secondary resource). Tracked in CombatState.stars.
    GainStars(u32),
    /// Channel an orb into the Defect's orb queue (evokes leftmost if slots full).
    ChannelOrb(OrbType),
    /// Evoke the rightmost orb N times (u32::MAX = evoke all).
    EvokeOrb(u32),
}

impl CardEffect {
    pub fn damage_value(&self) -> u32 {
        match self {
            CardEffect::Damage(d) => *d,
            CardEffect::DamageAll(d) => *d,
            CardEffect::DamageMulti { base, hits } => base * hits,
            _ => 0,
        }
    }

    pub fn block_value(&self) -> u32 {
        match self {
            CardEffect::Block(b) => *b,
            _ => 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn damage_value_multi() {
        let e = CardEffect::DamageMulti { base: 5, hits: 3 };
        assert_eq!(e.damage_value(), 15);
    }

    #[test]
    fn buff_type_hash() {
        use std::collections::HashMap;
        let mut map: HashMap<BuffType, i32> = HashMap::new();
        map.insert(BuffType::Strength, 3);
        assert_eq!(map[&BuffType::Strength], 3);
    }
}
