use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
};

use crate::domain::card::Card;
use crate::domain::combat::{EnemyState, Intent};
use crate::sim::mcts::PlayAdvice;

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

    let dist_color = if advice.hp_loss_p90 >= 20.0 {
        Color::Red
    } else if advice.hp_loss_p90 >= 10.0 {
        Color::Yellow
    } else {
        Color::Green
    };
    lines.push(Line::from(Span::styled(
        format!(
            "  HP risk: p10={:.0}  p50={:.0}  p90={:.0}",
            advice.hp_loss_p10, advice.hp_loss_p50, advice.hp_loss_p90
        ),
        Style::default().fg(dist_color),
    )));

    lines.push(Line::from(Span::raw(format!(
        "  Simulations: {}",
        advice.simulation_count
    ))));

    Text::from(lines)
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
