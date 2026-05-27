use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Gauge, Paragraph},
};

use crate::domain::card::Card;
use crate::domain::combat::{EnemyState, Intent};
use crate::sim::mcts::PlayAdvice;

pub fn card_widget(card: &Card, highlight: bool) -> Paragraph<'static> {
    let style = if highlight {
        Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::White)
    };
    let label = format!("[{}] {}e", card.name, card.cost);
    Paragraph::new(label).style(style)
}

pub fn enemy_row(enemy: &EnemyState, _index: usize) -> Line<'static> {
    let hp_style = Style::default().fg(Color::Green);
    let blk_style = Style::default().fg(Color::Blue);
    let intent = intent_label_owned(&enemy.intent);
    Line::from(vec![
        Span::raw(format!("  {}  ", enemy.name)),
        Span::styled(format!("HP:{}/{}", enemy.hp, enemy.max_hp), hp_style),
        Span::raw("  "),
        Span::styled(format!("BLK:{}", enemy.block), blk_style),
        Span::raw(format!("  Intent: {}", intent)),
    ])
}

pub fn hp_bar(current: u32, max: u32, _width: u16) -> Gauge<'static> {
    let ratio = if max == 0 {
        0.0
    } else {
        (current as f64 / max as f64).clamp(0.0, 1.0)
    };
    let color = if ratio > 0.5 {
        Color::Green
    } else if ratio > 0.25 {
        Color::Yellow
    } else {
        Color::Red
    };
    let label = format!("{}/{}", current, max);
    Gauge::default()
        .block(Block::default().borders(Borders::NONE))
        .gauge_style(Style::default().fg(color))
        .ratio(ratio)
        .label(label)
        .use_unicode(false)
}

pub fn advice_spans(advice: &PlayAdvice, hand: &[Card]) -> Text<'static> {
    let mut lines: Vec<Line<'static>> = Vec::new();

    let play_line = if advice.actions.is_empty() {
        "  Play: (end turn)".to_string()
    } else {
        let names: Vec<String> = advice
            .actions
            .iter()
            .filter_map(|a| hand.get(a.card_hand_idx))
            .map(|c| c.name.clone())
            .collect();
        format!("  Play: {}", names.join(" → "))
    };
    lines.push(Line::from(Span::styled(
        play_line,
        Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
    )));

    lines.push(Line::from(Span::raw(format!(
        "  Expected: {:.0} dmg dealt, {:.0} block, {:+.0} HP",
        advice.expected_damage,
        advice.expected_hp_retained,
        advice.expected_hp_retained - advice.expected_damage,
    ))));

    lines.push(Line::from(Span::raw(format!("  Reason: {}", advice.rationale))));
    lines.push(Line::from(Span::raw(format!(
        "  Simulations: {}",
        advice.simulation_count
    ))));

    Text::from(lines)
}

pub fn intent_label(intent: &Intent) -> &'static str {
    match intent {
        Intent::Block => "Block",
        Intent::Buff => "Buff",
        Intent::Unknown => "Unknown",
        Intent::Escape => "Escape",
        Intent::DebuffPlayer => "Debuff",
        Intent::Attack(_) | Intent::AttackMulti { .. } => "Attack",
    }
}

pub fn intent_label_owned(intent: &Intent) -> String {
    match intent {
        Intent::Attack(d) => format!("Attack {}", d),
        Intent::AttackMulti { damage, hits } => format!("Attack {}x{}", damage, hits),
        Intent::Block => "Block".to_string(),
        Intent::Buff => "Buff".to_string(),
        Intent::DebuffPlayer => "Debuff".to_string(),
        Intent::Escape => "Escape".to_string(),
        Intent::Unknown => "Unknown".to_string(),
    }
}
