use crossterm::event::{KeyCode, KeyEvent};
use rand::SeedableRng;
use rand::rngs::StdRng;

use crate::data::api::{SpireApiAncient, SpireApiCharacter, SpireApiEncounter, SpireApiEvent, SpireApiMonster, SpireApiRelic};
use crate::domain::card::Card;
use crate::domain::catalog;
use crate::domain::combat::CombatState;
use crate::domain::reward::{sample_offer, RewardKind};
use crate::domain::encounter::{encounter_to_combat, encounters_for_act};
use crate::domain::run::{PlayerClass, Relic, RunState, starting_relics};
use crate::input::event::{spawn_event_loop, AppEvent};
use crate::input::manual::{default_combat_state, ManualInputState};
use crate::metrics::card_pick::{sim_pick_score, CardAdvice};
use crate::metrics::path_ev::{compute_path_choices, NodeCosts, PathChoice};
use crate::metrics::run_ev::{compute_run_ev, RunEv};
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
    AncientBoon,
    Victory,
    ManualInput,
    Simulating,
    Exiting,
}

/// An action to take after the current card pick is confirmed.
#[derive(Debug, Clone, PartialEq)]
pub enum PostPickAction {
    BossActTransition,
}

pub struct App {
    pub mode: AppMode,
    pub combat: Option<CombatState>,
    pub run: Option<RunState>,
    pub play_advice: Option<PlayAdvice>,
    pub card_advice: Vec<CardAdvice>,
    pub deck_stats: Option<DeckStats>,
    pub map_ev: Option<MapEvData>,
    pub run_ev: Option<RunEv>,
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
    /// HP-cost estimates for each available map choice (recomputed on position change).
    pub path_choices: Vec<PathChoice>,
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
    /// Where to return when DeckDash is dismissed.
    pub deck_dash_return: AppMode,
    /// Where to return when MapEv is dismissed.
    pub map_ev_return: AppMode,
    /// All relic data loaded at startup.
    pub relics: Vec<SpireApiRelic>,
    /// The single relic on offer in treasure room.
    pub offered_relic: Option<SpireApiRelic>,
    /// Cursor in treasure room: 0 = Take, 1 = Skip.
    pub relic_cursor: usize,
    /// Shop inventory.
    pub shop_cards: Vec<Card>,
    pub shop_card_advice: Vec<CardAdvice>,
    pub shop_card_prices: Vec<u32>,
    pub shop_relics: Vec<SpireApiRelic>,
    pub shop_relic_prices: Vec<u32>,
    /// Index into shop_cards that has the 50% discount (None = no discount shown yet).
    pub shop_discounted_card_idx: Option<usize>,
    pub shop_has_removal: bool,
    pub shop_removal_price: u32,
    pub shop_cursor: usize,
    // Ancient boon state
    /// Ancient data loaded at startup.
    pub ancients: Vec<SpireApiAncient>,
    /// ID of the current ancient (e.g. "NEOW").
    pub ancient_id: String,
    /// Display name of the current ancient.
    pub ancient_name: String,
    /// Boon relics offered by the current ancient.
    pub ancient_boons: Vec<SpireApiRelic>,
    /// Cursor index into ancient_boons.
    pub ancient_cursor: usize,
    /// True when the ancient boon is triggered by an act transition (boss beaten).
    pub ancient_is_act_transition: bool,
    /// Whether Darv appeared in Act 2 of this run (affects Act 3 pool).
    pub ancient_had_darv_act2: bool,
    /// Action to take after the current card pick is confirmed.
    pub post_pick_action: Option<PostPickAction>,
}

impl App {
    pub fn new(
        characters: Vec<SpireApiCharacter>,
        encounters: Vec<SpireApiEncounter>,
        monsters: Vec<SpireApiMonster>,
        events: Vec<SpireApiEvent>,
        relics: Vec<SpireApiRelic>,
        ancients: Vec<SpireApiAncient>,
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
            run_ev: None,
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
            path_choices: Vec::new(),
            rest_advice: None,
            rest_cursor: 0,
            active_event: None,
            event_advice: Vec::new(),
            event_cursor: 0,
            offered_cards: Vec::new(),
            card_pick_return: AppMode::EncounterPick,
            deck_dash_return: AppMode::CombatAdvice,
            map_ev_return:    AppMode::EncounterPick,
            relics,
            offered_relic: None,
            relic_cursor: 0,
            shop_cards: Vec::new(),
            shop_card_advice: Vec::new(),
            shop_card_prices: Vec::new(),
            shop_relics: Vec::new(),
            shop_relic_prices: Vec::new(),
            shop_discounted_card_idx: None,
            shop_has_removal: false,
            shop_removal_price: 75,
            shop_cursor: 0,
            ancients,
            ancient_id: String::new(),
            ancient_name: String::new(),
            ancient_boons: Vec::new(),
            ancient_cursor: 0,
            ancient_is_act_transition: false,
            ancient_had_darv_act2: false,
            post_pick_action: None,
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
            AppMode::CardPick       => self.handle_key_pick(key, rng),
            AppMode::DeckDash       => self.handle_key_deck_dash(key),
            AppMode::MapEv          => self.handle_key_map_ev(key),
            AppMode::AncientBoon    => self.handle_key_ancient_boon(key, rng),
            AppMode::Victory        => self.handle_key_victory(key),
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
            KeyCode::Char('v') => {
                self.open_map_ev(rng);
            }
            KeyCode::Char('d') => {
                self.compute_deck_dash(rng);
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
                self.mode = self.deck_dash_return.clone();
            }
            _ => {}
        }
    }

    fn handle_key_map_ev(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('q') | KeyCode::Char('v') | KeyCode::Esc => {
                self.mode = self.map_ev_return.clone();
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

    fn handle_key_pick(&mut self, key: KeyEvent, rng: &mut impl rand::Rng) {
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => {
                self.post_pick_action = None;
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
                if let Some(action) = self.post_pick_action.take() {
                    match action {
                        PostPickAction::BossActTransition => self.begin_act_transition(rng),
                    }
                }
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
        self.deck_dash_return = self.mode.clone();
        self.mode = AppMode::DeckDash;
    }

    pub fn open_map_ev(&mut self, rng: &mut impl rand::Rng) {
        let deck = self.run.as_ref().map(|r| r.deck.clone()).unwrap_or_else(|| {
            crate::domain::catalog::ironclad::starter_deck()
        });
        let hp = self.run.as_ref().map(|r| r.hp)
            .or_else(|| self.combat.as_ref().map(|c| c.player.hp))
            .unwrap_or(80);
        let max_hp = self.run.as_ref().map(|r| r.max_hp)
            .or_else(|| self.combat.as_ref().map(|c| c.player.max_hp))
            .unwrap_or(80);
        let ascension = self.run.as_ref().map(|r| r.ascension).unwrap_or(0);
        let sub_act = self.run.as_ref().map(|r| r.sub_act.clone())
            .unwrap_or_else(|| self.act_filter.clone());

        let data = compute_map_ev(
            &deck,
            hp,
            max_hp,
            ascension,
            &sub_act,
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
        self.recompute_path_choices();

        // Compute full run projection.
        let best_remaining = if self.path_choices.is_empty() {
            // No more map floors — only the boss remains.
            -self.map_ev.as_ref().map(|me| me.boss.mean_hp_loss).unwrap_or(35.0)
        } else {
            self.path_choices.iter()
                .map(|pc| pc.total_hp_delta)
                .fold(f32::NEG_INFINITY, f32::max)
        };
        let current_simulated = self.path_choices.first().map(|pc| pc.simulated).unwrap_or(false);
        self.run_ev = Some(compute_run_ev(
            &deck,
            hp,
            max_hp,
            &sub_act,
            best_remaining,
            &self.encounters,
            &self.monsters,
            ascension,
            current_simulated,
            rng,
        ));

        self.selected_row = 0;
        self.map_ev_return = self.mode.clone();
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

        let act1_variants = ["overgrowth", "underdocks"];
        let sub_act = act1_variants[rng.gen_range(0..act1_variants.len())].to_string();

        let mut run  = RunState::new(class, hp, hp, deck, relic);
        run.gold     = gold;
        run.sub_act  = sub_act.clone();
        run.act      = crate::metrics::run_ev::act_number(&sub_act);

        self.status_message = format!(
            "Playing as {} — HP {}, {} starting cards — choose your path",
            char_name, hp, run.deck.len(),
        );
        self.run = Some(run);
        self.act_filter = sub_act.clone();
        self.refresh_filter();
        self.selected_row = 0;
        self.open_ancient_boon(&sub_act, false, rng);
    }

    /// Generate the map (if not already present) and switch to MapView.
    pub fn open_map_view(&mut self, rng: &mut impl rand::Rng) {
        if let Some(ref mut run) = self.run {
            if run.map.is_none() {
                let seed: u64 = rng.r#gen();
                run.generate_map(seed, run.ascension);
            }
        }
        self.map_cursor = 0;
        self.recompute_path_choices();
        self.mode = AppMode::MapView;
    }

    /// Recompute per-choice HP path costs using simulated or default node costs.
    pub fn recompute_path_choices(&mut self) {
        use crate::domain::map::ROWS;
        let Some(ref run) = self.run else { self.path_choices.clear(); return; };
        let Some(ref map) = run.map else { self.path_choices.clear(); return; };

        let max_hp = run.max_hp;
        let ascension = run.ascension;
        let (simulated, costs) = if let Some(ref me) = self.map_ev {
            (true, NodeCosts::from_map_ev(me, max_hp, ascension))
        } else {
            (false, NodeCosts::defaults(max_hp, ascension))
        };

        let (choices, next_floor) = match run.map_pos {
            None => (map.entry_nodes(), 0usize),
            Some(pos) => {
                let nf = pos.floor + 1;
                (map.choices_from(pos).to_vec(), nf)
            }
        };

        self.path_choices = if next_floor < ROWS {
            compute_path_choices(map, &choices, next_floor, &costs, simulated)
        } else {
            vec![]
        };
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
        drop(run);
        self.recompute_path_choices();

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
                self.apply_post_combat_relics();
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
                self.apply_post_combat_relics();
                self.post_pick_action = Some(PostPickAction::BossActTransition);
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
        use crate::domain::card::{CardType, Rarity};
        use crate::metrics::card_pick::pick_score;

        let class_color = self.run.as_ref().map(|r| char_color(&r.class)).unwrap_or("ironclad");
        let class_pool = catalog::cards_for_character(class_color);

        // ── 5 colored cards: 2 Attack + 2 Skill + 1 Power ─────────────────
        let mut shop_cards: Vec<Card> = Vec::with_capacity(7);
        let mut shop_card_prices: Vec<u32> = Vec::with_capacity(7);

        let card_slots: &[(CardType, usize)] = &[
            (CardType::Attack, 2),
            (CardType::Skill, 2),
            (CardType::Power, 1),
        ];
        for (ct, count) in card_slots {
            let type_pool: Vec<&Card> = class_pool.iter()
                .filter(|c| c.card_type == *ct
                    && matches!(c.rarity, Rarity::Common | Rarity::Uncommon | Rarity::Rare))
                .collect();
            for _ in 0..*count {
                let rarity = shop_card_rarity(rng);
                let rarity_pool: Vec<&Card> = type_pool.iter()
                    .filter(|c| c.rarity == rarity)
                    .copied()
                    .collect();
                let pick_pool: &[&Card] = if rarity_pool.is_empty() { &type_pool } else { &rarity_pool };
                if let Some(card) = pick_pool.choose(rng).map(|c| (*c).clone()) {
                    let price = colored_card_price(&rarity, rng);
                    shop_cards.push(card);
                    shop_card_prices.push(price);
                }
            }
        }

        // ── 2 colorless cards: 1 Uncommon + 1 Rare ────────────────────────
        let colorless_pool = catalog::colorless::all_cards();
        for target_rarity in &[Rarity::Uncommon, Rarity::Rare] {
            let rarity_pool: Vec<&Card> = colorless_pool.iter()
                .filter(|c| c.rarity == *target_rarity)
                .collect();
            if let Some(card) = rarity_pool.choose(rng).map(|c| (*c).clone()) {
                let price = colorless_card_price(target_rarity, rng);
                shop_cards.push(card);
                shop_card_prices.push(price);
            }
        }

        // ── 3 relics: 2 weighted + 1 Shop Relic always ────────────────────
        const SHOP_BLACKLIST: &[&str] = &[
            "AMETHYST_AUBERGINE", "BOWLER_HAT", "LUCKY_FYSH", "OLD_COIN", "THE_COURIER",
        ];
        let relic_regular_pool: Vec<&SpireApiRelic> = self.relics.iter().filter(|r| {
            let rarity = r.rarity.as_deref().unwrap_or("");
            let p = r.pool.as_deref().unwrap_or("shared");
            matches!(rarity, "Common Relic" | "Uncommon Relic" | "Rare Relic")
                && (p == "shared" || p == class_color)
                && !SHOP_BLACKLIST.contains(&r.id.as_str())
        }).collect();

        let mut shop_relics: Vec<SpireApiRelic> = Vec::with_capacity(3);
        let mut shop_relic_prices: Vec<u32> = Vec::with_capacity(3);

        for _ in 0..2 {
            let rarity_str = relic_regular_rarity(rng);
            let already_taken: std::collections::HashSet<&str> = shop_relics.iter().map(|r| r.id.as_str()).collect();
            // Prefer requested rarity; fall back to any rarity if the bucket is empty.
            let filtered: Vec<&SpireApiRelic> = relic_regular_pool.iter()
                .filter(|r| r.rarity.as_deref().unwrap_or("") == rarity_str && !already_taken.contains(r.id.as_str()))
                .copied()
                .collect();
            let fallback: Vec<&SpireApiRelic> = relic_regular_pool.iter()
                .filter(|r| !already_taken.contains(r.id.as_str()))
                .copied()
                .collect();
            let pick_pool = if filtered.is_empty() { &fallback } else { &filtered };
            if let Some(r) = pick_pool.choose(rng).map(|r| (*r).clone()) {
                let price = relic_price(r.rarity.as_deref().unwrap_or(""), rng);
                shop_relics.push(r);
                shop_relic_prices.push(price);
            }
        }

        // Rightmost relic is always a Shop Relic
        let shop_relic_pool: Vec<&SpireApiRelic> = self.relics.iter().filter(|r| {
            let rarity = r.rarity.as_deref().unwrap_or("");
            let p = r.pool.as_deref().unwrap_or("shared");
            rarity == "Shop Relic" && (p == "shared" || p == class_color)
        }).collect();
        if let Some(r) = shop_relic_pool.choose(rng).map(|r| (*r).clone()) {
            let price: u32 = rng.gen_range(170..=230);
            shop_relics.push(r);
            shop_relic_prices.push(price);
        }

        // ── One random card gets 50% discount ─────────────────────────────
        let discounted_idx = if !shop_cards.is_empty() {
            let idx = rng.gen_range(0..shop_cards.len());
            shop_card_prices[idx] = (shop_card_prices[idx] / 2).max(1);
            Some(idx)
        } else {
            None
        };

        // ── Removal service ────────────────────────────────────────────────
        let removals = self.run.as_ref().map(|r| r.card_removals_bought).unwrap_or(0);
        let removal_price = 75 + 25 * removals;

        // ── Score the cards ────────────────────────────────────────────────
        let advice = if let Some(run) = &self.run {
            pick_score(&shop_cards, run)
        } else {
            shop_cards.iter().enumerate().map(|(i, _)| crate::metrics::card_pick::CardAdvice {
                card_index: i, score: 0.0, reason: String::new(),
                win_rate: 0.0, mean_hp_delta: 0.0, hp_loss_p50: 0.0,
                delta_win_rate: 0.0, delta_hp: 0.0,
            }).collect()
        };

        self.shop_cards = shop_cards;
        self.shop_card_advice = advice;
        self.shop_card_prices = shop_card_prices;
        self.shop_relics = shop_relics;
        self.shop_relic_prices = shop_relic_prices;
        self.shop_discounted_card_idx = discounted_idx;
        self.shop_has_removal = true;
        self.shop_removal_price = removal_price;
        self.shop_cursor = 0;
        self.mode = AppMode::Shop;
    }

    fn handle_key_shop(&mut self, key: KeyEvent) {
        let n_cards = self.shop_cards.len();
        let n_relics = self.shop_relics.len();
        let has_removal = self.shop_has_removal;
        let total = n_cards + n_relics + if has_removal { 1 } else { 0 };
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
                    let price = self.shop_card_prices.get(self.shop_cursor).copied().unwrap_or(0);
                    if let (Some(card), Some(run)) = (card, self.run.as_mut()) {
                        if run.gold < price {
                            self.status_message = format!("Not enough gold ({}/{}g)", run.gold, price);
                            return;
                        }
                        run.gold -= price;
                        let name = card.name.clone();
                        run.deck.push(card);
                        self.status_message = format!("Bought {} for {}g ({} cards, {}g left)", name, price, run.deck.len(), run.gold);
                        self.shop_cards.remove(self.shop_cursor);
                        self.shop_card_prices.remove(self.shop_cursor);
                        if let Some(ref mut di) = self.shop_discounted_card_idx {
                            if *di == self.shop_cursor {
                                self.shop_discounted_card_idx = None;
                            } else if *di > self.shop_cursor {
                                *di -= 1;
                            }
                        }
                        self.shop_card_advice.retain(|a| a.card_index != self.shop_cursor);
                        if self.shop_cursor >= self.shop_cards.len() && self.shop_cursor > 0 {
                            self.shop_cursor -= 1;
                        }
                    }
                } else if self.shop_cursor < n_cards + n_relics {
                    let relic_idx = self.shop_cursor - n_cards;
                    let relic = self.shop_relics.get(relic_idx).cloned();
                    let price = self.shop_relic_prices.get(relic_idx).copied().unwrap_or(0);
                    if let (Some(relic), Some(run)) = (relic, self.run.as_mut()) {
                        if run.gold < price {
                            self.status_message = format!("Not enough gold ({}/{}g)", run.gold, price);
                            return;
                        }
                        run.gold -= price;
                        let desc = relic.description.clone().unwrap_or_default();
                        run.relics.push(crate::domain::run::Relic::new(&relic.name, desc));
                        self.status_message = format!("Bought {} for {}g ({}g left)", relic.name, price, run.gold);
                        self.shop_relics.remove(relic_idx);
                        self.shop_relic_prices.remove(relic_idx);
                        let new_total = self.shop_cards.len() + self.shop_relics.len() + if self.shop_has_removal { 1 } else { 0 };
                        if self.shop_cursor >= new_total && self.shop_cursor > 0 {
                            self.shop_cursor -= 1;
                        }
                    }
                } else if has_removal && self.shop_cursor == n_cards + n_relics {
                    let price = self.shop_removal_price;
                    if let Some(run) = self.run.as_mut() {
                        if run.gold < price {
                            self.status_message = format!("Not enough gold ({}/{}g)", run.gold, price);
                            return;
                        }
                        run.gold -= price;
                        run.card_removals_bought += 1;
                        self.shop_removal_price = 75 + 25 * run.card_removals_bought;
                        self.shop_has_removal = false;
                        self.status_message = format!("Card removal purchased for {}g ({}g left)", price, run.gold);
                        if self.shop_cursor > 0 { self.shop_cursor -= 1; }
                    }
                }
            }
            KeyCode::Esc => {
                self.shop_cards.clear();
                self.shop_card_advice.clear();
                self.shop_card_prices.clear();
                self.shop_relics.clear();
                self.shop_relic_prices.clear();
                self.shop_discounted_card_idx = None;
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

    /// Apply relics that trigger at the end of combat (e.g. Burning Blood).
    fn apply_post_combat_relics(&mut self) {
        let Some(ref mut run) = self.run else { return; };
        for relic in run.relics.clone() {
            match relic.name.as_str() {
                "Burning Blood" => {
                    let healed = run.hp.saturating_add(6).min(run.max_hp);
                    if healed > run.hp {
                        run.hp = healed;
                    }
                }
                _ => {}
            }
        }
    }

    /// Open the ancient boon selection screen for the given sub-act.
    pub fn open_ancient_boon(&mut self, sub_act: &str, is_act_transition: bool, rng: &mut impl rand::Rng) {
        use crate::domain::ancient::select_act_ancient;

        let had_darv = self.ancient_had_darv_act2;
        let ancient_id = select_act_ancient(sub_act, had_darv, rng);
        let boons = crate::domain::ancient::sample_boons(
            ancient_id,
            &self.ancients.clone(),
            &self.relics.clone(),
            rng,
        );

        if ancient_id == "DARV" && sub_act == "hive" {
            self.ancient_had_darv_act2 = true;
        }

        self.ancient_id = ancient_id.to_string();
        self.ancient_name = self
            .ancients
            .iter()
            .find(|a| a.id.eq_ignore_ascii_case(ancient_id))
            .map(|a| a.name.clone())
            .unwrap_or_else(|| ancient_id.to_string());
        self.ancient_boons = boons;
        self.ancient_cursor = 0;
        self.ancient_is_act_transition = is_act_transition;

        // Ancient heals player to full HP
        if let Some(ref mut run) = self.run {
            run.hp = run.max_hp;
        }

        self.status_message = format!("You meet {}. Choose a boon.", self.ancient_name);
        self.mode = AppMode::AncientBoon;
    }

    fn handle_key_ancient_boon(&mut self, key: KeyEvent, rng: &mut impl rand::Rng) {
        let n = self.ancient_boons.len();
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => {
                if n > 0 {
                    self.ancient_cursor = (self.ancient_cursor + 1).min(n - 1);
                }
            }
            KeyCode::Char('k') | KeyCode::Up => {
                if self.ancient_cursor > 0 {
                    self.ancient_cursor -= 1;
                }
            }
            KeyCode::Enter => self.confirm_ancient_boon(rng),
            KeyCode::Char('q') => self.mode = AppMode::Exiting,
            _ => {}
        }
    }

    fn confirm_ancient_boon(&mut self, rng: &mut impl rand::Rng) {
        // Add the chosen relic to run
        if let Some(relic) = self.ancient_boons.get(self.ancient_cursor).cloned() {
            if let Some(ref mut run) = self.run {
                let desc = relic.description.clone().unwrap_or_default();
                run.relics.push(crate::domain::run::Relic::new(&relic.name, desc));
            }
            self.status_message = format!("Took boon: {}", relic.name);
        }
        self.ancient_boons.clear();

        if self.ancient_is_act_transition {
            // Generate map for the new act and go to map view
            self.open_map_view(rng);
        } else {
            // Starting boon — go to map view
            self.open_map_view(rng);
        }
    }

    /// Handle act transition after a boss fight.
    fn begin_act_transition(&mut self, rng: &mut impl rand::Rng) {
        let current_sub_act = self.run.as_ref().map(|r| r.sub_act.clone()).unwrap_or_default();
        let future = crate::metrics::run_ev::acts_after(&current_sub_act);
        let Some(&(next_sub_act, _)) = future.first() else {
            // No more acts — victory
            self.open_victory_screen();
            return;
        };

        let ascension = self.run.as_ref().map(|r| r.ascension).unwrap_or(0);

        if let Some(ref mut run) = self.run {
            // Boss gold reward
            let boss_gold = if ascension >= 3 { 75 } else { 100 };
            run.gold += boss_gold;
            // Update act
            run.sub_act = next_sub_act.to_string();
            run.act = crate::metrics::run_ev::act_number(next_sub_act);
            // Reset map
            run.map = None;
            run.map_pos = None;
            run.floor = 0;
        }

        self.act_filter = next_sub_act.to_string();
        self.refresh_filter();
        self.map_ev = None;
        self.run_ev = None;
        self.path_choices.clear();
        self.open_ancient_boon(next_sub_act, true, rng);
    }

    pub fn open_victory_screen(&mut self) {
        self.status_message = "You have conquered the Spire!".to_string();
        self.mode = AppMode::Victory;
    }

    fn handle_key_victory(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('q') => self.mode = AppMode::Exiting,
            KeyCode::Char('n') | KeyCode::Enter => {
                self.run = None;
                self.map_ev = None;
                self.run_ev = None;
                self.path_choices.clear();
                self.ancient_had_darv_act2 = false;
                self.selected_row = 0;
                self.mode = if self.characters.is_empty() {
                    AppMode::EncounterPick
                } else {
                    AppMode::CharacterPick
                };
                self.status_message = "Choose your character.".to_string();
            }
            _ => {}
        }
    }
}

/// Return 3 random cards from the current character's pool as a simulated reward.
fn card_pool_for_run(app: &App) -> Vec<Card> {
    let color = app.run.as_ref().map(|r| char_color(&r.class)).unwrap_or("ironclad");
    catalog::cards_for_character(color)
}

/// Pick a rarity for a shop card: 54% Common, 37% Uncommon, 9% Rare.
fn shop_card_rarity(rng: &mut impl rand::Rng) -> crate::domain::card::Rarity {
    use crate::domain::card::Rarity;
    let roll: u32 = rng.gen_range(0..100);
    if roll < 54 { Rarity::Common } else if roll < 91 { Rarity::Uncommon } else { Rarity::Rare }
}

/// Price for a colored card at shop.
fn colored_card_price(rarity: &crate::domain::card::Rarity, rng: &mut impl rand::Rng) -> u32 {
    use crate::domain::card::Rarity;
    match rarity {
        Rarity::Common   => rng.gen_range(48..=53),
        Rarity::Uncommon => rng.gen_range(71..=79),
        Rarity::Rare     => rng.gen_range(143..=158),
        _                => rng.gen_range(48..=53),
    }
}

/// Price for a colorless card at shop.
fn colorless_card_price(rarity: &crate::domain::card::Rarity, rng: &mut impl rand::Rng) -> u32 {
    use crate::domain::card::Rarity;
    match rarity {
        Rarity::Uncommon => rng.gen_range(82..=90),
        Rarity::Rare     => rng.gen_range(164..=181),
        _                => rng.gen_range(82..=90),
    }
}

/// Pick a rarity string for a regular shop relic: 50% Common, 33% Uncommon, 17% Rare.
fn relic_regular_rarity(rng: &mut impl rand::Rng) -> &'static str {
    let roll: u32 = rng.gen_range(0..100);
    if roll < 50 { "Common Relic" } else if roll < 83 { "Uncommon Relic" } else { "Rare Relic" }
}

/// Price for a shop relic based on its rarity string.
fn relic_price(rarity: &str, rng: &mut impl rand::Rng) -> u32 {
    match rarity {
        "Common Relic"   => rng.gen_range(149..=201),
        "Uncommon Relic" => rng.gen_range(191..=259),
        "Rare Relic"     => rng.gen_range(234..=316),
        _                => rng.gen_range(149..=201),
    }
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
    let ancients = crate::data::api::load_ancients();

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;

    let backend = CrosstermBackend::new(io::stdout());
    let mut terminal = Terminal::new(backend)?;

    let mut app = App::new(characters, encounters, monsters, events, relics, ancients);
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
        App::new(vec![], vec![], vec![], vec![], vec![], vec![])
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
        let mut app = App::new(vec![], vec![enc], vec![], vec![], vec![], vec![]);
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
        let mut app = App::new(vec![], vec![make_enc("a"), make_enc("b"), make_enc("c")], vec![], vec![], vec![], vec![]);
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
