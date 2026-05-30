use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use std::sync::mpsc;
use std::time::Duration;

use crate::domain::combat::CombatState;
use crate::metrics::path_sim::PathProjection;

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub enum AppEvent {
    Key(KeyEvent),
    StateUpdated(Box<CombatState>),
    CardPickConfirmed(usize),
    Quit,
    RunSim,
    Tick,
    /// Background path simulation finished; deliver results to the app.
    /// The u64 is the generation counter: stale results from a previous floor are discarded.
    PathSimResult(u64, Vec<PathProjection>),
}

/// Spawn a background thread that reads crossterm events and sends them to the returned channel.
/// Ticks are sent when no input arrives within `tick_rate_ms`.
pub fn spawn_event_loop(tick_rate_ms: u64) -> mpsc::Receiver<AppEvent> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let tick = Duration::from_millis(tick_rate_ms);
        loop {
            match event::poll(tick) {
                Ok(true) => match event::read() {
                    Ok(Event::Key(key)) if key.kind == KeyEventKind::Press => {
                        // Ctrl-C / Ctrl-Q both quit
                        let app_event = if key.code == KeyCode::Char('c')
                            && key.modifiers.contains(KeyModifiers::CONTROL)
                        {
                            AppEvent::Quit
                        } else {
                            AppEvent::Key(key)
                        };
                        if tx.send(app_event).is_err() {
                            break;
                        }
                    }
                    Ok(_) => {}
                    Err(_) => break,
                },
                Ok(false) => {
                    if tx.send(AppEvent::Tick).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });
    rx
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_event_is_clone() {
        let key = KeyEvent::new(KeyCode::Char('s'), KeyModifiers::NONE);
        let ev = AppEvent::Key(key);
        let _ = ev.clone();
    }

    #[test]
    fn quit_event_debug() {
        let ev = AppEvent::Quit;
        assert!(format!("{ev:?}").contains("Quit"));
    }

    #[test]
    fn run_sim_event_debug() {
        let ev = AppEvent::RunSim;
        assert!(format!("{ev:?}").contains("RunSim"));
    }
}
