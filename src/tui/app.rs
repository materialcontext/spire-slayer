use crossterm::event::{KeyCode, KeyEvent};
use rand::SeedableRng;
use rand::rngs::StdRng;

use crate::data::api::{SpireApiEncounter, SpireApiMonster};
use crate::domain::card::Card;
use crate::domain::combat::CombatState;
use crate::domain::encounter::{encounter_to_combat, encounters_for_act};
use crate::domain::run::RunState;
use crate::input::event::{spawn_event_loop, AppEvent};
use crate::input::manual::{default_combat_state, ManualInputState};
use crate::metrics::card_pick::{sim_pick_score, CardAdvice};
use crate::sim::mcts::{best_play_sequence, PlayAdvice};
use crate::sim::playout::playout_n;
use crate::sim::policy::GreedyDamagePolicy;
use crate::tui::ui;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppMode {
    EncounterPick,
    CombatAdvice,
    CardPick,
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
    pub input: Option<ManualInputState>,
    pub status_message: String,
    pub selected_row: usize,
    // Encounter picker state
    pub encounters: Vec<SpireApiEncounter>,
    pub monsters: Vec<SpireApiMonster>,
    pub act_filter: String,
    pub filtered_indices: Vec<usize>,
}

impl App {
    pub fn new(encounters: Vec<SpireApiEncounter>, monsters: Vec<SpireApiMonster>) -> Self {
        let mut app = Self {
            mode: AppMode::EncounterPick,
            combat: None,
            run: None,
            play_advice: None,
            card_advice: Vec::new(),
            input: None,
            status_message: String::new(),
            selected_row: 0,
            encounters,
            monsters,
            act_filter: "1".to_string(),
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
            AppMode::EncounterPick => self.handle_key_encounter(key),
            AppMode::ManualInput => self.handle_key_input(key),
            AppMode::CombatAdvice => self.handle_key_combat(key, rng),
            AppMode::CardPick => self.handle_key_pick(key),
            AppMode::Simulating | AppMode::Exiting => {}
        }
    }

    fn handle_key_encounter(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('q') => {
                self.mode = AppMode::Exiting;
            }
            KeyCode::Char('1') => {
                self.act_filter = "1".to_string();
                self.refresh_filter();
            }
            KeyCode::Char('2') => {
                self.act_filter = "2".to_string();
                self.refresh_filter();
            }
            KeyCode::Char('3') => {
                self.act_filter = "3".to_string();
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
            KeyCode::Char('p') => {
                // Simulate a card reward pick with 3 sample cards
                let offered = sample_card_offer(rng);
                self.load_pick(offered, rng);
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
        let act = self.run.as_ref().map(|r| r.act).unwrap_or(1);
        self.card_advice = sim_pick_score(
            &offered,
            &deck,
            hp,
            max_hp,
            act,
            &self.encounters,
            &self.monsters,
            rng,
        );
        self.selected_row = 0;
        self.mode = AppMode::CardPick;
    }
}

/// Return 3 random Ironclad cards as a simulated post-combat reward.
fn sample_card_offer(rng: &mut impl rand::Rng) -> Vec<Card> {
    use rand::seq::SliceRandom;
    let mut pool = crate::domain::catalog::ironclad::all_cards();
    pool.shuffle(rng);
    pool.truncate(3);
    pool
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

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;

    let backend = CrosstermBackend::new(io::stdout());
    let mut terminal = Terminal::new(backend)?;

    let mut app = App::new(encounters, monsters);
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
        App::new(vec![], vec![])
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
        app.handle_event(make_key(KeyCode::Char('2')), &mut rng);
        assert_eq!(app.act_filter, "2");
        app.handle_event(make_key(KeyCode::Char('b')), &mut rng);
        assert_eq!(app.act_filter, "boss");
        app.handle_event(make_key(KeyCode::Char('a')), &mut rng);
        assert_eq!(app.act_filter, "all");
    }

    #[test]
    fn encounter_selected_loads_combat() {
        use crate::data::api::{ApiEncounterMonster, SpireApiEncounter};
        let enc = SpireApiEncounter {
            id: "test_enc".into(),
            name: "Test Fight".into(),
            room_type: Some("normal".into()),
            is_weak: Some(false),
            act: Some("1".into()),
            tags: vec![],
            monsters: vec![ApiEncounterMonster {
                id: "cultist".into(),
                name: "Cultist".into(),
            }],
            loss_text: None,
        };
        let mut app = App::new(vec![enc], vec![]);
        let mut rng = seeded_rng();
        // Act filter is "1", encounter is act 1 → should be in list
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
            room_type: None,
            is_weak: None,
            act: Some("1".into()),
            tags: vec![],
            monsters: vec![],
            loss_text: None,
        };
        let mut app = App::new(vec![make_enc("a"), make_enc("b"), make_enc("c")], vec![]);
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
