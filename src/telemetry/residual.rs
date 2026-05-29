use serde::{Deserialize, Serialize};

// ── Types ──────────────────────────────────────────────────────────────────────

/// One act's worth of prediction error.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActResidual {
    pub sub_act: String,
    /// HP delta the DP predicted at act start (best path from entry to boss).
    pub predicted_delta: f32,
    /// Actual HP delta over the act (hp_end - hp_start).
    pub actual_delta: f32,
}

impl ActResidual {
    /// Signed error: positive = we overestimated HP loss (too pessimistic).
    pub fn error(&self) -> f32 {
        self.predicted_delta - self.actual_delta
    }
}

// ── Store ──────────────────────────────────────────────────────────────────────

/// In-memory accumulator of act-level HP prediction residuals.
#[derive(Debug, Default)]
pub struct ResidualStore {
    records: Vec<ActResidual>,
}

impl ResidualStore {
    pub fn new() -> Self { Self::default() }

    /// Record one act's worth of prediction vs reality.
    pub fn record(&mut self, residual: ActResidual) {
        self.records.push(residual);
    }

    /// All recorded residuals.
    pub fn records(&self) -> &[ActResidual] { &self.records }

    /// Calibration summary for one sub-act: (mean_error, sample_count).
    /// Returns `None` if no records exist for that sub-act.
    pub fn summary_for(&self, sub_act: &str) -> Option<CalibrationSummary> {
        let matching: Vec<&ActResidual> = self.records.iter()
            .filter(|r| r.sub_act == sub_act)
            .collect();
        if matching.is_empty() {
            return None;
        }
        let n = matching.len();
        let mean_error = matching.iter().map(|r| r.error()).sum::<f32>() / n as f32;
        let mean_predicted = matching.iter().map(|r| r.predicted_delta).sum::<f32>() / n as f32;
        let mean_actual = matching.iter().map(|r| r.actual_delta).sum::<f32>() / n as f32;
        Some(CalibrationSummary {
            sub_act: sub_act.to_string(),
            sample_count: n,
            mean_predicted,
            mean_actual,
            mean_error,
        })
    }

    /// All unique sub-acts that have at least one record.
    pub fn sub_acts(&self) -> Vec<String> {
        let mut seen = std::collections::HashSet::new();
        let mut out = Vec::new();
        for r in &self.records {
            if seen.insert(r.sub_act.clone()) {
                out.push(r.sub_act.clone());
            }
        }
        out
    }

    /// Overall calibration summary across all acts.
    pub fn overall_summary(&self) -> Option<CalibrationSummary> {
        if self.records.is_empty() {
            return None;
        }
        let n = self.records.len();
        let mean_error = self.records.iter().map(|r| r.error()).sum::<f32>() / n as f32;
        let mean_predicted = self.records.iter().map(|r| r.predicted_delta).sum::<f32>() / n as f32;
        let mean_actual = self.records.iter().map(|r| r.actual_delta).sum::<f32>() / n as f32;
        Some(CalibrationSummary {
            sub_act: "all".to_string(),
            sample_count: n,
            mean_predicted,
            mean_actual,
            mean_error,
        })
    }
}

/// Aggregated calibration data for a single sub-act (or "all").
#[derive(Debug, Clone)]
pub struct CalibrationSummary {
    pub sub_act: String,
    pub sample_count: usize,
    pub mean_predicted: f32,
    pub mean_actual: f32,
    /// mean(predicted - actual): positive = we predicted more loss than occurred.
    pub mean_error: f32,
}

// ── Tests ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_sign_convention() {
        let r = ActResidual {
            sub_act: "overgrowth".into(),
            predicted_delta: -20.0,
            actual_delta: -15.0,
        };
        // We predicted -20 (more loss), actual was -15 (less loss).
        // Error = predicted - actual = -20 - (-15) = -5 (we were too pessimistic about loss).
        assert_eq!(r.error(), -5.0);
    }

    #[test]
    fn summary_for_single_act() {
        let mut store = ResidualStore::new();
        store.record(ActResidual { sub_act: "overgrowth".into(), predicted_delta: -20.0, actual_delta: -10.0 });
        store.record(ActResidual { sub_act: "overgrowth".into(), predicted_delta: -30.0, actual_delta: -20.0 });
        let s = store.summary_for("overgrowth").unwrap();
        assert_eq!(s.sample_count, 2);
        assert_eq!(s.mean_predicted, -25.0);
        assert_eq!(s.mean_actual, -15.0);
        // error = predicted - actual = -25 - (-15) = -10
        assert_eq!(s.mean_error, -10.0);
    }

    #[test]
    fn summary_for_missing_act_returns_none() {
        let store = ResidualStore::new();
        assert!(store.summary_for("overgrowth").is_none());
    }

    #[test]
    fn sub_acts_deduplicated() {
        let mut store = ResidualStore::new();
        store.record(ActResidual { sub_act: "overgrowth".into(), predicted_delta: -10.0, actual_delta: -8.0 });
        store.record(ActResidual { sub_act: "overgrowth".into(), predicted_delta: -12.0, actual_delta: -9.0 });
        store.record(ActResidual { sub_act: "hive".into(), predicted_delta: -20.0, actual_delta: -18.0 });
        let acts = store.sub_acts();
        assert_eq!(acts.len(), 2);
    }

    #[test]
    fn overall_summary_aggregates_all() {
        let mut store = ResidualStore::new();
        store.record(ActResidual { sub_act: "overgrowth".into(), predicted_delta: -10.0, actual_delta: -5.0 });
        store.record(ActResidual { sub_act: "hive".into(), predicted_delta: -30.0, actual_delta: -25.0 });
        let s = store.overall_summary().unwrap();
        assert_eq!(s.sample_count, 2);
        assert_eq!(s.mean_predicted, -20.0);
        assert_eq!(s.mean_actual, -15.0);
        assert_eq!(s.mean_error, -5.0);
    }

    #[test]
    fn overall_summary_empty_returns_none() {
        let store = ResidualStore::new();
        assert!(store.overall_summary().is_none());
    }

    #[test]
    fn residual_serialization_roundtrip() {
        let r = ActResidual {
            sub_act: "overgrowth".into(),
            predicted_delta: -15.5,
            actual_delta: -10.2,
        };
        let json = serde_json::to_string(&r).unwrap();
        let back: ActResidual = serde_json::from_str(&json).unwrap();
        assert_eq!(back.sub_act, "overgrowth");
        assert!((back.predicted_delta - (-15.5)).abs() < 0.01);
    }
}
