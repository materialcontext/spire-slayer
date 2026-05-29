use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Paragraph, Wrap},
    Frame,
};

use crate::metrics::combat::threat_score;
use crate::metrics::deck_dash::dist_bar;
use crate::tui::app::{App, AppMode};
use crate::tui::widgets::{advice_spans, enemy_row, intent_label_owned};

pub fn render(frame: &mut Frame, app: &App) {
    let area = frame.area();

    match app.mode {
        AppMode::CharacterPick => render_character_pick(frame, app, area),
        AppMode::EncounterPick => render_encounter_pick(frame, app, area),
        AppMode::ManualInput => render_input(frame, app, area),
        AppMode::CombatAdvice | AppMode::Simulating => render_combat(frame, app, area),
        AppMode::CardPick => render_card_pick(frame, app, area),
        AppMode::DeckDash => render_deck_dash(frame, app, area),
        AppMode::MapEv   => render_map_ev(frame, app, area),
        AppMode::MapView  => render_map_view(frame, app, area),
        AppMode::RestSite     => render_rest_site(frame, app, area),
        AppMode::EventRoom    => render_event_room(frame, app, area),
        AppMode::TreasureRoom => render_treasure_room(frame, app, area),
        AppMode::Shop         => render_shop(frame, app, area),
        AppMode::AncientBoon  => render_ancient_boon(frame, app, area),
        AppMode::Victory      => render_victory(frame, app, area),
        AppMode::Exiting      => {}
    }
}

fn render_combat(frame: &mut Frame, app: &App, area: Rect) {
    // Outer layout: title + body + status bar
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2),  // title
            Constraint::Min(8),     // main body
            Constraint::Length(4),  // recommendation
            Constraint::Length(1),  // status bar
        ])
        .split(area);

    // Title
    let title_text = match &app.run {
        Some(r) => format!(
            " COMBAT ADVICE   Floor {} • Act {} • Turn {}",
            r.floor,
            r.act,
            app.combat.as_ref().map(|c| c.turn).unwrap_or(1)
        ),
        None => format!(
            " COMBAT ADVICE   Turn {}",
            app.combat.as_ref().map(|c| c.turn).unwrap_or(1)
        ),
    };
    let title = Paragraph::new(title_text)
        .block(Block::default().borders(Borders::BOTTOM))
        .style(Style::default().fg(Color::White).add_modifier(Modifier::BOLD));
    frame.render_widget(title, rows[0]);

    // Main body: hand left, enemies+player right
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(40), Constraint::Percentage(60)])
        .split(rows[1]);

    render_hand(frame, app, cols[0]);
    render_enemies(frame, app, cols[1]);

    // Recommendation
    render_advice(frame, app, rows[2]);

    // Status bar
    render_statusbar(frame, app, rows[3]);
}

fn render_hand(frame: &mut Frame, app: &App, area: Rect) {
    let Some(ref combat) = app.combat else {
        frame.render_widget(
            Paragraph::new("No combat state").block(Block::default().title("HAND").borders(Borders::ALL)),
            area,
        );
        return;
    };

    let mut lines: Vec<Line<'static>> = Vec::new();
    for (i, card) in combat.hand.iter().enumerate() {
        let is_recommended = app
            .play_advice
            .as_ref()
            .map(|a| a.actions.iter().any(|act| act.card_hand_idx == i))
            .unwrap_or(false);
        let (prefix, style) = if i == app.selected_row {
            ("> ", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD))
        } else if is_recommended {
            ("  ", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD))
        } else {
            ("  ", Style::default().fg(Color::White))
        };
        let playable = card.is_playable(combat.energy);
        let cost_str = if card.cost == 255 { "X".to_string() } else { card.cost.to_string() };
        let dim = if !playable { Style::default().fg(Color::DarkGray) } else { style };
        lines.push(Line::from(Span::styled(
            format!("{}[{}]  {}e", prefix, card.name, cost_str),
            dim,
        )));
    }
    if lines.is_empty() {
        lines.push(Line::from(Span::raw("  (empty hand)")));
    }

    let block = Block::default()
        .title(format!(" HAND ({} cards) ", combat.hand.len()))
        .borders(Borders::ALL);
    let para = Paragraph::new(Text::from(lines)).block(block);
    frame.render_widget(para, area);
}

fn render_enemies(frame: &mut Frame, app: &App, area: Rect) {
    let Some(ref combat) = app.combat else {
        frame.render_widget(
            Paragraph::new("No combat state").block(Block::default().title("ENEMIES").borders(Borders::ALL)),
            area,
        );
        return;
    };

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
        .split(area);

    // Enemies panel
    let mut enemy_lines: Vec<Line<'static>> = Vec::new();
    for (i, enemy) in combat.enemies.iter().enumerate() {
        if !enemy.is_alive() {
            enemy_lines.push(Line::from(Span::styled(
                format!("  {} [DEAD]", enemy.name),
                Style::default().fg(Color::DarkGray),
            )));
            continue;
        }
        enemy_lines.push(enemy_row(enemy, i));
    }
    if enemy_lines.is_empty() {
        enemy_lines.push(Line::from(Span::raw("  (no enemies)")));
    }
    let enemy_block = Block::default().title(" ENEMIES ").borders(Borders::ALL);
    frame.render_widget(
        Paragraph::new(Text::from(enemy_lines)).block(enemy_block),
        rows[0],
    );

    // Player panel
    let hp_ratio = if combat.player.max_hp == 0 {
        0.0
    } else {
        (combat.player.hp as f64 / combat.player.max_hp as f64).clamp(0.0, 1.0)
    };
    let hp_color = if hp_ratio > 0.5 {
        Color::Green
    } else if hp_ratio > 0.25 {
        Color::Yellow
    } else {
        Color::Red
    };
    let player_text = vec![
        Line::from(vec![
            Span::raw("  HP: "),
            Span::styled(
                format!("{}/{}", combat.player.hp, combat.player.max_hp),
                Style::default().fg(hp_color),
            ),
            Span::raw(format!("  BLK: ")),
            Span::styled(
                format!("{}", combat.player.block),
                Style::default().fg(Color::Blue),
            ),
            Span::raw(format!("  Energy: {}/{}", combat.energy, combat.energy_max)),
        ]),
        Line::from(Span::raw(format!(
            "  Draw: {}  Discard: {}  Turn: {}",
            combat.draw_pile.len(),
            combat.discard_pile.len(),
            combat.turn
        ))),
    ];
    let player_block = Block::default().title(" PLAYER ").borders(Borders::ALL);
    frame.render_widget(
        Paragraph::new(Text::from(player_text)).block(player_block),
        rows[1],
    );
}

fn render_advice(frame: &mut Frame, app: &App, area: Rect) {
    let block = Block::default().title(" RECOMMENDATION ").borders(Borders::ALL);

    if app.mode == AppMode::Simulating {
        let para = Paragraph::new("  Simulating...").block(block);
        frame.render_widget(para, area);
        return;
    }

    let content = match (&app.play_advice, &app.combat) {
        (Some(advice), Some(combat)) => advice_spans(advice, &combat.hand),
        _ => Text::raw("  Press [s] to simulate"),
    };
    frame.render_widget(Paragraph::new(content).block(block).wrap(Wrap { trim: false }), area);
}

fn render_statusbar(frame: &mut Frame, app: &App, area: Rect) {
    let threat_label = app
        .combat
        .as_ref()
        .map(|c| {
            let ts = threat_score(c);
            if ts >= 0.6 {
                ("HIGH", Color::Red)
            } else if ts >= 0.3 {
                ("MEDIUM", Color::Yellow)
            } else {
                ("LOW", Color::Green)
            }
        })
        .unwrap_or(("—", Color::DarkGray));

    let status = if app.status_message.is_empty() {
        "".to_string()
    } else {
        format!("  {}", app.status_message)
    };

    let hint = match app.mode {
        AppMode::MapView => "  [v]ev  [d]eck  [j/k]cursor  [Enter]enter  [t/Esc]back  [q]uit",
        _                => "  [s]im  [d]eck  [p]ick  [v]ev  [e]dit  [n]ew  [q]uit",
    };
    let line = Line::from(vec![
        Span::raw(hint),
        Span::raw(status),
        Span::raw("          Threat: "),
        Span::styled(threat_label.0, Style::default().fg(threat_label.1).add_modifier(Modifier::BOLD)),
    ]);
    frame.render_widget(Paragraph::new(line), area);
}

fn render_input(frame: &mut Frame, app: &App, area: Rect) {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2),
            Constraint::Min(10),
            Constraint::Length(2),
        ])
        .split(area);

    let title = Paragraph::new(" MANUAL INPUT — Enter combat state")
        .block(Block::default().borders(Borders::BOTTOM))
        .style(Style::default().fg(Color::White).add_modifier(Modifier::BOLD));
    frame.render_widget(title, rows[0]);

    let Some(ref input) = app.input else {
        frame.render_widget(Paragraph::new("  No input state"), rows[1]);
        return;
    };

    let mut lines: Vec<Line<'static>> = Vec::new();
    lines.push(Line::from(Span::styled(
        format!("  {} ", input.field_label()),
        Style::default().fg(Color::Yellow),
    )));
    lines.push(Line::from(Span::styled(
        format!("  > {}█", input.buffer),
        Style::default().fg(Color::White).add_modifier(Modifier::BOLD),
    )));
    if !app.status_message.is_empty() {
        lines.push(Line::from(Span::styled(
            format!("  ! {}", app.status_message),
            Style::default().fg(Color::Red),
        )));
    }
    lines.push(Line::from(Span::raw("")));
    lines.push(Line::from(Span::raw("  Current state:")));

    let b = &input.base;
    lines.push(Line::from(Span::raw(format!(
        "    Player HP: {}/{}  Energy: {}/{}  Turn: {}",
        b.player.hp, b.player.max_hp, b.energy, b.energy_max, b.turn
    ))));
    lines.push(Line::from(Span::raw(format!(
        "    Draw pile: {}  Hand: {}  Discard: {}",
        b.draw_pile.len(),
        b.hand.len(),
        b.discard_pile.len()
    ))));
    for (i, enemy) in b.enemies.iter().enumerate() {
        lines.push(Line::from(Span::raw(format!(
            "    Enemy {}: {} HP:{}/{}  BLK:{}  Intent:{}",
            i,
            enemy.name,
            enemy.hp,
            enemy.max_hp,
            enemy.block,
            intent_label_owned(&enemy.intent)
        ))));
    }

    frame.render_widget(
        Paragraph::new(Text::from(lines))
            .block(Block::default().borders(Borders::ALL))
            .wrap(Wrap { trim: false }),
        rows[1],
    );

    let hint = Paragraph::new(
        "  [Enter] confirm field  [Tab] skip  [q] quit  [Backspace] delete",
    )
    .style(Style::default().fg(Color::DarkGray));
    frame.render_widget(hint, rows[2]);
}

fn render_card_pick(frame: &mut Frame, app: &App, area: Rect) {
    use crate::domain::card::{CardType, Rarity};

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(2), Constraint::Min(6), Constraint::Length(2)])
        .split(area);

    let from_map = app.card_pick_return == AppMode::MapView;
    let title_suffix = if from_map { "— pick to add to deck" } else { "— preview only" };
    let title = Paragraph::new(format!(" CARD REWARD {title_suffix}"))
        .block(Block::default().borders(Borders::BOTTOM))
        .style(Style::default().fg(Color::White).add_modifier(Modifier::BOLD));
    frame.render_widget(title, rows[0]);

    let mut lines: Vec<Line<'static>> = Vec::new();
    for (rank, advice) in app.card_advice.iter().enumerate() {
        let selected = rank == app.selected_row;
        let prefix = if selected { "▶ " } else { "  " };

        let (name_line, detail_line) = if advice.card_index == usize::MAX {
            (
                format!("{prefix}— skip reward —"),
                format!("     (keep current deck as-is)"),
            )
        } else {
            let card = app.offered_cards.get(advice.card_index);
            let name = card.map(|c| c.name.as_str()).unwrap_or("?");
            let cost = card.map(|c| if c.cost == 255 { "X".to_string() } else { c.cost.to_string() })
                .unwrap_or_else(|| "?".to_string());
            let ctype = card.map(|c| match c.card_type {
                CardType::Attack => "Atk",
                CardType::Skill  => "Skl",
                CardType::Power  => "Pwr",
                CardType::Status => "Sts",
                CardType::Curse  => "Crs",
            }).unwrap_or("?");
            let rarity = card.map(|c| match c.rarity {
                Rarity::Ancient  => "Ancient",
                Rarity::Rare     => "Rare",
                Rarity::Uncommon => "Uncommon",
                Rarity::Common   => "Common",
                Rarity::Basic    => "Basic",
                Rarity::Special  => "Special",
            }).unwrap_or("?");
            let dmg = card.map(|c| c.base_damage()).unwrap_or(0);
            let blk = card.map(|c| c.base_block()).unwrap_or(0);
            let stats = match (dmg, blk) {
                (d, b) if d > 0 && b > 0 => format!("  {d}dmg {b}blk"),
                (d, 0) if d > 0           => format!("  {d}dmg"),
                (0, b) if b > 0           => format!("  {b}blk"),
                _                         => String::new(),
            };
            (
                format!("{prefix}{name}   [{ctype} · {rarity} · {cost}E]{stats}"),
                format!("     {}", advice.reason),
            )
        };

        let delta_color = if advice.delta_win_rate > 0.02 {
            Color::Green
        } else if advice.delta_win_rate < -0.02 {
            Color::Red
        } else {
            Color::Yellow
        };
        let base_style = if selected {
            Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::White)
        };
        let detail_style = if selected {
            base_style
        } else if advice.win_rate > 0.0 || advice.card_index == usize::MAX {
            Style::default().fg(delta_color)
        } else {
            Style::default().fg(Color::DarkGray)
        };

        lines.push(Line::from(Span::styled(name_line, base_style)));
        lines.push(Line::from(Span::styled(detail_line, detail_style)));
        lines.push(Line::from(Span::raw("")));
    }
    if lines.is_empty() {
        lines.push(Line::from(Span::raw("  No cards offered")));
    }

    frame.render_widget(
        Paragraph::new(Text::from(lines))
            .block(Block::default().title(" OFFERED CARDS (ranked by sim) ").borders(Borders::ALL)),
        rows[1],
    );

    let action = if from_map { "Enter=add to deck" } else { "Enter=close" };
    let hint = Paragraph::new(format!("  [j/k] navigate  [{action}]  [q/Esc] back"))
        .style(Style::default().fg(Color::DarkGray));
    frame.render_widget(hint, rows[2]);
}

fn render_deck_dash(frame: &mut Frame, app: &App, area: Rect) {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2), // title
            Constraint::Min(12),   // stats body
            Constraint::Length(1), // hint
        ])
        .split(area);

    let title = Paragraph::new(" DECK DASHBOARD")
        .block(Block::default().borders(Borders::BOTTOM))
        .style(Style::default().fg(Color::White).add_modifier(Modifier::BOLD));
    frame.render_widget(title, rows[0]);

    let mut lines: Vec<Line<'static>> = Vec::new();

    if let Some(ref s) = app.deck_stats {
        // ── Deck properties ────────────────────────────────────────────────
        lines.push(Line::from(Span::styled(
            format!(
                "  Deck: {} cards   Cycle: {:.1}t   Cost: {:.1}/e   Attacks: {:.0}%   Block cards: {}",
                s.deck_size,
                s.cycle_turns,
                s.mean_energy_cost,
                s.attack_fraction * 100.0,
                s.block_card_count,
            ),
            Style::default().fg(Color::White),
        )));
        lines.push(Line::from(Span::styled(
            format!(
                "  Synergy axes: {}   Quality score: {:.2}",
                s.synergy_axes, s.heuristic_score
            ),
            Style::default().fg(Color::DarkGray),
        )));
        lines.push(Line::from(Span::raw("")));

        if s.encounter_count > 0 {
            let panel_label = format!(
                "  {} panel ({} encounters × {} sims = {} samples):",
                s.sub_act,
                s.encounter_count,
                s.playout_count / s.encounter_count as u32,
                s.playout_count,
            );
            lines.push(Line::from(Span::styled(
                panel_label,
                Style::default().fg(Color::DarkGray),
            )));
            lines.push(Line::from(Span::raw("")));

            // ── DPT ───────────────────────────────────────────────────────
            lines.push(Line::from(Span::styled(
                "  Damage Per Turn:",
                Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
            )));
            let dbar = dist_bar(s.dpt_p10, s.dpt_p50, s.dpt_p90);
            let dpt_color = if s.dpt_p50 >= 10.0 {
                Color::Green
            } else if s.dpt_p50 >= 6.0 {
                Color::Yellow
            } else {
                Color::Red
            };
            lines.push(Line::from(Span::styled(
                format!(
                    "    p10={:.0}  p50={:.0}  p90={:.0}  mean={:.1}",
                    s.dpt_p10, s.dpt_p50, s.dpt_p90, s.mean_dpt
                ),
                Style::default().fg(dpt_color),
            )));
            lines.push(Line::from(Span::styled(
                format!("    [{}]  0──────────{:.0}", dbar, s.dpt_p90),
                Style::default().fg(Color::DarkGray),
            )));
            lines.push(Line::from(Span::raw("")));

            // ── BPT ───────────────────────────────────────────────────────
            lines.push(Line::from(Span::styled(
                "  Block Per Turn:",
                Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
            )));
            let bbar = dist_bar(s.bpt_p10, s.bpt_p50, s.bpt_p90);
            let bpt_color = if s.bpt_p50 >= 8.0 {
                Color::Green
            } else if s.bpt_p50 >= 4.0 {
                Color::Yellow
            } else {
                Color::Red
            };
            lines.push(Line::from(Span::styled(
                format!(
                    "    p10={:.0}  p50={:.0}  p90={:.0}  mean={:.1}",
                    s.bpt_p10, s.bpt_p50, s.bpt_p90, s.mean_bpt
                ),
                Style::default().fg(bpt_color),
            )));
            lines.push(Line::from(Span::styled(
                format!("    [{}]  0──────────{:.0}", bbar, s.bpt_p90),
                Style::default().fg(Color::DarkGray),
            )));
            lines.push(Line::from(Span::raw("")));

            // ── Outcomes ──────────────────────────────────────────────────
            let outcome_color = if s.kill_rate >= 0.7 {
                Color::Green
            } else if s.kill_rate >= 0.4 {
                Color::Yellow
            } else {
                Color::Red
            };
            lines.push(Line::from(Span::styled(
                format!(
                    "  Kill rate: {:.0}%   Survival: {:.0}%   Avg HP loss: {:.0}",
                    s.kill_rate * 100.0,
                    s.survival_rate * 100.0,
                    s.mean_hp_loss
                ),
                Style::default().fg(outcome_color),
            )));
        } else {
            lines.push(Line::from(Span::styled(
                "  (No encounter data — load seed files to enable sim stats)",
                Style::default().fg(Color::DarkGray),
            )));
        }
    } else {
        lines.push(Line::from(Span::raw("  No deck stats computed yet.")));
    }

    frame.render_widget(
        Paragraph::new(Text::from(lines))
            .block(Block::default().borders(Borders::ALL)),
        rows[1],
    );

    let hint = Paragraph::new("  [d/q] close dashboard")
        .style(Style::default().fg(Color::DarkGray));
    frame.render_widget(hint, rows[2]);
}

fn render_map_ev(frame: &mut Frame, app: &App, area: Rect) {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2),  // title
            Constraint::Min(10),    // body
            Constraint::Length(1),  // hint
        ])
        .split(area);

    let title_text = format!(
        " MAP PLANNER — {}",
        app.map_ev.as_ref().map(|d| d.sub_act.as_str()).unwrap_or("—")
    );
    let title = Paragraph::new(title_text)
        .block(Block::default().borders(Borders::BOTTOM))
        .style(Style::default().fg(Color::White).add_modifier(Modifier::BOLD));
    frame.render_widget(title, rows[0]);

    let mut lines: Vec<Line<'static>> = Vec::new();

    if let Some(ref data) = app.map_ev {
        // ── Node EV table ──────────────────────────────────────────────────────
        let header = format!(
            "  {:<10}  {:>8}  {:>8}  {:<22}  {}",
            "NODE", "HP RISK", "SURV%", "REWARD", "SCORE"
        );
        lines.push(Line::from(Span::styled(header, Style::default().fg(Color::DarkGray))));
        lines.push(Line::from(Span::styled(
            format!("  {}", "─".repeat(62)),
            Style::default().fg(Color::DarkGray),
        )));

        for node in [
            &data.normal,
            &data.elite,
            &data.boss,
            &data.treasure,
            &data.rest,
            &data.shop,
        ] {
            let hp_col = if node.encounter_count > 0 {
                format!("-{:.0}", node.mean_hp_loss)
            } else {
                "—".to_string()
            };
            let surv_col = if node.encounter_count > 0 {
                format!("{:.0}%", node.survival_rate * 100.0)
            } else {
                "—".to_string()
            };
            let stars: String = "★".repeat(node.stars as usize)
                + &"☆".repeat(3usize.saturating_sub(node.stars as usize));
            let row = format!(
                "  {:<10}  {:>8}  {:>8}  {:<22}  {}",
                node.label, hp_col, surv_col, node.reward, stars,
            );
            let color = node_color(node);
            lines.push(Line::from(Span::styled(row, Style::default().fg(color))));
        }

        // Event node row: encounter_count holds total event count
        let ev = &data.event_node;
        let ev_reward = format!("{} ({} events)", ev.reward, ev.encounter_count);
        let ev_stars: String = "★".repeat(ev.stars as usize)
            + &"☆".repeat(3usize.saturating_sub(ev.stars as usize));
        let ev_row = format!(
            "  {:<10}  {:>8}  {:>8}  {:<22}  {}",
            ev.label, "—", "—", ev_reward, ev_stars,
        );
        lines.push(Line::from(Span::styled(ev_row, Style::default().fg(Color::Cyan))));

        lines.push(Line::from(Span::raw("")));

        // ── Events list ────────────────────────────────────────────────────────
        let event_header = format!(
            "  Events — {} + {} shared:",
            data.sub_act,
            data.shared_event_count,
        );
        lines.push(Line::from(Span::styled(
            event_header,
            Style::default().fg(Color::DarkGray),
        )));

        for (i, ev) in data.events.iter().enumerate() {
            let selected = i == app.selected_row;
            let indicator = if selected { "▶" } else { " " };
            let act_tag = if ev.is_shared {
                "[Shared]".to_string()
            } else {
                // Abbreviate long act strings
                ev.act
                    .replace("Act 1 - Overgrowth", "Overgrowth")
                    .replace("Act 1 - Underdocks", "Underdocks")
                    .replace("Act 2 - Hive", "Hive")
                    .replace("Act 3 - Glory", "Glory")
            };
            let opts = ev.option_titles.join(" / ");
            let opts_trunc = if opts.len() > 34 {
                format!("{}…", &opts[..33])
            } else {
                opts
            };
            let row_text = format!(
                "  {} {:<24} {:<14}  {}",
                indicator,
                if ev.name.len() > 23 { format!("{}…", &ev.name[..22]) } else { ev.name.clone() },
                act_tag,
                opts_trunc,
            );
            let style = if selected {
                Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::White)
            };
            lines.push(Line::from(Span::styled(row_text, style)));

            if selected && !ev.description.is_empty() {
                let desc = &ev.description;
                let truncated = if desc.len() > 100 {
                    format!("{}…", &desc[..99])
                } else {
                    desc.clone()
                };
                lines.push(Line::from(Span::styled(
                    format!("      {}", truncated),
                    Style::default().fg(Color::DarkGray),
                )));
            }
        }

        if data.events.is_empty() {
            lines.push(Line::from(Span::styled(
                "  (no events for this sub-act — change filter with [o/u/h/g])",
                Style::default().fg(Color::DarkGray),
            )));
        }

        // ── Run Projection ─────────────────────────────────────────────────────
        if let Some(ref ev) = app.run_ev {
            lines.push(Line::from(Span::raw("")));
            let sim_tag = if ev.simulated { "[sim]" } else { "[est]" };
            lines.push(Line::from(Span::styled(
                format!("  ── Run Projection {sim_tag} ──────────────────────────────"),
                Style::default().fg(Color::DarkGray),
            )));

            let delta = ev.current_act_remaining_delta;
            let after_boss = ev.hp_after_current_boss.max(0.0);
            let sign = if delta >= 0.0 { "+" } else { "" };
            let boss_color = if after_boss <= 0.0 { Color::Red } else if after_boss < 30.0 { Color::Yellow } else { Color::White };
            let cur_act_label = format!("Act {} ({})", ev.current_act_number, capitalize(&ev.current_sub_act));
            lines.push(Line::from(Span::styled(
                format!(
                    "  {cur_act_label:<16}  {sign}{delta:.0} HP   → {after_boss:.0} HP after boss",
                ),
                Style::default().fg(boss_color),
            )));

            if ev.hp_after_current_boss > 0.0 {
                lines.push(Line::from(Span::styled(
                    format!("  Post-act heal:  → {:.0} HP", ev.hp_after_current_heal),
                    Style::default().fg(Color::LightGreen),
                )));
            }

            for act_ev in &ev.future_acts {
                let name = format!("Act {} ({})", act_ev.act_number, capitalize(&act_ev.sub_act));
                let d = act_ev.expected_delta;
                let sign = if d >= 0.0 { "+" } else { "" };
                let exit = act_ev.exit_hp.max(0.0);
                let color = if exit <= 0.0 { Color::Red } else if exit < 30.0 { Color::Yellow } else { Color::White };
                lines.push(Line::from(Span::styled(
                    format!("  {name:<16}  {sign}{d:.0} HP   → {exit:.0} HP after boss"),
                    Style::default().fg(color),
                )));
                if act_ev.exit_hp > 0.0 && act_ev.post_heal_hp != act_ev.exit_hp {
                    lines.push(Line::from(Span::styled(
                        format!("  Post-act heal:  → {:.0} HP", act_ev.post_heal_hp),
                        Style::default().fg(Color::LightGreen),
                    )));
                }
                if act_ev.exit_hp <= 0.0 {
                    break;
                }
            }

            let final_hp = ev.projected_final_hp.max(0.0);
            let (final_color, final_tag) = if final_hp <= 0.0 {
                (Color::Red, " ⚠ LETHAL")
            } else if final_hp < 20.0 {
                (Color::Yellow, " ⚠ CRITICAL")
            } else {
                (Color::LightGreen, "")
            };
            lines.push(Line::from(Span::styled(
                format!("  Projected finish: {final_hp:.0} HP{final_tag}"),
                Style::default().fg(final_color).add_modifier(Modifier::BOLD),
            )));
        }
    } else {
        lines.push(Line::from(Span::raw("  No map data computed yet.")));
    }

    frame.render_widget(
        Paragraph::new(Text::from(lines))
            .block(Block::default().borders(Borders::ALL)),
        rows[1],
    );

    let hint = Paragraph::new("  [j/k] scroll events  [v/q] close")
        .style(Style::default().fg(Color::DarkGray));
    frame.render_widget(hint, rows[2]);
}

fn node_color(node: &crate::metrics::map_ev::NodeEv) -> Color {
    match node.stars {
        3 => Color::Green,
        2 => Color::Yellow,
        _ => Color::Red,
    }
}

fn render_character_pick(frame: &mut Frame, app: &App, area: Rect) {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2),  // title
            Constraint::Min(6),     // character list + detail
            Constraint::Length(1),  // hint
        ])
        .split(area);

    let title = Paragraph::new(" CHARACTER SELECT")
        .block(Block::default().borders(Borders::BOTTOM))
        .style(Style::default().fg(Color::White).add_modifier(Modifier::BOLD));
    frame.render_widget(title, rows[0]);

    let mut lines: Vec<Line<'static>> = Vec::new();

    // ── Character list ─────────────────────────────────────────────────────
    for (i, ch) in app.characters.iter().enumerate() {
        let selected = i == app.selected_row;
        let indicator = if selected { "▶" } else { " " };
        let hp = ch.starting_hp.unwrap_or(0);
        let energy = ch.max_energy.unwrap_or(3);
        let deck_count = ch.starting_deck.len();
        let orb_str = ch.orb_slots
            .filter(|&n| n > 0)
            .map(|n| format!("  orbs {}", n))
            .unwrap_or_default();

        let relic_id = ch.starting_relics.first().map(|s| s.as_str()).unwrap_or("—");
        let relic_display = camel_relic_name(relic_id);

        let row = format!(
            "  {}  {:<20}  HP {:>2}  Energy {}  {:>2} cards{}  [{}]",
            indicator,
            ch.name,
            hp,
            energy,
            deck_count,
            orb_str,
            relic_display,
        );
        let style = if selected {
            Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::White)
        };
        lines.push(Line::from(Span::styled(row, style)));
    }

    // ── Detail panel for selected character ───────────────────────────────
    if let Some(ch) = app.characters.get(app.selected_row) {
        lines.push(Line::from(Span::styled(
            format!("  {}", "─".repeat(72)),
            Style::default().fg(Color::DarkGray),
        )));

        // Description (first sentence / first 120 chars)
        if let Some(ref desc) = ch.description {
            let short: String = desc.lines().next().unwrap_or("").chars().take(110).collect();
            lines.push(Line::from(Span::styled(
                format!("  {}", short),
                Style::default().fg(Color::DarkGray),
            )));
        }
        lines.push(Line::from(Span::raw("")));

        // Starting deck summary
        let deck_summary = summarize_deck(&ch.starting_deck);
        lines.push(Line::from(Span::styled(
            format!("  Deck:  {}", deck_summary),
            Style::default().fg(Color::Cyan),
        )));

        // Starting relic
        let relic_id = ch.starting_relics.first().map(|s| s.as_str()).unwrap_or("—");
        let (relic_name, relic_desc) = relic_display_info(relic_id);
        lines.push(Line::from(Span::styled(
            format!("  Relic: {} — {}", relic_name, relic_desc),
            Style::default().fg(Color::Green),
        )));
    }

    if app.characters.is_empty() {
        lines.push(Line::from(Span::styled(
            "  No character data loaded — check network connection or cache.",
            Style::default().fg(Color::Red),
        )));
    }

    frame.render_widget(
        Paragraph::new(Text::from(lines))
            .block(Block::default().borders(Borders::ALL)),
        rows[1],
    );

    let hint = Paragraph::new("  [j/k] navigate  [Enter] select  [q] quit")
        .style(Style::default().fg(Color::DarkGray));
    frame.render_widget(hint, rows[2]);
}

/// Summarize a starting deck as "Strike ×5  Defend ×4  Bash ×1".
fn summarize_deck(ids: &[String]) -> String {
    // Strip character suffix ("StrikeIronclad" → "Strike")
    let suffixes = ["Ironclad", "Silent", "Regent", "Necrobinder", "Defect"];
    let mut counts: Vec<(String, usize)> = Vec::new();
    for id in ids {
        let short = suffixes
            .iter()
            .find_map(|&s| id.strip_suffix(s))
            .unwrap_or(id.as_str())
            .to_string();
        if let Some(entry) = counts.iter_mut().find(|(n, _)| *n == short) {
            entry.1 += 1;
        } else {
            counts.push((short, 1));
        }
    }
    counts
        .iter()
        .map(|(name, n)| {
            if *n > 1 {
                format!("{} ×{}", name, n)
            } else {
                name.clone()
            }
        })
        .collect::<Vec<_>>()
        .join("  ")
}

/// Convert a camelCase relic ID to a short display name by inserting spaces.
fn camel_relic_name(id: &str) -> String {
    let mut out = String::with_capacity(id.len() + 4);
    for (i, c) in id.char_indices() {
        if i > 0 && c.is_uppercase() {
            out.push(' ');
        }
        out.push(c);
    }
    out
}

/// Return (name, description) for a starting relic ID.
fn relic_display_info(id: &str) -> (&'static str, &'static str) {
    match id {
        "BurningBlood"    => ("Burning Blood",      "At the end of combat, heal 6 HP."),
        "RingOfTheSnake"  => ("Ring of the Snake",  "At the start of each combat, draw 2 additional cards."),
        "DivineRight"     => ("Divine Right",       "At the start of each combat, gain 3 Stars."),
        "BoundPhylactery" => ("Bound Phylactery",   "At the start of each combat, Summon 1."),
        "CrackedCore"     => ("Cracked Core",       "At the start of each combat, Channel 1 Lightning."),
        _                 => ("Unknown Relic",      ""),
    }
}

fn render_encounter_pick(frame: &mut Frame, app: &App, area: Rect) {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2), // title
            Constraint::Length(2), // act filter bar
            Constraint::Min(6),    // encounter list
            Constraint::Length(2), // hint
        ])
        .split(area);

    let title = Paragraph::new(" ENCOUNTER PICKER — Choose your fight")
        .block(Block::default().borders(Borders::BOTTOM))
        .style(Style::default().fg(Color::White).add_modifier(Modifier::BOLD));
    frame.render_widget(title, rows[0]);

    // Sub-act filter chips
    let acts = [
        ("overgrowth", "Overgrowth [o]"),
        ("underdocks", "Underdocks [u]"),
        ("hive", "Hive [h]"),
        ("glory", "Glory [g]"),
        ("boss", "Boss [b]"),
        ("all", "All [a]"),
    ];
    let filter_spans: Vec<Span<'static>> = acts
        .iter()
        .flat_map(|(key, label)| {
            let selected = app.act_filter == *key;
            let style = if selected {
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Cyan)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::DarkGray)
            };
            vec![
                Span::styled(format!(" {} ", label), style),
                Span::raw("  "),
            ]
        })
        .collect();
    frame.render_widget(Paragraph::new(Line::from(filter_spans)), rows[1]);

    // Encounter list
    let mut lines: Vec<Line<'static>> = Vec::new();
    if app.encounters.is_empty() {
        lines.push(Line::from(Span::styled(
            "  No encounter data — API unavailable. Press Enter to use default, [m] for manual input.",
            Style::default().fg(Color::Yellow),
        )));
    } else if app.filtered_indices.is_empty() {
        lines.push(Line::from(Span::raw("  No encounters for this act.")));
    } else {
        for (row, &idx) in app.filtered_indices.iter().enumerate() {
            let Some(enc) = app.encounters.get(idx) else { continue };
            let selected = row == app.selected_row;
            let (prefix, style) = if selected {
                ("> ", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD))
            } else {
                ("  ", Style::default().fg(Color::White))
            };

            let room = enc.room_type.as_deref().unwrap_or("?");
            let room_color = match room.to_lowercase().as_str() {
                r if r.contains("elite") => Color::Yellow,
                r if r.contains("boss") => Color::Red,
                _ => Color::DarkGray,
            };

            let monster_names: Vec<String> = enc.monsters.iter().map(|m| m.name.clone()).collect();
            let monsters_str = if monster_names.is_empty() {
                "?".to_string()
            } else {
                monster_names.join(", ")
            };

            lines.push(Line::from(vec![
                Span::styled(format!("{}{}", prefix, enc.name), style),
                Span::raw("  "),
                Span::styled(format!("[{}]", room), Style::default().fg(room_color)),
                Span::raw(format!("  {}", monsters_str)),
            ]));
        }
    }

    frame.render_widget(
        Paragraph::new(Text::from(lines))
            .block(Block::default().borders(Borders::ALL))
            .wrap(Wrap { trim: false }),
        rows[2],
    );

    let hint = Paragraph::new(
        "  [o/u/h/g/b/a] filter  [j/k] navigate  [Enter] select  [v] map  [m] manual  [q] quit",
    )
    .style(Style::default().fg(Color::DarkGray));
    frame.render_widget(hint, rows[3]);
}

fn render_map_view(frame: &mut Frame, app: &App, area: Rect) {
    use crate::domain::map::{ActMap, MapPos, RoomType, COLS, ROWS};

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2), // header + run stats
            Constraint::Min(0),    // map grid
            Constraint::Length(2), // choices bar
            Constraint::Length(1), // status
        ])
        .split(area);

    // ── Header ─────────────────────────────────────────────────────────────────
    let sub_act = app.run.as_ref().map(|r| r.sub_act.as_str()).unwrap_or("—");
    let floor_label = app.run.as_ref()
        .and_then(|r| r.map_pos)
        .map(|p| format!("Floor {}/15", p.floor + 1))
        .unwrap_or_else(|| "Floor —/15".to_string());
    let header_line1 = format!(
        " MAP \u{2014} {}  {}   [j/k] choose  [Enter] enter  [t/Esc] back",
        capitalize(sub_act), floor_label,
    );
    let run_stats_line = app.run.as_ref().map(|r| {
        let relic_names: Vec<&str> = r.relics.iter().map(|rel| rel.name.as_str()).collect();
        let relics_str = if relic_names.is_empty() { "none".to_string() } else { relic_names.join(", ") };
        format!(" HP {}/{}  Gold {}  | {}", r.hp, r.max_hp, r.gold, relics_str)
    }).unwrap_or_default();
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(Span::styled(header_line1, Style::default().fg(Color::White).add_modifier(Modifier::BOLD))),
            Line::from(Span::styled(run_stats_line, Style::default().fg(Color::DarkGray))),
        ]),
        chunks[0],
    );

    // ── Map grid ───────────────────────────────────────────────────────────────
    let (run, map) = match app.run.as_ref().and_then(|r| r.map.as_ref().map(|m| (r, m))) {
        Some(pair) => pair,
        None => {
            frame.render_widget(
                Paragraph::new("  No map — press [t] to generate.")
                    .block(Block::default().borders(Borders::ALL)),
                chunks[1],
            );
            render_statusbar(frame, app, chunks[3]);
            return;
        }
    };

    let cur_pos    = run.map_pos;
    let choices    = app.map_choices();
    let choices_floor: Option<usize> = match cur_pos {
        None      => Some(0),
        Some(pos) => if pos.floor + 1 < ROWS { Some(pos.floor + 1) } else { None },
    };
    let selected_col = choices.get(app.map_cursor).copied();

    const LABEL_W: usize = 4;
    const COL_W:   usize = 5; // " [X] "

    let mut lines: Vec<Line<'static>> = Vec::with_capacity(ROWS * 2);

    for floor in (0..ROWS).rev() {
        let game_floor  = floor + 1;
        let is_cur      = cur_pos.map(|p| p.floor) == Some(floor);
        let cur_floor_n = cur_pos.map(|p| p.floor);

        // ── Node row ──────────────────────────────────────────────────────────
        let mut spans: Vec<Span<'static>> = Vec::new();

        let label = if is_cur {
            format!("{:2}▶ ", game_floor)
        } else {
            format!("{:2}  ", game_floor)
        };
        spans.push(Span::styled(
            label,
            if is_cur {
                Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::DarkGray)
            },
        ));

        for col in 0..COLS {
            let rt        = map.room_type(floor, col);
            let is_me     = cur_pos == Some(MapPos { floor, col });
            let is_choice = choices_floor == Some(floor) && choices.contains(&(col as u8));
            let is_sel    = choices_floor == Some(floor) && selected_col == Some(col as u8);
            let is_past   = cur_floor_n.map(|cf| floor < cf).unwrap_or(false);

            let symbol = rt.map(|r| format!("[{}]", r.label())).unwrap_or_else(|| "   ".into());
            let cell   = format!(" {} ", symbol);

            let style = if is_me {
                Style::default().fg(Color::Black).bg(Color::Cyan).add_modifier(Modifier::BOLD)
            } else if is_sel {
                Style::default().fg(Color::Black).bg(Color::Green).add_modifier(Modifier::BOLD)
            } else if is_choice {
                Style::default().fg(Color::LightGreen).add_modifier(Modifier::BOLD)
            } else if rt.is_none() {
                Style::default()
            } else if is_past {
                Style::default().fg(Color::DarkGray)
            } else {
                map_rt_style(rt)
            };

            spans.push(Span::styled(cell, style));
        }

        lines.push(Line::from(spans));

        // ── Connection row (edges from floor-1 → floor) ───────────────────────
        if floor > 0 {
            lines.push(map_connection_row(floor - 1, map, LABEL_W, COL_W));
        }
    }

    frame.render_widget(
        Paragraph::new(lines).block(Block::default().borders(Borders::ALL).title(" MAP ")),
        chunks[1],
    );

    // ── Choices bar ────────────────────────────────────────────────────────────
    let mut choice_spans: Vec<Span<'static>> = vec![Span::raw("  Choices: ")];
    if choices.is_empty() {
        choice_spans.push(Span::styled(
            "none (you are at the top)",
            Style::default().fg(Color::DarkGray),
        ));
    } else {
        let cf = choices_floor.unwrap_or(0);
        for (i, &col) in choices.iter().enumerate() {
            let rt_label = map.room_type(cf, col as usize)
                .map(|r| r.label())
                .unwrap_or("?");
            // Annotate with path HP cost if available
            let path_info = app.path_choices.iter().find(|pc| pc.col == col);
            let path_str = path_info.map(|pc| {
                let sign = if pc.total_hp_delta >= 0.0 { "+" } else { "" };
                format!("{sign}{:.0}hp +{:.0}g", pc.total_hp_delta, pc.expected_gold)
            }).unwrap_or_default();
            let best_marker = path_info.map(|pc| pc.is_best).unwrap_or(false);
            let label = if path_str.is_empty() {
                format!(" col{}[{}] ", col, rt_label)
            } else if best_marker {
                format!(" col{}[{}] {} ▲ ", col, rt_label, path_str)
            } else {
                format!(" col{}[{}] {} ", col, rt_label, path_str)
            };
            let is_cursor = i == app.map_cursor;
            let style = if is_cursor {
                Style::default().fg(Color::Black).bg(Color::Green).add_modifier(Modifier::BOLD)
            } else if best_marker {
                Style::default().fg(Color::LightGreen).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::LightGreen)
            };
            choice_spans.push(Span::styled(label, style));
        }
        // Show whether costs are from simulation or defaults
        if let Some(pc) = app.path_choices.first() {
            let tag = if pc.simulated { " [sim]" } else { " [est]" };
            choice_spans.push(Span::styled(tag, Style::default().fg(Color::DarkGray)));
        }
    }
    frame.render_widget(
        Paragraph::new(Line::from(choice_spans)).block(Block::default().borders(Borders::TOP)),
        chunks[2],
    );

    render_statusbar(frame, app, chunks[3]);
}

/// Build the connection row between array floor `floor_idx` and `floor_idx + 1`.
/// Uses `|` for straight edges, `/` for up-left, `\` for up-right.
fn map_connection_row(floor_idx: usize, map: &crate::domain::map::ActMap,
                      label_w: usize, col_w: usize) -> Line<'static> {
    let total = label_w + crate::domain::map::COLS * col_w;
    let mut buf: Vec<char> = vec![' '; total];

    for col in 0..crate::domain::map::COLS {
        for &dst in map.next_nodes(floor_idx, col) {
            let dst = dst as usize;
            let src_cx = label_w + col * col_w + col_w / 2;
            let dst_cx = label_w + dst * col_w + col_w / 2;
            let mid    = (src_cx + dst_cx) / 2;
            let ch = if col == dst { '|' } else if dst < col { '\\' } else { '/' };
            if mid < buf.len() { buf[mid] = ch; }
        }
    }

    Line::from(Span::styled(
        buf.into_iter().collect::<String>(),
        Style::default().fg(Color::DarkGray),
    ))
}

fn map_rt_style(rt: Option<crate::domain::map::RoomType>) -> Style {
    use crate::domain::map::RoomType;
    match rt {
        Some(RoomType::Monster)  => Style::default().fg(Color::White),
        Some(RoomType::Elite)    => Style::default().fg(Color::LightRed),
        Some(RoomType::Event)    => Style::default().fg(Color::Yellow),
        Some(RoomType::Shop)     => Style::default().fg(Color::Cyan),
        Some(RoomType::Rest)     => Style::default().fg(Color::LightGreen),
        Some(RoomType::Treasure) => Style::default().fg(Color::LightYellow),
        Some(RoomType::Boss)     => Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        None                     => Style::default(),
    }
}

fn render_event_room(frame: &mut Frame, app: &App, area: Rect) {
    use crate::metrics::map_ev::strip_tags;

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2),  // header
            Constraint::Length(8),  // event description
            Constraint::Min(4),     // options
            Constraint::Length(1),  // status
        ])
        .split(area);

    let floor = app.run.as_ref().map(|r| r.floor).unwrap_or(0);
    let event_name = app.active_event.as_ref().map(|e| e.name.as_str()).unwrap_or("Unknown Event");

    let header = Paragraph::new(format!(
        " EVENT   Floor {floor}   {event_name}   j/k select  Enter confirm  Esc back"
    ))
    .style(Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD));
    frame.render_widget(header, rows[0]);

    // Description — first paragraph, tags stripped
    let raw_desc = app.active_event.as_ref()
        .and_then(|e| e.description.as_deref())
        .unwrap_or("No description available.");
    let stripped = strip_tags(raw_desc);
    let first_para: String = stripped.lines().take(4).collect::<Vec<_>>().join(" ");
    let desc = Paragraph::new(first_para)
        .block(Block::default().borders(Borders::ALL).title(format!(" {event_name} ")))
        .wrap(Wrap { trim: true })
        .style(Style::default().fg(Color::White));
    frame.render_widget(desc, rows[1]);

    // Options
    let options = app.active_event.as_ref().map(|e| e.options.as_slice()).unwrap_or(&[]);
    let recommended_idx = app.event_advice.first().map(|a| a.option_idx);

    let option_lines: Vec<Line> = options.iter().enumerate().map(|(i, opt)| {
        let is_cursor = i == app.event_cursor;
        let is_rec    = recommended_idx == Some(i);
        let title     = opt.title.as_deref().unwrap_or("?");
        let opt_desc  = strip_tags(opt.description.as_deref().unwrap_or(""));
        let reason    = app.event_advice.iter()
            .find(|a| a.option_idx == i)
            .map(|a| a.reason.as_str())
            .unwrap_or("");
        let rec_marker = if is_rec { " ★" } else { "  " };
        let prefix     = if is_cursor { "▶ " } else { "  " };
        let style = if is_cursor {
            Style::default().fg(Color::Black).bg(Color::Yellow)
        } else if is_rec {
            Style::default().fg(Color::LightYellow).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::White)
        };
        let line_text = format!("{prefix}{title}{rec_marker}  {opt_desc:.60}  [{reason}]");
        Line::from(vec![Span::styled(line_text, style)])
    }).collect();

    let options_widget = Paragraph::new(Text::from(option_lines))
        .block(Block::default().borders(Borders::ALL).title(" Options "))
        .wrap(Wrap { trim: false });
    frame.render_widget(options_widget, rows[2]);

    // Status
    let (hp, max_hp) = app.run.as_ref().map(|r| (r.hp, r.max_hp)).unwrap_or((80, 80));
    let status = Paragraph::new(format!("  HP: {hp}/{max_hp}   {}", app.status_message))
        .style(Style::default().fg(Color::DarkGray));
    frame.render_widget(status, rows[3]);
}

fn render_rest_site(frame: &mut Frame, app: &App, area: Rect) {
    use crate::metrics::rest::RestAction;

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2),  // header
            Constraint::Min(6),     // options
            Constraint::Length(1),  // status
        ])
        .split(area);

    // Header
    let floor = app.run.as_ref().map(|r| r.floor).unwrap_or(0);
    let header = Paragraph::new(format!(" REST SITE   Floor {floor}   j/k select  Enter confirm  Esc back"))
        .style(Style::default().fg(Color::Green).add_modifier(Modifier::BOLD));
    frame.render_widget(header, rows[0]);

    // Build option lines
    let (hp, max_hp) = app.run.as_ref()
        .map(|r| (r.hp, r.max_hp))
        .unwrap_or((80, 80));
    let heal_amount = ((max_hp as f32) * 0.30).floor() as u32;
    let healed_hp = (hp + heal_amount).min(max_hp);

    let smith_label = app.rest_advice.as_ref().and_then(|a| {
        if let RestAction::Smith(idx) = a.action {
            app.run.as_ref().and_then(|r| r.deck.get(idx)).map(|c| c.name.clone())
        } else {
            None
        }
    }).unwrap_or_else(|| "—".to_string());

    let recommended = app.rest_advice.as_ref().map(|a| {
        matches!(a.action, RestAction::Heal)
    }).unwrap_or(true);
    let reason = app.rest_advice.as_ref().map(|a| a.reason.as_str()).unwrap_or("");

    let options = [
        format!("Heal    restore {} HP ({} → {})", heal_amount, hp, healed_hp),
        format!("Smith   upgrade {}", smith_label),
    ];
    let rec_idx: usize = if recommended { 0 } else { 1 };

    let mut lines: Vec<Line> = options.iter().enumerate().map(|(i, label)| {
        let is_cursor   = i == app.rest_cursor;
        let is_rec      = i == rec_idx;
        let rec_marker  = if is_rec { " ★" } else { "  " };
        let prefix      = if is_cursor { "▶ " } else { "  " };
        let style = if is_cursor {
            Style::default().fg(Color::Black).bg(Color::Green)
        } else if is_rec {
            Style::default().fg(Color::LightGreen).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::White)
        };
        Line::from(vec![
            Span::styled(format!("{prefix}{label}{rec_marker}"), style),
        ])
    }).collect();

    lines.push(Line::from(""));
    lines.push(Line::from(vec![
        Span::styled(format!("  Advice: {reason}"), Style::default().fg(Color::DarkGray)),
    ]));

    let options_widget = Paragraph::new(Text::from(lines))
        .block(Block::default().borders(Borders::ALL).title(" Rest Site "));
    frame.render_widget(options_widget, rows[1]);

    // Status
    let status = Paragraph::new(format!("  HP: {hp}/{max_hp}   {}", app.status_message))
        .style(Style::default().fg(Color::DarkGray));
    frame.render_widget(status, rows[2]);
}

fn render_treasure_room(frame: &mut Frame, app: &App, area: Rect) {
    use crate::metrics::map_ev::strip_tags;

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2),  // header
            Constraint::Min(6),     // relic display
            Constraint::Length(1),  // hint
            Constraint::Length(1),  // status
        ])
        .split(area);

    let floor = app.run.as_ref().map(|r| r.floor).unwrap_or(0);
    let header = Paragraph::new(format!(" TREASURE CHEST   Floor {floor}   ↑↓ select  Enter confirm  Esc back"))
        .style(Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD));
    frame.render_widget(header, rows[0]);

    let (relic_name, relic_desc) = if let Some(ref relic) = app.offered_relic {
        let name = relic.name.clone();
        let desc = strip_tags(relic.description.as_deref().unwrap_or("No description."));
        (name, desc)
    } else {
        ("No relic found".to_string(), String::new())
    };

    let options = ["[T]ake", "[S]kip"];
    let mut lines: Vec<Line> = vec![
        Line::from(vec![Span::styled(
            format!("  {relic_name}"),
            Style::default().fg(Color::LightYellow).add_modifier(Modifier::BOLD),
        )]),
        Line::from(vec![Span::styled(
            format!("  {relic_desc}"),
            Style::default().fg(Color::White),
        )]),
        Line::from(""),
    ];

    for (i, label) in options.iter().enumerate() {
        let is_cursor = i == app.relic_cursor;
        let prefix = if is_cursor { "▶ " } else { "  " };
        let style = if is_cursor {
            Style::default().fg(Color::Black).bg(Color::Yellow)
        } else {
            Style::default().fg(Color::White)
        };
        lines.push(Line::from(vec![Span::styled(format!("{prefix}{label}"), style)]));
    }

    let relic_widget = Paragraph::new(Text::from(lines))
        .block(Block::default().borders(Borders::ALL).title(" Relic "))
        .wrap(Wrap { trim: true });
    frame.render_widget(relic_widget, rows[1]);

    let hint = Paragraph::new("  ↑↓ select  Enter confirm  Esc back")
        .style(Style::default().fg(Color::DarkGray));
    frame.render_widget(hint, rows[2]);

    let (hp, max_hp) = app.run.as_ref().map(|r| (r.hp, r.max_hp)).unwrap_or((80, 80));
    let status = Paragraph::new(format!("  HP: {hp}/{max_hp}   {}", app.status_message))
        .style(Style::default().fg(Color::DarkGray));
    frame.render_widget(status, rows[3]);
}

fn render_shop(frame: &mut Frame, app: &App, area: Rect) {
    use crate::metrics::map_ev::strip_tags;

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2),  // header
            Constraint::Min(8),     // cards
            Constraint::Length(6),  // relics
            Constraint::Length(2),  // removal service
            Constraint::Length(1),  // hint
            Constraint::Length(1),  // status
        ])
        .split(area);

    let floor = app.run.as_ref().map(|r| r.floor).unwrap_or(0);
    let gold = app.run.as_ref().map(|r| r.gold).unwrap_or(0);
    let header = Paragraph::new(format!(" SHOP   Floor {floor}   Gold: {gold}g   ↑↓ navigate  Enter buy  Esc leave"))
        .style(Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD));
    frame.render_widget(header, rows[0]);

    // Cards section
    let n_cards = app.shop_cards.len();
    let n_relics = app.shop_relics.len();
    let gold = app.run.as_ref().map(|r| r.gold).unwrap_or(0);

    let card_lines: Vec<Line> = app.shop_cards.iter().enumerate().map(|(i, card)| {
        let is_cursor = i == app.shop_cursor;
        let is_best  = app.shop_best_buy_cursor == Some(i);
        let advice   = app.shop_card_advice.iter().find(|a| a.card_index == i);
        let price    = app.shop_card_prices.get(i).copied().unwrap_or(0);
        let can_afford = gold >= price;
        let is_discounted = app.shop_discounted_card_idx == Some(i);

        let best_marker    = if is_best { "★ " } else { "  " };
        let discount_marker = if is_discounted { " [SALE]" } else { "" };
        let reason         = advice.map(|a| a.reason.as_str()).unwrap_or("");
        let label = format!("{best_marker}{}{discount_marker}  {price}g   {reason}", card.name);

        let style = if is_cursor {
            Style::default().fg(Color::Black).bg(Color::Cyan)
        } else if !can_afford {
            Style::default().fg(Color::DarkGray)
        } else if is_best {
            Style::default().fg(Color::LightGreen)
        } else if is_discounted {
            Style::default().fg(Color::LightGreen)
        } else {
            Style::default().fg(Color::White)
        };
        Line::from(vec![Span::styled(label, style)])
    }).collect();

    let cards_widget = Paragraph::new(Text::from(card_lines))
        .block(Block::default().borders(Borders::ALL).title(" Cards (5 class · 2 colorless) "))
        .wrap(Wrap { trim: false });
    frame.render_widget(cards_widget, rows[1]);

    // Relics section
    let relic_lines: Vec<Line> = app.shop_relics.iter().enumerate().map(|(i, relic)| {
        let cursor_idx = n_cards + i;
        let is_cursor  = cursor_idx == app.shop_cursor;
        let is_best    = app.shop_best_buy_cursor == Some(cursor_idx);
        let price      = app.shop_relic_prices.get(i).copied().unwrap_or(0);
        let can_afford = gold >= price;

        let ra = app.shop_relic_advice.get(i);
        let hp_str   = ra.map(|a| format!("{:.0}HP", a.hp_value)).unwrap_or_default();
        let reason   = ra.map(|a| a.reason.as_str()).unwrap_or("");
        let sim_mark = if ra.map(|a| a.simulated).unwrap_or(false) { "" } else { "" };

        let best_marker = if is_best { "★ " } else { "  " };
        let label = format!(
            "{best_marker}{}  {price}g  {hp_str}{sim_mark} — {reason}",
            relic.name
        );

        let style = if is_cursor {
            Style::default().fg(Color::Black).bg(Color::Cyan)
        } else if !can_afford {
            Style::default().fg(Color::DarkGray)
        } else if is_best {
            Style::default().fg(Color::LightGreen)
        } else {
            Style::default().fg(Color::LightYellow)
        };
        Line::from(vec![Span::styled(label, style)])
    }).collect();

    let relics_widget = Paragraph::new(Text::from(relic_lines))
        .block(Block::default().borders(Borders::ALL).title(" Relics "))
        .wrap(Wrap { trim: false });
    frame.render_widget(relics_widget, rows[2]);

    // Card Removal Service
    let removal_cursor  = n_cards + n_relics;
    let removal_is_cursor = app.shop_cursor == removal_cursor;
    let is_best_removal = app.shop_best_buy_cursor == Some(removal_cursor);
    let removal_line = if app.shop_has_removal {
        let price      = app.shop_removal_price;
        let can_afford = gold >= price;
        let hp_val     = app.shop_removal_hp_value;
        let hp_str     = if hp_val > 0.0 { format!("  {hp_val:.0}HP saved") } else { String::new() };
        let best_marker = if is_best_removal { "★ " } else { "  " };
        let style = if removal_is_cursor {
            Style::default().fg(Color::Black).bg(Color::Cyan)
        } else if !can_afford {
            Style::default().fg(Color::DarkGray)
        } else if is_best_removal {
            Style::default().fg(Color::LightGreen)
        } else {
            Style::default().fg(Color::Gray)
        };
        Line::from(vec![Span::styled(
            format!("{best_marker}Card Removal Service  {price}g{hp_str}"),
            style,
        )])
    } else {
        Line::from(vec![Span::styled("  Card Removal Service  [sold]", Style::default().fg(Color::DarkGray))])
    };
    let removal_widget = Paragraph::new(Text::from(vec![removal_line]))
        .block(Block::default().borders(Borders::ALL).title(" Services "));
    frame.render_widget(removal_widget, rows[3]);

    let hint = Paragraph::new("  ↑↓ navigate  Enter buy  Esc leave")
        .style(Style::default().fg(Color::DarkGray));
    frame.render_widget(hint, rows[4]);

    let (hp, max_hp) = app.run.as_ref().map(|r| (r.hp, r.max_hp)).unwrap_or((80, 80));
    let status = Paragraph::new(format!("  HP: {hp}/{max_hp}   {}", app.status_message))
        .style(Style::default().fg(Color::DarkGray));
    frame.render_widget(status, rows[5]);
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        None    => String::new(),
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
    }
}

fn render_victory(frame: &mut Frame, app: &App, area: Rect) {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(0),
            Constraint::Length(1),
        ])
        .split(area);

    frame.render_widget(
        Paragraph::new("  \u{2605}  VICTORY \u{2014} The Spire Has Been Conquered!  \u{2605}")
            .block(Block::default().borders(Borders::BOTTOM))
            .style(Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
        rows[0],
    );

    let mut lines: Vec<Line<'static>> = Vec::new();
    lines.push(Line::from(Span::raw("")));
    if let Some(run) = &app.run {
        lines.push(Line::from(Span::styled(
            format!("  Class:    {}", run.class),
            Style::default().fg(Color::White),
        )));
        lines.push(Line::from(Span::styled(
            format!("  Final HP: {}/{}", run.hp, run.max_hp),
            Style::default().fg(Color::LightGreen),
        )));
        lines.push(Line::from(Span::styled(
            format!("  Gold:     {}", run.gold),
            Style::default().fg(Color::Yellow),
        )));
        lines.push(Line::from(Span::styled(
            format!("  Deck:     {} cards", run.deck.len()),
            Style::default().fg(Color::White),
        )));
        lines.push(Line::from(Span::raw("")));
        lines.push(Line::from(Span::styled(
            "  Relics:".to_string(),
            Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
        )));
        for relic in &run.relics {
            lines.push(Line::from(Span::styled(
                format!("    \u{2022} {}", relic.name),
                Style::default().fg(Color::Cyan),
            )));
        }
    }
    frame.render_widget(Paragraph::new(lines), rows[1]);

    frame.render_widget(
        Paragraph::new("  [n/Enter] New Run  [q] Quit"),
        rows[2],
    );
}

fn render_ancient_boon(frame: &mut Frame, app: &App, area: Rect) {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // header with ancient name
            Constraint::Min(0),    // boon choices
            Constraint::Length(1), // hint
        ])
        .split(area);

    let header = format!(
        "  {} — {}",
        app.ancient_name,
        if app.ancient_is_act_transition { "Act Transition Boon" } else { "Starting Boon" }
    );
    frame.render_widget(
        Paragraph::new(header)
            .block(Block::default().borders(Borders::BOTTOM))
            .style(Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
        rows[0],
    );

    let mut lines: Vec<Line<'static>> = Vec::new();
    lines.push(Line::from(Span::raw("")));
    for (i, relic) in app.ancient_boons.iter().enumerate() {
        let cursor = if i == app.ancient_cursor { "▶ " } else { "  " };
        let rarity = relic.rarity.as_deref().unwrap_or("Relic");
        let name_style = if i == app.ancient_cursor {
            Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::White)
        };
        lines.push(Line::from(Span::styled(
            format!("{}{}  [{}]", cursor, relic.name, rarity),
            name_style,
        )));
        let desc = relic.description.as_deref().unwrap_or("");
        lines.push(Line::from(Span::styled(
            format!("     {}", desc),
            Style::default().fg(Color::DarkGray),
        )));
        lines.push(Line::from(Span::raw("")));
    }

    frame.render_widget(
        Paragraph::new(lines).block(Block::default()),
        rows[1],
    );

    let hint = Paragraph::new("  [j/k] navigate  [Enter] take boon  [q] quit");
    frame.render_widget(hint, rows[2]);
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{backend::TestBackend, Terminal};
    use crate::input::manual::default_combat_state;
    use crate::tui::app::App;

    fn make_terminal() -> Terminal<TestBackend> {
        Terminal::new(TestBackend::new(120, 30)).unwrap()
    }

    fn make_empty_app() -> App {
        App::new(vec![], vec![], vec![], vec![], vec![], vec![])
    }

    #[test]
    fn render_character_pick_does_not_panic() {
        use crate::data::api::SpireApiCharacter;
        let ch = SpireApiCharacter {
            id: "IRONCLAD".into(),
            name: "The Ironclad".into(),
            description: Some("Test character".into()),
            starting_hp: Some(80),
            starting_gold: Some(99),
            max_energy: Some(3),
            orb_slots: None,
            starting_deck: vec!["StrikeIronclad".into(), "Bash".into()],
            starting_relics: vec!["BurningBlood".into()],
            unlocks_after: None,
            color: Some("red".into()),
            image_url: None,
            quotes: None,
            dialogues: None,
        };
        let mut terminal = make_terminal();
        let app = App::new(vec![ch], vec![], vec![], vec![], vec![], vec![]);
        terminal.draw(|frame| render(frame, &app)).unwrap();
    }

    #[test]
    fn render_encounter_pick_does_not_panic() {
        let mut terminal = make_terminal();
        let app = make_empty_app();
        terminal.draw(|frame| render(frame, &app)).unwrap();
    }

    #[test]
    fn render_manual_input_does_not_panic() {
        use rand::SeedableRng;
        use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
        let mut terminal = make_terminal();
        let mut app = make_empty_app();
        app.load_combat(default_combat_state());
        let key = KeyEvent { code: KeyCode::Char('e'), modifiers: KeyModifiers::NONE, kind: KeyEventKind::Press, state: KeyEventState::NONE };
        let mut rng = rand::rngs::StdRng::seed_from_u64(42);
        app.handle_event(crate::input::event::AppEvent::Key(key), &mut rng);
        terminal.draw(|frame| render(frame, &app)).unwrap();
    }

    #[test]
    fn render_combat_advice_does_not_panic() {
        let mut terminal = make_terminal();
        let mut app = make_empty_app();
        app.load_combat(default_combat_state());
        terminal.draw(|frame| render(frame, &app)).unwrap();
    }

    #[test]
    fn render_combat_after_sim_does_not_panic() {
        use rand::SeedableRng;
        let mut terminal = make_terminal();
        let mut app = make_empty_app();
        app.load_combat(default_combat_state());
        let mut rng = rand::rngs::StdRng::seed_from_u64(42);
        app.run_simulation(&mut rng);
        terminal.draw(|frame| render(frame, &app)).unwrap();
    }

    #[test]
    fn render_deck_dash_does_not_panic() {
        use rand::SeedableRng;
        let mut terminal = make_terminal();
        let mut app = make_empty_app();
        app.load_combat(default_combat_state());
        let mut rng = rand::rngs::StdRng::seed_from_u64(42);
        app.compute_deck_dash(&mut rng);
        terminal.draw(|frame| render(frame, &app)).unwrap();
    }

    #[test]
    fn render_map_ev_does_not_panic() {
        use rand::SeedableRng;
        let mut terminal = make_terminal();
        let mut app = make_empty_app();
        let mut rng = rand::rngs::StdRng::seed_from_u64(42);
        app.open_map_ev(&mut rng);
        terminal.draw(|frame| render(frame, &app)).unwrap();
    }

    #[test]
    fn render_map_view_does_not_panic() {
        use rand::SeedableRng;
        let mut terminal = make_terminal();
        let mut app = make_empty_app();
        let mut rng = rand::rngs::StdRng::seed_from_u64(99);
        // Select a character so a run exists, then open the map view.
        app.select_character(&mut rng);
        terminal.draw(|frame| render(frame, &app)).unwrap();
    }

    #[test]
    fn render_event_room_does_not_panic() {
        use rand::SeedableRng;
        let mut terminal = make_terminal();
        let mut app = make_empty_app();
        let mut rng = rand::rngs::StdRng::seed_from_u64(11);
        app.select_character(&mut rng);
        // Load real events seed so the pool is non-empty.
        app.events = crate::data::api::load_events();
        app.mode = crate::tui::app::AppMode::EventRoom;
        // Populate event directly (bypassing network).
        if let Some(ev) = app.events.first().cloned() {
            let hp_ratio = app.run.as_ref().map(|r| r.hp as f32 / r.max_hp as f32).unwrap_or(1.0);
            app.event_advice = crate::metrics::event::advise_event(&ev.options, hp_ratio);
            app.event_cursor = app.event_advice.first().map(|a| a.option_idx).unwrap_or(0);
            app.active_event = Some(ev);
        }
        terminal.draw(|frame| render(frame, &app)).unwrap();
    }

    #[test]
    fn render_rest_site_does_not_panic() {
        use rand::SeedableRng;
        let mut terminal = make_terminal();
        let mut app = make_empty_app();
        let mut rng = rand::rngs::StdRng::seed_from_u64(7);
        app.select_character(&mut rng);
        app.mode = crate::tui::app::AppMode::RestSite;
        if let Some(ref run) = app.run {
            let advice = crate::metrics::rest::advise_rest(run);
            app.rest_cursor = if matches!(advice.action, crate::metrics::rest::RestAction::Heal) { 0 } else { 1 };
            app.rest_advice = Some(advice);
        }
        terminal.draw(|frame| render(frame, &app)).unwrap();
    }

    #[test]
    fn render_encounter_with_data_does_not_panic() {
        use crate::data::api::{ApiEncounterMonster, SpireApiEncounter};
        let enc = SpireApiEncounter {
            id: "test".into(), name: "Test Fight".into(),
            room_type: Some("normal".into()), is_weak: Some(false),
            act: Some("1".into()), tags: vec![],
            monsters: vec![ApiEncounterMonster { id: "c".into(), name: "Cultist".into() }],
            loss_text: None,
        };
        let mut terminal = make_terminal();
        let app = App::new(vec![], vec![enc], vec![], vec![], vec![], vec![]);
        terminal.draw(|frame| render(frame, &app)).unwrap();
    }

    #[test]
    fn render_treasure_room_does_not_panic() {
        use rand::SeedableRng;
        use crate::data::api::SpireApiRelic;
        let mut terminal = make_terminal();
        let relic = SpireApiRelic {
            id: "test_relic".into(),
            name: "Test Relic".into(),
            description: Some("A [gold]test[/gold] relic.".into()),
            flavor: None,
            rarity: Some("Common Relic".into()),
            rarity_key: None,
            pool: Some("shared".into()),
            merchant_price: None,
            image_url: None,
            notes: None,
        };
        let mut app = App::new(vec![], vec![], vec![], vec![], vec![relic.clone()], vec![]);
        let mut rng = rand::rngs::StdRng::seed_from_u64(7);
        app.select_character(&mut rng);
        app.mode = crate::tui::app::AppMode::TreasureRoom;
        app.offered_relic = Some(relic);
        app.relic_cursor = 0;
        terminal.draw(|frame| render(frame, &app)).unwrap();
    }

    #[test]
    fn render_shop_does_not_panic() {
        use rand::SeedableRng;
        use crate::data::api::SpireApiRelic;
        let mut terminal = make_terminal();
        let relic = SpireApiRelic {
            id: "shop_relic".into(),
            name: "Shop Relic".into(),
            description: Some("A [blue]shop[/blue] relic.".into()),
            flavor: None,
            rarity: Some("Shop Relic".into()),
            rarity_key: None,
            pool: Some("shared".into()),
            merchant_price: None,
            image_url: None,
            notes: None,
        };
        let mut app = App::new(vec![], vec![], vec![], vec![], vec![relic], vec![]);
        let mut rng = rand::rngs::StdRng::seed_from_u64(7);
        app.select_character(&mut rng);
        app.mode = crate::tui::app::AppMode::Shop;
        // Populate shop inventory directly.
        app.shop_cards = crate::domain::catalog::ironclad::starter_deck().into_iter().take(3).collect();
        app.shop_relics = Vec::new();
        app.shop_cursor = 0;
        terminal.draw(|frame| render(frame, &app)).unwrap();
    }

    #[test]
    fn render_victory_does_not_panic() {
        use crate::domain::run::{PlayerClass, RunState, starting_relics};
        use crate::domain::catalog::ironclad;
        let mut terminal = make_terminal();
        let mut app = make_empty_app();
        let deck = ironclad::starter_deck();
        let relic = starting_relics::ironclad();
        let run = RunState::new(PlayerClass::Ironclad, 75, 80, deck, relic);
        app.run = Some(run);
        app.mode = crate::tui::app::AppMode::Victory;
        terminal.draw(|frame| render(frame, &app)).unwrap();
    }

    #[test]
    fn render_ancient_boon_does_not_panic() {
        use crate::data::api::SpireApiRelic;
        let mut terminal = make_terminal();
        let mut app = make_empty_app();
        // Set up a fake boon relic
        let fake_relic = SpireApiRelic {
            id: "FAKE_RELIC".to_string(),
            name: "Fake Relic".to_string(),
            description: Some("A test relic.".to_string()),
            flavor: None,
            rarity: Some("Rare".to_string()),
            rarity_key: None,
            pool: None,
            merchant_price: None,
            image_url: None,
            notes: None,
        };
        app.ancient_id = "NEOW".to_string();
        app.ancient_name = "Neow".to_string();
        app.ancient_boons = vec![fake_relic];
        app.ancient_cursor = 0;
        app.ancient_is_act_transition = false;
        app.mode = crate::tui::app::AppMode::AncientBoon;
        terminal.draw(|frame| render(frame, &app)).unwrap();
    }
}
