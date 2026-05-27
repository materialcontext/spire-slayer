use crossterm::event::{KeyCode, KeyEvent};
use rand::SeedableRng;
use rand::rngs::StdRng;

use crate::domain::card::Card;
use crate::domain::combat::CombatState;
use crate::domain::run::RunState;
use crate::input::event::{spawn_event_loop, AppEvent};
use crate::input::manual::{default_combat_state, ManualInputState};
use crate::metrics::card_pick::{pick_score, CardAdvice};
use crate::sim::mcts::{best_play_sequence, PlayAdvice};
use crate::tui::ui;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppMode {
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
}

impl App {
    pub fn new() -> Self {
        let base = default_combat_state();
        let input = ManualInputState::new(base);
        Self {
            mode: AppMode::ManualInput,
            combat: None,
            run: None,
            play_advice: None,
            card_advice: Vec::new(),
            input: Some(input),
            status_message: String::new(),
            selected_row: 0,
        }
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
            AppMode::ManualInput => self.handle_key_input(key),
            AppMode::CombatAdvice => self.handle_key_combat(key, rng),
            AppMode::CardPick => self.handle_key_pick(key),
            AppMode::Simulating | AppMode::Exiting => {}
        }
    }

    fn handle_key_input(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('q') => {
                self.mode = AppMode::Exiting;
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
        let advice = best_play_sequence(combat, 500, rng);
        self.status_message = format!("{} simulations run", advice.simulation_count);
        self.play_advice = Some(advice);
        self.mode = AppMode::CombatAdvice;
    }

    pub fn load_combat(&mut self, state: CombatState) {
        self.play_advice = None;
        self.selected_row = 0;
        self.combat = Some(state);
        self.mode = AppMode::CombatAdvice;
        self.status_message = "State loaded. Press [s] to simulate.".to_string();
    }

    pub fn load_pick(&mut self, offered: Vec<Card>) {
        let _deck = self.run.as_ref().map(|r| r.deck.as_slice()).unwrap_or(&[]);
        let act = self.run.as_ref().map(|r| r.act).unwrap_or(1);
        self.card_advice = pick_score(&offered, &crate::domain::run::RunState {
            class: self.run.as_ref().map(|r| r.class.clone())
                .unwrap_or(crate::domain::run::PlayerClass::Ironclad),
            floor: self.run.as_ref().map(|r| r.floor).unwrap_or(0),
            act,
            hp: self.combat.as_ref().map(|c| c.player.hp).unwrap_or(80),
            max_hp: self.combat.as_ref().map(|c| c.player.max_hp).unwrap_or(80),
            gold: self.run.as_ref().map(|r| r.gold).unwrap_or(0),
            deck: offered.clone(),
            relics: vec![],
            potions: vec![],
        });
        self.selected_row = 0;
        self.mode = AppMode::CardPick;
    }
}

pub fn run_app() -> anyhow::Result<()> {
    use crossterm::{
        execute,
        terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
    };
    use ratatui::{backend::CrosstermBackend, Terminal};
    use std::io;

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;

    let backend = CrosstermBackend::new(io::stdout());
    let mut terminal = Terminal::new(backend)?;

    let mut app = App::new();
    let mut rng = StdRng::seed_from_u64(42);
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

    #[test]
    fn new_app_starts_in_manual_input() {
        let app = App::new();
        assert_eq!(app.mode, AppMode::ManualInput);
        assert!(app.input.is_some());
    }

    #[test]
    fn quit_key_returns_false() {
        let mut app = App::new();
        let mut rng = seeded_rng();
        let result = app.handle_event(AppEvent::Quit, &mut rng);
        assert!(!result);
    }

    #[test]
    fn load_combat_transitions_to_combat_advice() {
        let mut app = App::new();
        let state = default_combat_state();
        app.load_combat(state);
        assert_eq!(app.mode, AppMode::CombatAdvice);
        assert!(app.combat.is_some());
    }

    #[test]
    fn simulate_produces_advice() {
        let mut app = App::new();
        let state = default_combat_state();
        app.load_combat(state);
        let mut rng = seeded_rng();
        app.run_simulation(&mut rng);
        assert!(app.play_advice.is_some());
    }

    #[test]
    fn edit_key_returns_to_manual_input() {
        let mut app = App::new();
        let state = default_combat_state();
        app.load_combat(state);
        let mut rng = seeded_rng();
        app.handle_event(make_key(KeyCode::Char('e')), &mut rng);
        assert_eq!(app.mode, AppMode::ManualInput);
    }

    #[test]
    fn j_k_navigation_in_combat() {
        let mut app = App::new();
        let state = default_combat_state();
        app.load_combat(state);
        let mut rng = seeded_rng();

        app.handle_event(make_key(KeyCode::Char('j')), &mut rng);
        assert_eq!(app.selected_row, 1);
        app.handle_event(make_key(KeyCode::Char('k')), &mut rng);
        assert_eq!(app.selected_row, 0);
    }

    #[test]
    fn tick_event_is_no_op() {
        let mut app = App::new();
        let mut rng = seeded_rng();
        let mode_before = app.mode.clone();
        app.handle_event(AppEvent::Tick, &mut rng);
        assert_eq!(app.mode, mode_before);
    }

    #[test]
    fn state_updated_event_loads_combat() {
        let mut app = App::new();
        let mut rng = seeded_rng();
        let state = default_combat_state();
        app.handle_event(AppEvent::StateUpdated(Box::new(state)), &mut rng);
        assert_eq!(app.mode, AppMode::CombatAdvice);
    }
}
