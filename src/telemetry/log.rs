use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

// ── Event types ────────────────────────────────────────────────────────────────

/// A scored item in a decision (card, path node, etc.).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScoredChoice {
    pub label: String,
    pub hp_delta: f32,
}

/// A single event written to the run log.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum RunEvent {
    /// Recorded each time the player enters a map node.
    MapDecision {
        run_id: u64,
        timestamp: u64,
        floor: u8,
        sub_act: String,
        room_type: String,
        hp_before: u32,
        max_hp: u32,
        /// Best-path HP delta predicted by the DP (for the chosen node).
        predicted_hp_delta: f32,
        choices: Vec<ScoredChoice>,
        /// Index into `choices` that was selected (usize::MAX = no map data).
        chose_index: usize,
    },
    /// Recorded when the player picks (or skips) a card reward.
    CardPick {
        run_id: u64,
        timestamp: u64,
        floor: u8,
        sub_act: String,
        hp_before: u32,
        choices: Vec<ScoredChoice>,
        /// Index into `choices` of the picked card (usize::MAX = skip).
        chose_index: usize,
    },
    /// Recorded when an act boss is defeated (and after HP loss applied).
    ActComplete {
        run_id: u64,
        timestamp: u64,
        sub_act: String,
        hp_at_act_start: u32,
        hp_at_act_end: u32,
        max_hp: u32,
        predicted_hp_delta: f32,
        floors_completed: u8,
    },
    /// Recorded at the end of a run (victory or quit).
    RunComplete {
        run_id: u64,
        timestamp: u64,
        sub_act: String,
        final_hp: u32,
        max_hp: u32,
        won: bool,
        deck_size: usize,
        floors_completed: u8,
    },
}

// ── Logger ─────────────────────────────────────────────────────────────────────

/// Append-only JSONL writer for run events.
pub struct RunLogger {
    writer: BufWriter<File>,
}

impl RunLogger {
    /// Open (or create) the JSONL log at `path`, appending to any existing content.
    pub fn open(path: &PathBuf) -> anyhow::Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let file = OpenOptions::new().create(true).append(true).open(path)?;
        Ok(Self { writer: BufWriter::new(file) })
    }

    /// Write one event as a single JSON line.
    pub fn write(&mut self, event: &RunEvent) -> anyhow::Result<()> {
        let line = serde_json::to_string(event)?;
        self.writer.write_all(line.as_bytes())?;
        self.writer.write_all(b"\n")?;
        self.writer.flush()?;
        Ok(())
    }
}

// ── Helpers ────────────────────────────────────────────────────────────────────

/// Seconds since Unix epoch — used as a monotonic timestamp in log records.
pub fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Default log path: `~/.local/share/spire-slayer/runs.jsonl`
/// Falls back to `./runs.jsonl` if the home directory cannot be determined.
pub fn default_log_path() -> PathBuf {
    if let Some(home) = std::env::var_os("HOME") {
        PathBuf::from(home)
            .join(".local/share/spire-slayer/runs.jsonl")
    } else {
        PathBuf::from("runs.jsonl")
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;
    use tempfile::NamedTempFile;

    fn map_decision() -> RunEvent {
        RunEvent::MapDecision {
            run_id: 1,
            timestamp: 0,
            floor: 3,
            sub_act: "overgrowth".into(),
            room_type: "Monster".into(),
            hp_before: 60,
            max_hp: 80,
            predicted_hp_delta: -8.5,
            choices: vec![
                ScoredChoice { label: "col 2 Monster".into(), hp_delta: -8.5 },
                ScoredChoice { label: "col 4 Elite".into(), hp_delta: -22.0 },
            ],
            chose_index: 0,
        }
    }

    fn run_complete() -> RunEvent {
        RunEvent::RunComplete {
            run_id: 1,
            timestamp: 0,
            sub_act: "glory".into(),
            final_hp: 42,
            max_hp: 80,
            won: true,
            deck_size: 17,
            floors_completed: 15,
        }
    }

    #[test]
    fn map_decision_roundtrip() {
        let ev = map_decision();
        let json = serde_json::to_string(&ev).unwrap();
        let back: RunEvent = serde_json::from_str(&json).unwrap();
        match back {
            RunEvent::MapDecision { floor, hp_before, chose_index, .. } => {
                assert_eq!(floor, 3);
                assert_eq!(hp_before, 60);
                assert_eq!(chose_index, 0);
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn run_complete_roundtrip() {
        let ev = run_complete();
        let json = serde_json::to_string(&ev).unwrap();
        let back: RunEvent = serde_json::from_str(&json).unwrap();
        match back {
            RunEvent::RunComplete { won, final_hp, .. } => {
                assert!(won);
                assert_eq!(final_hp, 42);
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn logger_writes_two_lines() {
        let tmp = NamedTempFile::new().unwrap();
        let path = tmp.path().to_path_buf();
        {
            let mut logger = RunLogger::open(&path).unwrap();
            logger.write(&map_decision()).unwrap();
            logger.write(&run_complete()).unwrap();
        }
        let mut contents = String::new();
        std::fs::File::open(&path).unwrap().read_to_string(&mut contents).unwrap();
        let lines: Vec<&str> = contents.lines().collect();
        assert_eq!(lines.len(), 2);
        // Each line is valid JSON containing a "kind" tag.
        for line in &lines {
            assert!(line.contains("\"kind\""), "line missing kind tag: {line}");
        }
    }

    #[test]
    fn logger_appends_on_reopen() {
        let tmp = NamedTempFile::new().unwrap();
        let path = tmp.path().to_path_buf();
        {
            let mut logger = RunLogger::open(&path).unwrap();
            logger.write(&map_decision()).unwrap();
        }
        {
            let mut logger = RunLogger::open(&path).unwrap();
            logger.write(&run_complete()).unwrap();
        }
        let contents = std::fs::read_to_string(&path).unwrap();
        assert_eq!(contents.lines().count(), 2);
    }

    #[test]
    fn act_complete_roundtrip() {
        let ev = RunEvent::ActComplete {
            run_id: 2,
            timestamp: 100,
            sub_act: "overgrowth".into(),
            hp_at_act_start: 80,
            hp_at_act_end: 55,
            max_hp: 80,
            predicted_hp_delta: -18.0,
            floors_completed: 15,
        };
        let json = serde_json::to_string(&ev).unwrap();
        let back: RunEvent = serde_json::from_str(&json).unwrap();
        match back {
            RunEvent::ActComplete { hp_at_act_start, hp_at_act_end, .. } => {
                assert_eq!(hp_at_act_start, 80);
                assert_eq!(hp_at_act_end, 55);
            }
            _ => panic!("wrong variant"),
        }
    }
}
