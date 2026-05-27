use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Paragraph, Wrap},
    Frame,
};

use crate::metrics::combat::threat_score;
use crate::tui::app::{App, AppMode};
use crate::tui::widgets::{advice_spans, enemy_row, intent_label_owned};

pub fn render(frame: &mut Frame, app: &App) {
    let area = frame.area();

    match app.mode {
        AppMode::ManualInput => render_input(frame, app, area),
        AppMode::CombatAdvice | AppMode::Simulating => render_combat(frame, app, area),
        AppMode::CardPick => render_card_pick(frame, app, area),
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
        Span::raw("  [s]im  [e]dit  [q]uit"),
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

    let title = Paragraph::new(" CARD PICK — Choose a card to add to your deck")
        .block(Block::default().borders(Borders::BOTTOM))
        .style(Style::default().fg(Color::White).add_modifier(Modifier::BOLD));
    frame.render_widget(title, rows[0]);

    let run = app.run.as_ref();
    let _act = run.map(|r| r.act).unwrap_or(1);

    let mut lines: Vec<Line<'static>> = Vec::new();
    for (i, advice) in app.card_advice.iter().enumerate() {
        let selected = i == app.selected_row;
        let prefix = if selected { "> " } else { "  " };
        let style = if selected {
            Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::White)
        };
        lines.push(Line::from(Span::styled(
            format!(
                "{}[{}] score={:.2}  {}",
                prefix, i + 1, advice.score, advice.reason
            ),
            style,
        )));
    }
    if lines.is_empty() {
        lines.push(Line::from(Span::raw("  No cards offered")));
    }

    frame.render_widget(
        Paragraph::new(Text::from(lines))
            .block(Block::default().title(" OFFERED CARDS ").borders(Borders::ALL)),
        rows[1],
    );

    let hint = Paragraph::new("  [j/k] navigate  [Enter] pick  [q] skip reward")
        .style(Style::default().fg(Color::DarkGray));
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

    #[test]
    fn render_manual_input_does_not_panic() {
        let mut terminal = make_terminal();
        let app = App::new();
        terminal.draw(|frame| render(frame, &app)).unwrap();
    }

    #[test]
    fn render_combat_advice_does_not_panic() {
        let mut terminal = make_terminal();
        let mut app = App::new();
        app.load_combat(default_combat_state());
        terminal.draw(|frame| render(frame, &app)).unwrap();
    }

    #[test]
    fn render_combat_after_sim_does_not_panic() {
        use rand::SeedableRng;
        let mut terminal = make_terminal();
        let mut app = App::new();
        app.load_combat(default_combat_state());
        let mut rng = rand::rngs::StdRng::seed_from_u64(42);
        app.run_simulation(&mut rng);
        terminal.draw(|frame| render(frame, &app)).unwrap();
    }
}
