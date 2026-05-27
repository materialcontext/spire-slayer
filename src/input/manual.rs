use crate::domain::card::{Card, CardType, Rarity};
use crate::domain::catalog::ironclad;
use crate::domain::combat::{CombatState, EnemyState, Intent, PlayerState};
use crate::domain::effect::CardEffect;

#[derive(Debug, Clone, PartialEq)]
pub enum InputField {
    PlayerHp,
    Energy,
    Turn,
    DrawPileSize,
    EnemyHp { index: usize },
    EnemyBlock { index: usize },
    EnemyIntent { index: usize },
    Done,
}

pub struct ManualInputState {
    pub current_field: InputField,
    pub buffer: String,
    pub base: CombatState,
    enemy_count: usize,
}

impl ManualInputState {
    pub fn new(base: CombatState) -> Self {
        let enemy_count = base.enemies.len();
        Self {
            current_field: InputField::PlayerHp,
            buffer: String::new(),
            base,
            enemy_count,
        }
    }

    pub fn handle_char(&mut self, c: char) {
        self.buffer.push(c);
    }

    pub fn handle_backspace(&mut self) {
        self.buffer.pop();
    }

    pub fn commit(&mut self) -> Result<(), String> {
        let result = self.apply_buffer();
        if result.is_ok() {
            self.buffer.clear();
            self.advance();
        }
        result
    }

    pub fn next_field(&mut self) {
        self.buffer.clear();
        self.advance();
    }

    pub fn is_complete(&self) -> bool {
        self.current_field == InputField::Done
    }

    pub fn build(self) -> CombatState {
        self.base
    }

    pub fn field_label(&self) -> String {
        match &self.current_field {
            InputField::PlayerHp => format!("Player HP (current/max, e.g. 72/80):"),
            InputField::Energy => format!("Energy (current/max, e.g. 3/3):"),
            InputField::Turn => format!("Turn number:"),
            InputField::DrawPileSize => format!("Draw pile size:"),
            InputField::EnemyHp { index } => {
                let name = self.base.enemies.get(*index).map(|e| e.name.as_str()).unwrap_or("?");
                format!("Enemy {} '{}' HP (current/max):", index, name)
            }
            InputField::EnemyBlock { index } => {
                let name = self.base.enemies.get(*index).map(|e| e.name.as_str()).unwrap_or("?");
                format!("Enemy {} '{}' block:", index, name)
            }
            InputField::EnemyIntent { index } => {
                let name = self.base.enemies.get(*index).map(|e| e.name.as_str()).unwrap_or("?");
                format!("Enemy {} '{}' intent (A12, AM5x3, B, U, D, E, ?):", index, name)
            }
            InputField::Done => "Done".to_string(),
        }
    }

    fn apply_buffer(&mut self) -> Result<(), String> {
        let buf = self.buffer.trim().to_string();
        match &self.current_field.clone() {
            InputField::PlayerHp => {
                let (hp, max_hp) = parse_hp_pair(&buf)?;
                self.base.player.hp = hp;
                self.base.player.max_hp = max_hp;
            }
            InputField::Energy => {
                let (energy, energy_max) = parse_u8_pair(&buf)?;
                self.base.energy = energy;
                self.base.energy_max = energy_max;
            }
            InputField::Turn => {
                let t = buf.parse::<u32>().map_err(|_| format!("expected number, got '{buf}'"))?;
                self.base.turn = t;
            }
            InputField::DrawPileSize => {
                let n = buf.parse::<usize>().map_err(|_| format!("expected number, got '{buf}'"))?;
                self.base.draw_pile.truncate(n);
                while self.base.draw_pile.len() < n {
                    let filler = make_filler_card(self.base.draw_pile.len());
                    self.base.draw_pile.push(filler);
                }
            }
            InputField::EnemyHp { index } => {
                let idx = *index;
                if idx >= self.base.enemies.len() {
                    return Err(format!("no enemy at index {idx}"));
                }
                let (hp, max_hp) = parse_hp_pair(&buf)?;
                self.base.enemies[idx].hp = hp;
                self.base.enemies[idx].max_hp = max_hp;
            }
            InputField::EnemyBlock { index } => {
                let idx = *index;
                if idx >= self.base.enemies.len() {
                    return Err(format!("no enemy at index {idx}"));
                }
                let block = buf.parse::<u32>().map_err(|_| format!("expected number, got '{buf}'"))?;
                self.base.enemies[idx].block = block;
            }
            InputField::EnemyIntent { index } => {
                let idx = *index;
                if idx >= self.base.enemies.len() {
                    return Err(format!("no enemy at index {idx}"));
                }
                let intent = parse_intent(&buf)?;
                self.base.enemies[idx].intent = intent;
            }
            InputField::Done => {}
        }
        Ok(())
    }

    fn advance(&mut self) {
        self.current_field = match &self.current_field {
            InputField::PlayerHp => InputField::Energy,
            InputField::Energy => InputField::Turn,
            InputField::Turn => InputField::DrawPileSize,
            InputField::DrawPileSize => {
                if self.enemy_count > 0 {
                    InputField::EnemyHp { index: 0 }
                } else {
                    InputField::Done
                }
            }
            InputField::EnemyHp { index } => InputField::EnemyBlock { index: *index },
            InputField::EnemyBlock { index } => InputField::EnemyIntent { index: *index },
            InputField::EnemyIntent { index } => {
                let next = index + 1;
                if next < self.enemy_count {
                    InputField::EnemyHp { index: next }
                } else {
                    InputField::Done
                }
            }
            InputField::Done => InputField::Done,
        };
    }
}

fn parse_hp_pair(s: &str) -> Result<(u32, u32), String> {
    if let Some((a, b)) = s.split_once('/') {
        let hp = a.trim().parse::<u32>().map_err(|_| format!("bad HP value '{}'", a.trim()))?;
        let max = b.trim().parse::<u32>().map_err(|_| format!("bad max HP value '{}'", b.trim()))?;
        Ok((hp, max))
    } else {
        let hp = s.parse::<u32>().map_err(|_| format!("expected HP or HP/max, got '{s}'"))?;
        Ok((hp, hp))
    }
}

fn parse_u8_pair(s: &str) -> Result<(u8, u8), String> {
    if let Some((a, b)) = s.split_once('/') {
        let v = a.trim().parse::<u8>().map_err(|_| format!("bad value '{}'", a.trim()))?;
        let m = b.trim().parse::<u8>().map_err(|_| format!("bad max value '{}'", b.trim()))?;
        Ok((v, m))
    } else {
        let v = s.parse::<u8>().map_err(|_| format!("expected number or n/max, got '{s}'"))?;
        Ok((v, v))
    }
}

/// Parse intent shorthand:
/// - `A12`     → Attack(12)
/// - `AM5x3`   → AttackMulti { damage: 5, hits: 3 }
/// - `B`       → Block
/// - `U`       → Buff
/// - `D`       → DebuffPlayer
/// - `E`       → Escape
/// - `?`       → Unknown
pub fn parse_intent(s: &str) -> Result<Intent, String> {
    let s = s.trim();
    if s.eq_ignore_ascii_case("B") {
        return Ok(Intent::Block);
    }
    if s.eq_ignore_ascii_case("U") {
        return Ok(Intent::Buff);
    }
    if s.eq_ignore_ascii_case("D") {
        return Ok(Intent::DebuffPlayer);
    }
    if s.eq_ignore_ascii_case("E") {
        return Ok(Intent::Escape);
    }
    if s == "?" {
        return Ok(Intent::Unknown);
    }
    // AttackMulti: AM<damage>x<hits>
    if s.len() >= 2 && s[..2].eq_ignore_ascii_case("AM") {
        let rest = &s[2..];
        if let Some((dmg_str, hits_str)) = rest.split_once('x') {
            let damage = dmg_str.parse::<u32>().map_err(|_| format!("bad damage in '{s}'"))?;
            let hits = hits_str.parse::<u32>().map_err(|_| format!("bad hits in '{s}'"))?;
            return Ok(Intent::AttackMulti { damage, hits });
        }
        return Err(format!("expected AM<damage>x<hits>, got '{s}'"));
    }
    // Attack: A<damage>
    if s.len() >= 2 && s[..1].eq_ignore_ascii_case("A") {
        let rest = &s[1..];
        let damage = rest.parse::<u32>().map_err(|_| format!("bad damage in '{s}'"))?;
        return Ok(Intent::Attack(damage));
    }
    Err(format!("unknown intent shorthand '{s}'; use A12, AM5x3, B, U, D, E, or ?"))
}

fn make_filler_card(idx: usize) -> Card {
    Card::new(
        (1000 + idx) as u32,
        "Strike",
        1,
        CardType::Attack,
        Rarity::Basic,
        vec![CardEffect::Damage(6)],
    )
}

pub fn default_combat_state() -> CombatState {
    let player = PlayerState::new(80, 80);
    let enemy = EnemyState::new("Cultist", 50, Intent::Attack(9));
    let deck = ironclad::starter_deck();
    let mut state = CombatState::new(player, vec![enemy], deck);
    // Draw an opening hand
    let hand: Vec<Card> = state.draw_pile.drain(..5.min(state.draw_pile.len())).collect();
    state.hand = hand;
    state
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> CombatState {
        default_combat_state()
    }

    #[test]
    fn player_hp_pair() {
        let mut s = ManualInputState::new(base());
        "72/80".chars().for_each(|c| s.handle_char(c));
        s.commit().unwrap();
        assert_eq!(s.base.player.hp, 72);
        assert_eq!(s.base.player.max_hp, 80);
    }

    #[test]
    fn player_hp_single_value() {
        let mut s = ManualInputState::new(base());
        "75".chars().for_each(|c| s.handle_char(c));
        s.commit().unwrap();
        assert_eq!(s.base.player.hp, 75);
        assert_eq!(s.base.player.max_hp, 75);
    }

    #[test]
    fn energy_pair() {
        let mut s = ManualInputState::new(base());
        s.next_field(); // skip PlayerHp
        "2/3".chars().for_each(|c| s.handle_char(c));
        s.commit().unwrap();
        assert_eq!(s.base.energy, 2);
        assert_eq!(s.base.energy_max, 3);
    }

    #[test]
    fn turn_field() {
        let mut s = ManualInputState::new(base());
        s.next_field(); // PlayerHp
        s.next_field(); // Energy
        "5".chars().for_each(|c| s.handle_char(c));
        s.commit().unwrap();
        assert_eq!(s.base.turn, 5);
    }

    #[test]
    fn enemy_intent_attack() {
        assert_eq!(parse_intent("A9").unwrap(), Intent::Attack(9));
    }

    #[test]
    fn enemy_intent_attack_multi() {
        assert_eq!(parse_intent("AM5x3").unwrap(), Intent::AttackMulti { damage: 5, hits: 3 });
    }

    #[test]
    fn enemy_intent_block() {
        assert_eq!(parse_intent("B").unwrap(), Intent::Block);
    }

    #[test]
    fn enemy_intent_buff() {
        assert_eq!(parse_intent("U").unwrap(), Intent::Buff);
    }

    #[test]
    fn enemy_intent_unknown() {
        assert_eq!(parse_intent("?").unwrap(), Intent::Unknown);
    }

    #[test]
    fn enemy_intent_escape() {
        assert_eq!(parse_intent("E").unwrap(), Intent::Escape);
    }

    #[test]
    fn enemy_intent_invalid() {
        assert!(parse_intent("X99").is_err());
    }

    #[test]
    fn backspace_removes_char() {
        let mut s = ManualInputState::new(base());
        s.handle_char('7');
        s.handle_char('2');
        s.handle_backspace();
        assert_eq!(s.buffer, "7");
    }

    #[test]
    fn next_field_skips_without_committing() {
        let mut s = ManualInputState::new(base());
        let original_hp = s.base.player.hp;
        s.handle_char('1');
        s.next_field();
        assert_eq!(s.base.player.hp, original_hp);
        assert_eq!(s.current_field, InputField::Energy);
    }

    #[test]
    fn complete_cycle_ends_at_done() {
        let mut s = ManualInputState::new(base());
        // Cycle through all fields with next_field (skip)
        while !s.is_complete() {
            s.next_field();
        }
        assert!(s.is_complete());
    }

    #[test]
    fn enemy_hp_field() {
        let mut s = ManualInputState::new(base());
        // Advance to EnemyHp[0]: PlayerHp, Energy, Turn, DrawPileSize
        for _ in 0..4 {
            s.next_field();
        }
        assert_eq!(s.current_field, InputField::EnemyHp { index: 0 });
        "48/50".chars().for_each(|c| s.handle_char(c));
        s.commit().unwrap();
        assert_eq!(s.base.enemies[0].hp, 48);
        assert_eq!(s.base.enemies[0].max_hp, 50);
    }
}
