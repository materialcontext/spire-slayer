use crossterm::event::{KeyCode, KeyEvent};
use rand::SeedableRng;
use rand::rngs::StdRng;

use crate::data::api::{SpireApiCharacter, SpireApiEncounter, SpireApiEvent, SpireApiMonster, SpireApiRelic};
use crate::domain::card::Card;
use crate::domain::catalog;
use crate::domain::combat::CombatState;
use crate::domain::reward::{sample_offer, RewardKind};
use crate::domain::encounter::{encounter_to_combat, encounters_for_act};
use crate::domain::run::{PlayerClass, Relic, RunState, starting_relics};
use crate::input::event::{spawn_event_loop, AppEvent};
use crate::input::manual::{default_combat_state, ManualInputState};
use crate::metrics::card_pick::{sim_pick_score, CardAdvice};
use crate::metrics::deck_dash::{compute_deck_stats, DeckStats};
use crate::metrics::event::{advise_event, EventOptionAdvice};
use crate::metrics::map_ev::{compute_map_ev, events_for_sub_act, MapEvData};
use crate::metrics::rest::{advise_rest, RestAction, RestAdvice};
use crate::sim::mcts::{best_play_sequence, PlayAdvice};
use crate::sim::playout::playout_n;
use crate::sim::policy::GreedyDamagePolicy;
use crate::tui::ui;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppMode {
    CharacterPick,
    EncounterPick,
    MapView,
    RestSite,
    EventRoom,
    TreasureRoom,
    Shop,
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
    /// Cursor index into the available map choices at the current position.
    pub map_cursor: usize,
    /// Rest site recommendation and current selection (0=Heal, 1=Smith).
    pub rest_advice: Option<RestAdvice>,
    pub rest_cursor: usize,
    /// Active event and scored options for EventRoom mode.
    pub active_event: Option<SpireApiEvent>,
    pub event_advice: Vec<EventOptionAdvice>,
    pub event_cursor: usize,
    /// Cards in the current card-pick offer; indices match card_advice[*].card_index.
    pub offered_cards: Vec<Card>,
    /// Where to return when CardPick is dismissed, and whether to commit the pick to the run.
    pub card_pick_return: AppMode,
    /// All relic data loaded at startup.
    pub relics: Vec<SpireApiRelic>,
    /// The single relic on offer in treasure room.
    pub offered_relic: Option<SpireApiRelic>,
    /// Cursor in treasure room: 0 = Take, 1 = Skip.
    pub relic_cursor: usize,
    /// Shop inventory.
    pub shop_cards: Vec<Card>,
    pub shop_card_advice: Vec<CardAdvice>,
    pub shop_relics: Vec<SpireApiRelic>,
    pub shop_cursor: usize,
}

impl App {
    pub fn new(
        characters: Vec<SpireApiCharacter>,
        encounters: Vec<SpireApiEncounter>,
        monsters: Vec<SpireApiMonster>,
        events: Vec<SpireApiEvent>,
        relics: Vec<SpireApiRelic>,
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
            map_cursor: 0,
            rest_advice: None,
            rest_cursor: 0,
            active_event: None,
            event_advice: Vec::new(),
            event_cursor: 0,
            offered_cards: Vec::new(),
            card_pick_return: AppMode::EncounterPick,
            relics,
            offered_relic: None,
            relic_cursor: 0,
            shop_cards: Vec::new(),
            shop_card_advice: Vec::new(),
            shop_relics: Vec::new(),
            shop_cursor: 0,
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
            AppMode::CharacterPick  => self.handle_key_character(key, rng),
            AppMode::EncounterPick  => self.handle_key_encounter(key, rng),
            AppMode::MapView        => self.handle_key_map_view(key, rng),
            AppMode::RestSite       => self.handle_key_rest_site(key),
            AppMode::EventRoom      => self.handle_key_event_room(key),
            AppMode::TreasureRoom   => self.handle_key_treasure_room(key),
            AppMode::Shop           => self.handle_key_shop(key),
            AppMode::ManualInput    => self.handle_key_input(key),
            AppMode::CombatAdvice   => self.handle_key_combat(key, rng),
            AppMode::CardPick       => self.handle_key_pick(key),
            AppMode::DeckDash       => self.handle_key_deck_dash(key),
            AppMode::MapEv          => self.handle_key_map_ev(key),
            AppMode::Simulating | AppMode::Exiting => {}
        }
    }

    fn handle_key_character(&mut self, key: KeyEvent, rng: &mut impl rand::Rng) {
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
                self.select_character(rng);
            }
            _ => {}
        }
    }

    fn handle_key_map_view(&mut self, key: KeyEvent, rng: &mut impl rand::Rng) {
        match key.code {
            KeyCode::Char('q') => {
                self.mode = AppMode::Exiting;
            }
            KeyCode::Esc | KeyCode::Char('t') => {
                self.mode = AppMode::EncounterPick;
            }
            KeyCode::Char('j') | KeyCode::Right => {
                let n = self.map_choices().len();
                if n > 0 { self.map_cursor = (self.map_cursor + 1).min(n - 1); }
            }
            KeyCode::Char('k') | KeyCode::Left => {
                if self.map_cursor > 0 { self.map_cursor -= 1; }
            }
            KeyCode::Enter => {
                self.select_map_node(rng);
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
            KeyCode::Char('t') => {
                if self.run.is_some() { self.open_map_view(rng); }
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
                let pool = card_pool_for_run(self);
                // Preview: use a throwaway offset so the real run's offset is unchanged.
                let mut preview_offset = self.run.as_ref().map(|r| r.rare_offset).unwrap_or(-5);
                let offered = sample_offer(&pool, RewardKind::Monster, &mut preview_offset, rng);
                self.load_pick(offered, rng, AppMode::CombatAdvice);
            }
            KeyCode::Char('v') => {
                self.open_map_ev(rng);
            }
            KeyCode::Char('t') => {
                self.open_map_view(rng);
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
            KeyCode::Char('q') | KeyCode::Esc => {
                let dest = self.card_pick_return.clone();
                self.mode = dest;
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
                // Only commit the pick to the run when triggered from the map.
                if self.card_pick_return == AppMode::MapView {
                    self.apply_card_pick();
                }
                let dest = self.card_pick_return.clone();
                self.mode = dest;
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

    pub fn select_character(&mut self, rng: &mut impl rand::Rng) {
        let Some(char_data) = self.characters.get(self.selected_row) else { return; };
        let hp       = char_data.starting_hp.unwrap_or(75).max(1) as u32;
        let gold     = char_data.starting_gold.unwrap_or(99) as u32;
        let deck_ids = char_data.starting_deck.clone();
        let color    = char_data.color.clone().unwrap_or_default();
        let char_name = char_data.name.clone();

        let deck  = catalog::deck_from_ids(&deck_ids);
        let class = class_from_color(&color);
        let relic = starting_relics::for_class(&class);

        let mut run  = RunState::new(class, hp, hp, deck, relic);
        run.gold     = gold;
        run.sub_act  = "overgrowth".to_string();

        self.status_message = format!(
            "Playing as {} — HP {}, {} starting cards — choose your path",
            char_name, hp, run.deck.len(),
        );
        self.run = Some(run);
        self.act_filter = "overgrowth".to_string();
        self.refresh_filter();
        self.selected_row = 0;
        self.open_map_view(rng);
    }

    /// Generate the map (if not already present) and switch to MapView.
    pub fn open_map_view(&mut self, rng: &mut impl rand::Rng) {
        if let Some(ref mut run) = self.run {
            if run.map.is_none() {
                let seed: u64 = rng.r#gen();
                run.generate_map(seed, 0);
            }
        }
        self.map_cursor = 0;
        self.mode = AppMode::MapView;
    }

    /// Columns available to move to from the current map position.
    pub fn map_choices(&self) -> Vec<u8> {
        let Some(ref run) = self.run else { return vec![]; };
        let Some(ref map) = run.map else { return vec![]; };
        match run.map_pos {
            None      => map.entry_nodes(),
            Some(pos) => map.choices_from(pos).to_vec(),
        }
    }

    /// Move to the currently cursor-selected map node and resolve the room.
    fn select_map_node(&mut self, rng: &mut impl rand::Rng) {
        use crate::domain::map::RoomType;
        let choices = self.map_choices();
        let Some(&col) = choices.get(self.map_cursor) else { return; };
        let Some(ref mut run) = self.run else { return; };
        run.move_to(col as usize);
        self.map_cursor = 0;

        let room = run.map_pos.and_then(|pos| {
            run.map.as_ref().and_then(|m| m.room_type(pos.floor, pos.col))
        });

        match room {
            Some(rt @ RoomType::Monster) | Some(rt @ RoomType::Elite) => {
                let kind = if rt == RoomType::Elite { RewardKind::Elite } else { RewardKind::Monster };
                let pool = card_pool_for_run(self);
                let offset = self.run.as_mut().map(|r| &mut r.rare_offset);
                let offered = if let Some(off) = offset {
                    sample_offer(&pool, kind, off, rng)
                } else {
                    pool.into_iter().take(3).collect()
                };
                self.load_pick(offered, rng, AppMode::MapView);
            }
            Some(RoomType::Boss) => {
                let pool = card_pool_for_run(self);
                let offset = self.run.as_mut().map(|r| &mut r.rare_offset);
                let offered = if let Some(off) = offset {
                    sample_offer(&pool, RewardKind::Boss, off, rng)
                } else {
                    pool.into_iter().take(3).collect()
                };
                self.load_pick(offered, rng, AppMode::MapView);
            }
            Some(RoomType::Rest) => self.open_rest_site(),
            Some(RoomType::Event) => self.open_event_room(rng),
            Some(RoomType::Treasure) => self.open_treasure_room(rng),
            Some(RoomType::Shop) => self.open_shop(rng),
            Some(rt) => {
                self.status_message = format!("Floor {} — {}", self.run.as_ref().map(|r| r.floor).unwrap_or(0), rt.label());
            }
            None => {}
        }
    }

    fn open_rest_site(&mut self) {
        if let Some(ref run) = self.run {
            let advice = advise_rest(run);
            self.rest_cursor = match advice.action {
                RestAction::Heal    => 0,
                RestAction::Smith(_) => 1,
            };
            self.rest_advice = Some(advice);
        }
        self.mode = AppMode::RestSite;
    }

    fn handle_key_rest_site(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => {
                self.rest_cursor = (self.rest_cursor + 1) % 2;
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.rest_cursor = self.rest_cursor.saturating_sub(1).max(0);
                if self.rest_cursor == 0 { self.rest_cursor = 0; }
            }
            KeyCode::Enter => self.confirm_rest(),
            KeyCode::Esc => self.mode = AppMode::MapView,
            KeyCode::Char('q') => self.mode = AppMode::Exiting,
            _ => {}
        }
    }

    fn confirm_rest(&mut self) {
        let Some(ref run) = self.run else { return; };
        let smith_idx = match self.rest_advice.as_ref().and_then(|a| {
            if let RestAction::Smith(i) = a.action { Some(i) } else { None }
        }) {
            Some(i) => i,
            None => {
                // No smith candidate — treat as heal regardless of cursor.
                let run = self.run.as_mut().unwrap();
                run.heal();
                self.status_message = format!("Healed to {} HP", run.hp);
                self.mode = AppMode::MapView;
                return;
            }
        };
        let _ = run; // drop shared borrow before mut
        let run = self.run.as_mut().unwrap();
        if self.rest_cursor == 0 {
            run.heal();
            self.status_message = format!("Healed to {} HP", run.hp);
        } else {
            if run.smith(smith_idx) {
                let name = run.deck[smith_idx].name.clone();
                self.status_message = format!("Upgraded {}", name);
            }
        }
        self.rest_advice = None;
        self.mode = AppMode::MapView;
    }

    fn open_event_room(&mut self, rng: &mut impl rand::Rng) {
        use rand::seq::SliceRandom;
        let sub_act = self.run.as_ref().map(|r| r.sub_act.clone()).unwrap_or_else(|| "overgrowth".to_string());
        let hp_ratio = self.run.as_ref().map(|r| r.hp as f32 / r.max_hp as f32).unwrap_or(1.0);

        let pool = events_for_sub_act(&sub_act, &self.events);
        let event = pool.choose(rng).map(|e| (*e).clone());

        self.event_advice = event.as_ref()
            .map(|e| advise_event(&e.options, hp_ratio))
            .unwrap_or_default();

        // Set cursor to recommended option (first in sorted advice).
        self.event_cursor = self.event_advice.first().map(|a| a.option_idx).unwrap_or(0);
        self.active_event = event;
        self.mode = AppMode::EventRoom;
    }

    fn handle_key_event_room(&mut self, key: KeyEvent) {
        let n = self.active_event.as_ref().map(|e| e.options.len()).unwrap_or(0);
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => {
                if n > 0 { self.event_cursor = (self.event_cursor + 1).min(n - 1); }
            }
            KeyCode::Char('k') | KeyCode::Up => {
                if self.event_cursor > 0 { self.event_cursor -= 1; }
            }
            KeyCode::Enter => {
                let title = self.active_event.as_ref()
                    .and_then(|e| e.options.get(self.event_cursor))
                    .and_then(|o| o.title.as_deref())
                    .unwrap_or("?")
                    .to_string();
                self.status_message = format!("Chose: {title}");
                self.active_event = None;
                self.event_advice.clear();
                self.mode = AppMode::MapView;
            }
            KeyCode::Esc => {
                self.mode = AppMode::MapView;
            }
            KeyCode::Char('q') => {
                self.mode = AppMode::Exiting;
            }
            _ => {}
        }
    }

    fn open_treasure_room(&mut self, rng: &mut impl rand::Rng) {
        use rand::seq::SliceRandom;
        let class_pool = self.run.as_ref()
            .map(|r| char_color(&r.class))
            .unwrap_or("ironclad");
        let pool: Vec<&SpireApiRelic> = self.relics.iter().filter(|r| {
            let rarity = r.rarity.as_deref().unwrap_or("");
            let p = r.pool.as_deref().unwrap_or("shared");
            matches!(rarity, "Common Relic" | "Uncommon Relic" | "Rare Relic")
                && (p == "shared" || p == class_pool)
        }).collect();
        self.offered_relic = pool.choose(rng).map(|r| (*r).clone());
        self.relic_cursor = 0;
        self.mode = AppMode::TreasureRoom;
    }

    fn handle_key_treasure_room(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => { self.relic_cursor = 1; }
            KeyCode::Char('k') | KeyCode::Up   => { self.relic_cursor = 0; }
            KeyCode::Enter => {
                if self.relic_cursor == 0 {
                    if let (Some(relic), Some(run)) = (self.offered_relic.take(), self.run.as_mut()) {
                        let desc = relic.description.clone().unwrap_or_default();
                        run.relics.push(crate::domain::run::Relic::new(&relic.name, desc));
                        self.status_message = format!("Took {}", relic.name);
                    }
                } else {
                    self.status_message = "Skipped relic".to_string();
                    self.offered_relic = None;
                }
                self.mode = AppMode::MapView;
            }
            KeyCode::Esc => {
                self.offered_relic = None;
                self.mode = AppMode::MapView;
            }
            KeyCode::Char('q') => { self.mode = AppMode::Exiting; }
            _ => {}
        }
    }

    fn open_shop(&mut self, rng: &mut impl rand::Rng) {
        use rand::seq::SliceRandom;
        use crate::metrics::card_pick::pick_score;

        // 5 cards at shop rarity weights
        let pool = card_pool_for_run(self);
        let mut offset_copy = self.run.as_ref().map(|r| r.rare_offset).unwrap_or(-5);
        let mut cards = sample_offer(&pool, RewardKind::Shop, &mut offset_copy, rng);
        let extra = sample_offer(&pool, RewardKind::Shop, &mut offset_copy, rng);
        cards.extend(extra);
        cards.truncate(5);

        // 2 shop relics
        let class_pool = self.run.as_ref().map(|r| char_color(&r.class)).unwrap_or("ironclad");
        let relic_pool: Vec<&SpireApiRelic> = self.relics.iter().filter(|r| {
            let rarity = r.rarity.as_deref().unwrap_or("");
            let p = r.pool.as_deref().unwrap_or("shared");
            rarity == "Shop Relic" && (p == "shared" || p == class_pool)
        }).collect();
        let shop_relics: Vec<SpireApiRelic> = relic_pool
            .choose_multiple(rng, 2)
            .map(|r| (*r).clone())
            .collect();

        // Score the cards
        let advice = if let Some(run) = &self.run {
            pick_score(&cards, run)
        } else {
            cards.iter().enumerate().map(|(i, _)| crate::metrics::card_pick::CardAdvice {
                card_index: i, score: 0.0, reason: String::new(),
                win_rate: 0.0, mean_hp_delta: 0.0, hp_loss_p50: 0.0,
                delta_win_rate: 0.0, delta_hp: 0.0,
            }).collect()
        };

        self.shop_cards = cards;
        self.shop_card_advice = advice;
        self.shop_relics = shop_relics;
        self.shop_cursor = 0;
        self.mode = AppMode::Shop;
    }

    fn handle_key_shop(&mut self, key: KeyEvent) {
        let n_cards = self.shop_cards.len();
        let n_relics = self.shop_relics.len();
        let total = n_cards + n_relics;
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => {
                if total > 0 { self.shop_cursor = (self.shop_cursor + 1).min(total - 1); }
            }
            KeyCode::Char('k') | KeyCode::Up => {
                if self.shop_cursor > 0 { self.shop_cursor -= 1; }
            }
            KeyCode::Enter => {
                if self.shop_cursor < n_cards {
                    let card = self.shop_cards.get(self.shop_cursor).cloned();
                    if let (Some(card), Some(run)) = (card, self.run.as_mut()) {
                        let name = card.name.clone();
                        run.deck.push(card);
                        self.status_message = format!("Bought {} ({} cards)", name, run.deck.len());
                        self.shop_cards.remove(self.shop_cursor);
                        self.shop_card_advice.retain(|a| a.card_index != self.shop_cursor);
                        if self.shop_cursor >= self.shop_cards.len() {
                            self.shop_cursor = self.shop_cursor.saturating_sub(1);
                        }
                    }
                } else {
                    let relic_idx = self.shop_cursor - n_cards;
                    let relic = self.shop_relics.get(relic_idx).cloned();
                    if let (Some(relic), Some(run)) = (relic, self.run.as_mut()) {
                        let desc = relic.description.clone().unwrap_or_default();
                        run.relics.push(crate::domain::run::Relic::new(&relic.name, desc));
                        self.status_message = format!("Bought {}", relic.name);
                        self.shop_relics.remove(relic_idx);
                        if self.shop_cursor >= self.shop_cards.len() + self.shop_relics.len() {
                            self.shop_cursor = self.shop_cursor.saturating_sub(1);
                        }
                    }
                }
            }
            KeyCode::Esc => {
                self.shop_cards.clear();
                self.shop_card_advice.clear();
                self.shop_relics.clear();
                self.mode = AppMode::MapView;
            }
            KeyCode::Char('q') => { self.mode = AppMode::Exiting; }
            _ => {}
        }
    }

    pub fn load_combat(&mut self, state: CombatState) {
        self.play_advice = None;
        self.selected_row = 0;
        self.combat = Some(state);
        self.mode = AppMode::CombatAdvice;
    }

    pub fn load_pick(&mut self, offered: Vec<Card>, rng: &mut impl rand::Rng, return_to: AppMode) {
        let deck = self.run.as_ref().map(|r| r.deck.clone()).unwrap_or_else(|| {
            crate::domain::catalog::ironclad::starter_deck()
        });
        let hp = self.run.as_ref().map(|r| r.hp)
            .or_else(|| self.combat.as_ref().map(|c| c.player.hp))
            .unwrap_or(80);
        let max_hp = self.run.as_ref().map(|r| r.max_hp)
            .or_else(|| self.combat.as_ref().map(|c| c.player.max_hp))
            .unwrap_or(80);
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
        self.offered_cards = offered;
        self.card_pick_return = return_to;
        self.selected_row = 0;
        self.mode = AppMode::CardPick;
    }

    fn apply_card_pick(&mut self) {
        let Some(advice) = self.card_advice.get(self.selected_row) else { return; };
        if advice.card_index == usize::MAX {
            self.status_message = "Skipped card reward".to_string();
            return;
        }
        let card = self.offered_cards.get(advice.card_index).cloned();
        if let Some(card) = card {
            let name = card.name.clone();
            if let Some(ref mut run) = self.run {
                run.deck.push(card);
                self.status_message = format!("Added {} to deck ({} cards)", name, run.deck.len());
            }
        }
    }
}

/// Return 3 random cards from the current character's pool as a simulated reward.
fn card_pool_for_run(app: &App) -> Vec<Card> {
    let color = app.run.as_ref().map(|r| char_color(&r.class)).unwrap_or("ironclad");
    catalog::cards_for_character(color)
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
    let relics = crate::data::api::load_relics();

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;

    let backend = CrosstermBackend::new(io::stdout());
    let mut terminal = Terminal::new(backend)?;

    let mut app = App::new(characters, encounters, monsters, events, relics);
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
        App::new(vec![], vec![], vec![], vec![], vec![])
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
        let mut app = App::new(vec![], vec![enc], vec![], vec![], vec![]);
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
        let mut app = App::new(vec![], vec![make_enc("a"), make_enc("b"), make_enc("c")], vec![], vec![], vec![]);
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

    fn app_with_run() -> App {
        use crate::domain::catalog::ironclad;
        use crate::domain::run::{PlayerClass, RunState, starting_relics};
        let mut app = empty_app();
        app.run = Some(RunState::new(
            PlayerClass::Ironclad,
            80, 80,
            ironclad::starter_deck(),
            starting_relics::ironclad(),
        ));
        app.act_filter = "overgrowth".to_string();
        app
    }

    fn make_offer(app: &App, rng: &mut StdRng) -> Vec<Card> {
        let pool = card_pool_for_run(app);
        let mut offset = app.run.as_ref().map(|r| r.rare_offset).unwrap_or(-5);
        sample_offer(&pool, RewardKind::Monster, &mut offset, rng)
    }

    #[test]
    fn card_pick_from_map_adds_to_deck() {
        let mut rng = seeded_rng();
        let mut app = app_with_run();
        let deck_before = app.run.as_ref().unwrap().deck.len();

        let offered = make_offer(&app, &mut rng);
        app.load_pick(offered, &mut rng, AppMode::MapView);
        assert_eq!(app.mode, AppMode::CardPick);
        assert!(!app.offered_cards.is_empty());

        let first_card_rank = app.card_advice.iter().position(|a| a.card_index != usize::MAX).unwrap_or(0);
        app.selected_row = first_card_rank;
        app.handle_event(make_key(KeyCode::Enter), &mut rng);

        assert_eq!(app.mode, AppMode::MapView);
        assert_eq!(app.run.as_ref().unwrap().deck.len(), deck_before + 1);
    }

    #[test]
    fn card_pick_skip_does_not_add_to_deck() {
        let mut rng = seeded_rng();
        let mut app = app_with_run();
        let deck_before = app.run.as_ref().unwrap().deck.len();

        let offered = make_offer(&app, &mut rng);
        app.load_pick(offered, &mut rng, AppMode::MapView);

        let skip_rank = app.card_advice.iter().position(|a| a.card_index == usize::MAX).unwrap();
        app.selected_row = skip_rank;
        app.handle_event(make_key(KeyCode::Enter), &mut rng);

        assert_eq!(app.mode, AppMode::MapView);
        assert_eq!(app.run.as_ref().unwrap().deck.len(), deck_before);
    }

    #[test]
    fn card_pick_from_combat_does_not_modify_deck() {
        let mut rng = seeded_rng();
        let mut app = app_with_run();
        let deck_before = app.run.as_ref().unwrap().deck.len();

        let offered = make_offer(&app, &mut rng);
        app.load_pick(offered, &mut rng, AppMode::CombatAdvice);

        let first_card_rank = app.card_advice.iter().position(|a| a.card_index != usize::MAX).unwrap_or(0);
        app.selected_row = first_card_rank;
        app.handle_event(make_key(KeyCode::Enter), &mut rng);

        assert_eq!(app.mode, AppMode::CombatAdvice);
        assert_eq!(app.run.as_ref().unwrap().deck.len(), deck_before, "preview mode must not modify deck");
    }
}
