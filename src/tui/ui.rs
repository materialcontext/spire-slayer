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
        AppMode::EncounterPick => render_encounter_pick(frame, app, area),
        AppMode::ManualInput => render_input(frame, app, area),
        AppMode::CombatAdvice | AppMode::Simulating => render_combat(frame, app, area),
        AppMode::CardPick => render_card_pick(frame, app, area),
        AppMode::DeckDash => render_deck_dash(frame, app, area),
        AppMode::MapEv => render_map_ev(frame, app, area),
        AppMode::Exiting => {}
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

    let line = Line::from(vec![
        Span::raw("  [s]im  [d]eck  [p]ick  [v]map  [e]dit  [n]ew  [q]uit"),
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
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(2), Constraint::Min(6), Constraint::Length(2)])
        .split(area);

    let title = Paragraph::new(" CARD PICK — Simulator-backed reward evaluation")
        .block(Block::default().borders(Borders::BOTTOM))
        .style(Style::default().fg(Color::White).add_modifier(Modifier::BOLD));
    frame.render_widget(title, rows[0]);

    let mut lines: Vec<Line<'static>> = Vec::new();
    for (rank, advice) in app.card_advice.iter().enumerate() {
        let selected = rank == app.selected_row;
        let prefix = if selected { "> " } else { "  " };

        let name = if advice.card_index == usize::MAX {
            "— skip reward —".to_string()
        } else {
            format!("#{}", advice.card_index + 1)
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

        // Card name / label line
        lines.push(Line::from(Span::styled(
            format!("{}{}", prefix, name),
            base_style,
        )));

        // Stats line with delta coloring
        if advice.win_rate > 0.0 || advice.card_index == usize::MAX {
            lines.push(Line::from(Span::styled(
                format!("     {}", advice.reason),
                if selected { base_style } else { Style::default().fg(delta_color) },
            )));
        } else {
            // Heuristic fallback
            lines.push(Line::from(Span::styled(
                format!("     score={:.2}  {}", advice.score, advice.reason),
                Style::default().fg(Color::DarkGray),
            )));
        }
    }
    if lines.is_empty() {
        lines.push(Line::from(Span::raw("  No cards offered")));
    }

    frame.render_widget(
        Paragraph::new(Text::from(lines))
            .block(Block::default().title(" OFFERED CARDS (ranked by sim) ").borders(Borders::ALL)),
        rows[1],
    );

    let hint = Paragraph::new("  [j/k] navigate  [Enter] pick  [q] back")
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
        App::new(vec![], vec![], vec![])
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
        let app = App::new(vec![enc], vec![], vec![]);
        terminal.draw(|frame| render(frame, &app)).unwrap();
    }
}
