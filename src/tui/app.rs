use crossterm::event::{KeyCode, KeyEvent};
use rand::SeedableRng;
use rand::rngs::StdRng;

use crate::data::api::{SpireApiCharacter, SpireApiEncounter, SpireApiEvent, SpireApiMonster};
use crate::domain::card::Card;
use crate::domain::catalog;
use crate::domain::combat::CombatState;
use crate::domain::encounter::{encounter_to_combat, encounters_for_act};
use crate::domain::run::{PlayerClass, RunState, starting_relics};
use crate::input::event::{spawn_event_loop, AppEvent};
use crate::input::manual::{default_combat_state, ManualInputState};
use crate::metrics::card_pick::{sim_pick_score, CardAdvice};
use crate::metrics::deck_dash::{compute_deck_stats, DeckStats};
use crate::metrics::map_ev::{compute_map_ev, MapEvData};
use crate::sim::mcts::{best_play_sequence, PlayAdvice};
use crate::sim::playout::playout_n;
use crate::sim::policy::GreedyDamagePolicy;
use crate::tui::ui;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppMode {
    CharacterPick,
    EncounterPick,
    CombatAdvice,
    CardPick,
    DeckDash,
    MapEv,
    ManualInput,
    Simulating,
    Exiting,
}

pub struct App {
    pub mode: AppMode,
    pub combat: Option<CombatState>,
    pub run: Option<RunState>,
    pub play_advice: Option<PlayAdvice>,
    pub card_advice: Vec<CardAdvice>,
    pub deck_stats: Option<DeckStats>,
    pub map_ev: Option<MapEvData>,
    pub input: Option<ManualInputState>,
    pub status_message: String,
    pub selected_row: usize,
    // Character picker state
    pub characters: Vec<SpireApiCharacter>,
    // Encounter picker state
    pub encounters: Vec<SpireApiEncounter>,
    pub monsters: Vec<SpireApiMonster>,
    pub events: Vec<SpireApiEvent>,
    pub act_filter: String,
    pub filtered_indices: Vec<usize>,
}

impl App {
    pub fn new(
        characters: Vec<SpireApiCharacter>,
        encounters: Vec<SpireApiEncounter>,
        monsters: Vec<SpireApiMonster>,
        events: Vec<SpireApiEvent>,
    ) -> Self {
        // Show character picker if we have data; otherwise skip straight to encounter pick.
        let initial_mode = if characters.is_empty() {
            AppMode::EncounterPick
        } else {
            AppMode::CharacterPick
        };
        let mut app = Self {
            mode: initial_mode,
            combat: None,
            run: None,
            play_advice: None,
            card_advice: Vec::new(),
            deck_stats: None,
            map_ev: None,
            input: None,
            status_message: String::new(),
            selected_row: 0,
            characters: sort_characters(characters),
            encounters,
            monsters,
            events,
            act_filter: "overgrowth".to_string(),
            filtered_indices: Vec::new(),
        };
        app.refresh_filter();
        app
    }

    pub fn refresh_filter(&mut self) {
        let filtered = encounters_for_act(&self.encounters, &self.act_filter);
        self.filtered_indices = filtered
            .iter()
            .filter_map(|e| self.encounters.iter().position(|x| x.id == e.id))
            .collect();
        self.selected_row = 0;
    }

    pub fn current_encounter(&self) -> Option<&SpireApiEncounter> {
        self.filtered_indices
            .get(self.selected_row)
            .and_then(|&i| self.encounters.get(i))
    }

    /// Returns false when the app should quit.
    pub fn handle_event(&mut self, event: AppEvent, rng: &mut impl rand::Rng) -> bool {
        match event {
            AppEvent::Quit => return false,
            AppEvent::RunSim => {
                self.run_simulation(rng);
                return true;
            }
            AppEvent::StateUpdated(state) => {
                self.load_combat(*state);
                return true;
            }
            AppEvent::CardPickConfirmed(idx) => {
                self.selected_row = idx.min(self.card_advice.len().saturating_sub(1));
                return true;
            }
            AppEvent::Tick => return true,
            AppEvent::Key(key) => self.handle_key(key, rng),
        }
        true
    }

    fn handle_key(&mut self, key: KeyEvent, rng: &mut impl rand::Rng) {
        match self.mode {
            AppMode::CharacterPick => self.handle_key_character(key),
            AppMode::EncounterPick => self.handle_key_encounter(key, rng),
            AppMode::ManualInput => self.handle_key_input(key),
            AppMode::CombatAdvice => self.handle_key_combat(key, rng),
            AppMode::CardPick => self.handle_key_pick(key),
            AppMode::DeckDash => self.handle_key_deck_dash(key),
            AppMode::MapEv => self.handle_key_map_ev(key),
            AppMode::Simulating | AppMode::Exiting => {}
        }
    }

    fn handle_key_character(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('q') => {
                self.mode = AppMode::Exiting;
            }
            KeyCode::Char('j') | KeyCode::Down => {
                if self.selected_row + 1 < self.characters.len() {
                    self.selected_row += 1;
                }
            }
            KeyCode::Char('k') | KeyCode::Up => {
                if self.selected_row > 0 {
                    self.selected_row -= 1;
                }
            }
            KeyCode::Enter => {
                self.select_character();
            }
            _ => {}
        }
    }

    fn handle_key_encounter(&mut self, key: KeyEvent, rng: &mut impl rand::Rng) {
        match key.code {
            KeyCode::Char('q') => {
                self.mode = AppMode::Exiting;
            }
            KeyCode::Esc => {
                if !self.characters.is_empty() {
                    self.selected_row = 0;
                    self.mode = AppMode::CharacterPick;
                }
            }
            // Sub-act filters: o=Overgrowth, u=Underdocks, h=Hive, g=Glory
            KeyCode::Char('o') | KeyCode::Char('O') => {
                self.act_filter = "overgrowth".to_string();
                self.refresh_filter();
            }
            KeyCode::Char('u') | KeyCode::Char('U') => {
                self.act_filter = "underdocks".to_string();
                self.refresh_filter();
            }
            KeyCode::Char('h') | KeyCode::Char('H') => {
                self.act_filter = "hive".to_string();
                self.refresh_filter();
            }
            KeyCode::Char('g') | KeyCode::Char('G') => {
                self.act_filter = "glory".to_string();
                self.refresh_filter();
            }
            KeyCode::Char('b') | KeyCode::Char('B') => {
                self.act_filter = "boss".to_string();
                self.refresh_filter();
            }
            KeyCode::Char('a') | KeyCode::Char('A') => {
                self.act_filter = "all".to_string();
                self.refresh_filter();
            }
            KeyCode::Char('j') | KeyCode::Down => {
                if self.selected_row + 1 < self.filtered_indices.len() {
                    self.selected_row += 1;
                }
            }
            KeyCode::Char('k') | KeyCode::Up => {
                if self.selected_row > 0 {
                    self.selected_row -= 1;
                }
            }
            KeyCode::Enter => {
                if let Some(encounter) = self.current_encounter() {
                    let encounter = encounter.clone();
                    let state = encounter_to_combat(&encounter, &self.monsters);
                    self.load_combat(state);
                    self.status_message =
                        format!("Loaded '{}'. Press [s] to simulate.", encounter.name);
                } else if self.encounters.is_empty() {
                    // No API data yet — fall back to default
                    self.load_combat(default_combat_state());
                    self.status_message =
                        "No encounter data loaded (API unavailable). Using default.".to_string();
                }
            }
            KeyCode::Char('v') => {
                self.open_map_ev(rng);
            }
            KeyCode::Char('m') => {
                // Manual input fallback
                let base = self.combat.clone().unwrap_or_else(default_combat_state);
                self.input = Some(ManualInputState::new(base));
                self.mode = AppMode::ManualInput;
                self.status_message.clear();
            }
            _ => {}
        }
    }

    fn handle_key_input(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('q') => {
                self.mode = AppMode::Exiting;
            }
            KeyCode::Esc => {
                self.mode = AppMode::EncounterPick;
                self.status_message.clear();
            }
            KeyCode::Enter => {
                if let Some(ref mut input) = self.input {
                    match input.commit() {
                        Ok(()) => {
                            self.status_message.clear();
                            if input.is_complete() {
                                let state = self.input.take().unwrap().build();
                                self.load_combat(state);
                            }
                        }
                        Err(e) => self.status_message = e,
                    }
                }
            }
            KeyCode::Tab => {
                if let Some(ref mut input) = self.input {
                    input.next_field();
                    self.status_message.clear();
                    if input.is_complete() {
                        let state = self.input.take().unwrap().build();
                        self.load_combat(state);
                    }
                }
            }
            KeyCode::Backspace => {
                if let Some(ref mut input) = self.input {
                    input.handle_backspace();
                }
            }
            KeyCode::Char(c) => {
                if let Some(ref mut input) = self.input {
                    input.handle_char(c);
                }
            }
            _ => {}
        }
    }

    fn handle_key_combat(&mut self, key: KeyEvent, rng: &mut impl rand::Rng) {
        match key.code {
            KeyCode::Char('q') => {
                self.mode = AppMode::Exiting;
            }
            KeyCode::Char('s') => {
                self.run_simulation(rng);
            }
            KeyCode::Char('e') => {
                let base = self.combat.clone().unwrap_or_else(default_combat_state);
                self.input = Some(ManualInputState::new(base));
                self.mode = AppMode::ManualInput;
                self.status_message.clear();
            }
            KeyCode::Char('n') => {
                // Back to encounter picker for next fight
                self.mode = AppMode::EncounterPick;
                self.play_advice = None;
                self.status_message.clear();
            }
            KeyCode::Char('d') => {
                self.compute_deck_dash(rng);
            }
            KeyCode::Char('p') => {
                let color = self.run.as_ref().map(|r| char_color(&r.class)).unwrap_or("ironclad");
                let offered = sample_card_offer(color, rng);
                self.load_pick(offered, rng);
            }
            KeyCode::Char('v') => {
                self.open_map_ev(rng);
            }
            KeyCode::Char('j') | KeyCode::Down => {
                if let Some(ref combat) = self.combat {
                    if self.selected_row + 1 < combat.hand.len() {
                        self.selected_row += 1;
                    }
                }
            }
            KeyCode::Char('k') | KeyCode::Up => {
                if self.selected_row > 0 {
                    self.selected_row -= 1;
                }
            }
            _ => {}
        }
    }

    fn handle_key_deck_dash(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('q') | KeyCode::Char('d') => {
                self.mode = AppMode::CombatAdvice;
            }
            _ => {}
        }
    }

    fn handle_key_map_ev(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('q') | KeyCode::Char('v') | KeyCode::Esc => {
                self.mode = if self.combat.is_some() {
                    AppMode::CombatAdvice
                } else {
                    AppMode::EncounterPick
                };
            }
            KeyCode::Char('j') | KeyCode::Down => {
                if let Some(ref data) = self.map_ev {
                    if self.selected_row + 1 < data.events.len() {
                        self.selected_row += 1;
                    }
                }
            }
            KeyCode::Char('k') | KeyCode::Up => {
                if self.selected_row > 0 {
                    self.selected_row -= 1;
                }
            }
            _ => {}
        }
    }

    fn handle_key_pick(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('q') => {
                self.mode = AppMode::CombatAdvice;
            }
            KeyCode::Char('j') | KeyCode::Down => {
                if self.selected_row + 1 < self.card_advice.len() {
                    self.selected_row += 1;
                }
            }
            KeyCode::Char('k') | KeyCode::Up => {
                if self.selected_row > 0 {
                    self.selected_row -= 1;
                }
            }
            KeyCode::Enter => {
                self.mode = AppMode::CombatAdvice;
            }
            _ => {}
        }
    }

    pub fn run_simulation(&mut self, rng: &mut impl rand::Rng) {
        let Some(ref combat) = self.combat else {
            self.status_message = "No combat state loaded".to_string();
            return;
        };
        let mut advice = best_play_sequence(combat, 500, rng);
        let stats = playout_n(combat, &GreedyDamagePolicy, 50, rng);
        advice.hp_loss_p10 = stats.hp_loss_p10;
        advice.hp_loss_p50 = stats.hp_loss_p50;
        advice.hp_loss_p90 = stats.hp_loss_p90;
        self.status_message = format!("{} simulations run", advice.simulation_count);
        self.play_advice = Some(advice);
        self.mode = AppMode::CombatAdvice;
    }

    pub fn compute_deck_dash(&mut self, rng: &mut impl rand::Rng) {
        let deck = self.run.as_ref().map(|r| r.deck.clone()).unwrap_or_else(|| {
            crate::domain::catalog::ironclad::starter_deck()
        });
        let hp = self.combat.as_ref().map(|c| c.player.hp).unwrap_or(80);
        let max_hp = self.combat.as_ref().map(|c| c.player.max_hp).unwrap_or(80);
        let sub_act = self.act_filter.clone();
        let stats = compute_deck_stats(
            &deck,
            hp,
            max_hp,
            &sub_act,
            &self.encounters,
            &self.monsters,
            rng,
        );
        self.status_message = if stats.encounter_count == 0 {
            "Deck stats (no encounter data — intrinsics only)".to_string()
        } else {
            format!(
                "Deck stats: {} encounters × {} sims",
                stats.encounter_count, stats.playout_count / stats.encounter_count as u32
            )
        };
        self.deck_stats = Some(stats);
        self.mode = AppMode::DeckDash;
    }

    pub fn open_map_ev(&mut self, rng: &mut impl rand::Rng) {
        let deck = self.run.as_ref().map(|r| r.deck.clone()).unwrap_or_else(|| {
            crate::domain::catalog::ironclad::starter_deck()
        });
        let hp = self.combat.as_ref().map(|c| c.player.hp).unwrap_or(80);
        let max_hp = self.combat.as_ref().map(|c| c.player.max_hp).unwrap_or(80);
        let data = compute_map_ev(
            &deck,
            hp,
            max_hp,
            &self.act_filter,
            &self.encounters,
            &self.monsters,
            &self.events,
            rng,
        );
        self.status_message = format!(
            "Map EV: {} — {} events ({} shared)",
            data.sub_act, data.events.len(), data.shared_event_count,
        );
        self.map_ev = Some(data);
        self.selected_row = 0;
        self.mode = AppMode::MapEv;
    }

    pub fn select_character(&mut self) {
        let Some(char_data) = self.characters.get(self.selected_row) else {
            return;
        };
        // Clone the fields we need before any mutable borrows.
        let hp = char_data.starting_hp.unwrap_or(75).max(1) as u32;
        let gold = char_data.starting_gold.unwrap_or(99) as u32;
        let deck_ids = char_data.starting_deck.clone();
        let color = char_data.color.clone().unwrap_or_default();
        let char_name = char_data.name.clone();

        let deck = catalog::deck_from_ids(&deck_ids);
        let class = class_from_color(&color);
        let relic = starting_relics::for_class(&class);

        let mut run = RunState::new(class, hp, hp, deck, relic);
        run.gold = gold;

        self.act_filter = "overgrowth".to_string();
        self.refresh_filter();

        self.status_message = format!(
            "Playing as {} — HP {}, {} starting cards",
            char_name, hp, run.deck.len(),
        );
        self.run = Some(run);
        self.selected_row = 0;
        self.mode = AppMode::EncounterPick;
    }

    pub fn load_combat(&mut self, state: CombatState) {
        self.play_advice = None;
        self.selected_row = 0;
        self.combat = Some(state);
        self.mode = AppMode::CombatAdvice;
    }

    pub fn load_pick(&mut self, offered: Vec<Card>, rng: &mut impl rand::Rng) {
        let deck = self.run.as_ref().map(|r| r.deck.clone()).unwrap_or_else(|| {
            crate::domain::catalog::ironclad::starter_deck()
        });
        let hp = self.combat.as_ref().map(|c| c.player.hp).unwrap_or(80);
        let max_hp = self.combat.as_ref().map(|c| c.player.max_hp).unwrap_or(80);
        self.card_advice = sim_pick_score(
            &offered,
            &deck,
            hp,
            max_hp,
            &self.act_filter,
            &self.encounters,
            &self.monsters,
            rng,
        );
        self.selected_row = 0;
        self.mode = AppMode::CardPick;
    }
}

/// Return 3 random cards from the current character's pool as a simulated reward.
fn sample_card_offer(color: &str, rng: &mut impl rand::Rng) -> Vec<Card> {
    use rand::seq::SliceRandom;
    let mut pool = catalog::cards_for_character(color);
    pool.shuffle(rng);
    pool.truncate(3);
    pool
}

/// Derive the character color string (used by the card catalog) from a PlayerClass.
fn char_color(class: &PlayerClass) -> &'static str {
    match class {
        PlayerClass::Ironclad    => "ironclad",
        PlayerClass::Silent      => "silent",
        PlayerClass::Regent      => "regent",
        PlayerClass::Necrobinder => "necrobinder",
        PlayerClass::Defect      => "defect",
        PlayerClass::Watcher     => "colorless",
    }
}

/// Derive PlayerClass from the color field in the API character data.
fn class_from_color(color: &str) -> PlayerClass {
    match color {
        "red"    => PlayerClass::Ironclad,
        "green"  => PlayerClass::Silent,
        "orange" => PlayerClass::Regent,
        "purple" => PlayerClass::Necrobinder,
        "blue"   => PlayerClass::Defect,
        _        => PlayerClass::Ironclad,
    }
}

/// Sort characters into the canonical unlock order:
/// Ironclad → Silent → Regent → Necrobinder → Defect.
fn sort_characters(mut chars: Vec<SpireApiCharacter>) -> Vec<SpireApiCharacter> {
    let order = ["red", "green", "orange", "purple", "blue"];
    chars.sort_by_key(|c| {
        let color = c.color.as_deref().unwrap_or("");
        order.iter().position(|&o| o == color).unwrap_or(99)
    });
    chars
}

pub fn run_app() -> anyhow::Result<()> {
    use crossterm::{
        execute,
        terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
    };
    use ratatui::{backend::CrosstermBackend, Terminal};
    use std::io;

    let monsters = crate::data::api::load_monsters();
    let encounters = crate::data::api::load_encounters();
    let events = crate::data::api::load_events();
    let characters = crate::data::api::load_characters();

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;

    let backend = CrosstermBackend::new(io::stdout());
    let mut terminal = Terminal::new(backend)?;

    let mut app = App::new(characters, encounters, monsters, events);
    let mut rng = StdRng::from_entropy();
    let events = spawn_event_loop(200);

    let result = run_loop(&mut terminal, &mut app, &mut rng, events);

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    result
}

fn run_loop(
    terminal: &mut ratatui::Terminal<ratatui::backend::CrosstermBackend<std::io::Stdout>>,
    app: &mut App,
    rng: &mut impl rand::Rng,
    events: std::sync::mpsc::Receiver<AppEvent>,
) -> anyhow::Result<()> {
    loop {
        terminal.draw(|frame| ui::render(frame, app))?;

        match events.recv() {
            Ok(event) => {
                if !app.handle_event(event, rng) {
                    break;
                }
                if app.mode == AppMode::Exiting {
                    break;
                }
            }
            Err(_) => break,
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyEventKind, KeyEventState, KeyModifiers};

    fn make_key(code: KeyCode) -> AppEvent {
        AppEvent::Key(KeyEvent {
            code,
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        })
    }

    fn seeded_rng() -> StdRng {
        StdRng::seed_from_u64(42)
    }

    fn empty_app() -> App {
        App::new(vec![], vec![], vec![], vec![])
    }

    fn app_with_combat() -> App {
        let mut app = empty_app();
        app.load_combat(default_combat_state());
        app
    }

    #[test]
    fn new_app_starts_in_encounter_pick() {
        let app = empty_app();
        assert_eq!(app.mode, AppMode::EncounterPick);
    }

    #[test]
    fn quit_key_returns_false() {
        let mut app = empty_app();
        let mut rng = seeded_rng();
        let result = app.handle_event(AppEvent::Quit, &mut rng);
        assert!(!result);
    }

    #[test]
    fn enter_with_no_encounters_falls_back_to_default() {
        let mut app = empty_app();
        let mut rng = seeded_rng();
        app.handle_event(make_key(KeyCode::Enter), &mut rng);
        assert_eq!(app.mode, AppMode::CombatAdvice);
        assert!(app.combat.is_some());
    }

    #[test]
    fn act_filter_keys_change_filter() {
        let mut app = empty_app();
        let mut rng = seeded_rng();
        app.handle_event(make_key(KeyCode::Char('u')), &mut rng);
        assert_eq!(app.act_filter, "underdocks");
        app.handle_event(make_key(KeyCode::Char('h')), &mut rng);
        assert_eq!(app.act_filter, "hive");
        app.handle_event(make_key(KeyCode::Char('g')), &mut rng);
        assert_eq!(app.act_filter, "glory");
        app.handle_event(make_key(KeyCode::Char('b')), &mut rng);
        assert_eq!(app.act_filter, "boss");
        app.handle_event(make_key(KeyCode::Char('a')), &mut rng);
        assert_eq!(app.act_filter, "all");
        app.handle_event(make_key(KeyCode::Char('o')), &mut rng);
        assert_eq!(app.act_filter, "overgrowth");
    }

    #[test]
    fn encounter_selected_loads_combat() {
        use crate::data::api::{ApiEncounterMonster, SpireApiEncounter};
        let enc = SpireApiEncounter {
            id: "test_enc".into(),
            name: "Test Fight".into(),
            room_type: Some("Monster".into()),
            is_weak: Some(false),
            act: Some("Act 1 - Overgrowth".into()),
            tags: vec![],
            monsters: vec![ApiEncounterMonster {
                id: "cultist".into(),
                name: "Cultist".into(),
            }],
            loss_text: None,
        };
        let mut app = App::new(vec![], vec![enc], vec![], vec![]);
        let mut rng = seeded_rng();
        // Default filter is "overgrowth" → should match
        assert_eq!(app.filtered_indices.len(), 1);
        app.handle_event(make_key(KeyCode::Enter), &mut rng);
        assert_eq!(app.mode, AppMode::CombatAdvice);
        assert!(app.combat.is_some());
        assert_eq!(app.combat.as_ref().unwrap().enemies.len(), 1);
    }

    #[test]
    fn load_combat_transitions_to_combat_advice() {
        let mut app = empty_app();
        app.load_combat(default_combat_state());
        assert_eq!(app.mode, AppMode::CombatAdvice);
    }

    #[test]
    fn simulate_produces_advice() {
        let mut app = app_with_combat();
        let mut rng = seeded_rng();
        app.run_simulation(&mut rng);
        assert!(app.play_advice.is_some());
    }

    #[test]
    fn edit_key_goes_to_manual_input() {
        let mut app = app_with_combat();
        let mut rng = seeded_rng();
        app.handle_event(make_key(KeyCode::Char('e')), &mut rng);
        assert_eq!(app.mode, AppMode::ManualInput);
    }

    #[test]
    fn n_key_returns_to_encounter_pick() {
        let mut app = app_with_combat();
        let mut rng = seeded_rng();
        app.handle_event(make_key(KeyCode::Char('n')), &mut rng);
        assert_eq!(app.mode, AppMode::EncounterPick);
    }

    #[test]
    fn j_k_navigation_in_encounter_list() {
        use crate::data::api::{ApiEncounterMonster, SpireApiEncounter};
        let make_enc = |id: &str| SpireApiEncounter {
            id: id.into(),
            name: id.into(),
            room_type: Some("Monster".into()),
            is_weak: None,
            act: Some("Act 1 - Overgrowth".into()),
            tags: vec![],
            monsters: vec![],
            loss_text: None,
        };
        let mut app = App::new(vec![], vec![make_enc("a"), make_enc("b"), make_enc("c")], vec![], vec![]);
        let mut rng = seeded_rng();
        assert_eq!(app.selected_row, 0);
        app.handle_event(make_key(KeyCode::Char('j')), &mut rng);
        assert_eq!(app.selected_row, 1);
        app.handle_event(make_key(KeyCode::Char('k')), &mut rng);
        assert_eq!(app.selected_row, 0);
    }

    #[test]
    fn tick_is_noop() {
        let mut app = empty_app();
        let mut rng = seeded_rng();
        let mode_before = app.mode.clone();
        app.handle_event(AppEvent::Tick, &mut rng);
        assert_eq!(app.mode, mode_before);
    }
}
