//! What update passes did to one product's addons, newest first.
//!
//! Kept beside the ledger, never inside it: the ledger is the only record of
//! what may be deleted, and a history that cannot be read must not stop an
//! update.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::manage::{Change, Report};

/// Runs past this many are dropped, oldest first.
const KEPT: usize = 200;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct History {
    #[serde(default)]
    pub runs: Vec<Run>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Trigger {
    /// Lightforge's own pass, with nobody watching.
    Automatic,
    /// Update, Update all, a picked version, or the CLI.
    #[default]
    Manual,
}

/// One pass that changed something, or could not.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Run {
    /// Seconds since the epoch.
    pub at: i64,
    pub trigger: Trigger,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub updated: Vec<Change>,
    /// One line each: a failed check, a skipped pass, an addon left out.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub problems: Vec<String>,
}

impl Run {
    /// `None` when the pass changed nothing and nothing went wrong. Addons
    /// left out count only for the automatic pass: a manual run shows them
    /// on screen as it ends.
    pub fn of(report: &Report, trigger: Trigger) -> Option<Self> {
        let problems: Vec<String> = match trigger {
            Trigger::Automatic => {
                report.unavailable.iter().map(|addon| format!("{}: {}", addon.name, addon.reason)).collect()
            }
            Trigger::Manual => Vec::new(),
        };
        if report.updated.is_empty() && problems.is_empty() {
            return None;
        }
        Some(Self { updated: report.updated.clone(), problems, ..Self::now(trigger) })
    }

    pub fn problem(trigger: Trigger, problem: String) -> Self {
        Self { problems: vec![problem], ..Self::now(trigger) }
    }

    fn now(trigger: Trigger) -> Self {
        let at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or_default();
        Self { at, trigger, updated: Vec::new(), problems: Vec::new() }
    }

    /// When it ran, in local time: `2026-11-27 09:14`.
    pub fn when(&self) -> String {
        let Some(tm) = crate::text::local_time(self.at) else {
            return self.at.to_string();
        };
        format!(
            "{:04}-{:02}-{:02} {:02}:{:02}",
            tm.tm_year + 1900,
            tm.tm_mon + 1,
            tm.tm_mday,
            tm.tm_hour,
            tm.tm_min,
        )
    }
}

impl History {
    /// Named after the ledger at `ledger_path`, so it shares its key.
    pub fn path_for(ledger_path: &Path) -> PathBuf {
        ledger_path.with_extension("history.json")
    }

    /// Missing or unreadable reads as empty: it is shown, never acted on.
    pub fn load(path: &Path) -> Self {
        let bytes = match std::fs::read(path) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Self::default(),
            Err(e) => {
                tracing::warn!("addons: the update history at {} cannot be read: {e}", path.display());
                return Self::default();
            }
        };
        serde_json::from_slice(&bytes).unwrap_or_else(|e| {
            tracing::warn!("addons: the update history at {} is damaged, starting over: {e}", path.display());
            Self::default()
        })
    }

    /// Add `run` beside the ledger at `ledger_path`. Failing costs the record,
    /// not the update, so it is logged and the run goes on.
    pub fn record(ledger_path: &Path, run: Run) {
        let path = Self::path_for(ledger_path);
        if let Err(e) = Self::append(&path, run) {
            tracing::warn!("addons: could not write the update history {}: {e}", path.display());
        }
    }

    /// Add `run` as the newest.
    pub fn append(path: &Path, run: Run) -> std::io::Result<()> {
        let mut history = Self::load(path);
        history.runs.insert(0, run);
        history.runs.truncate(KEPT);
        let json = serde_json::to_vec_pretty(&history).expect("history serializes");
        crate::fsutil::write_atomic(path, &json)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::addons::Unavailable;

    fn change(name: &str) -> Change {
        Change { name: name.to_owned(), from: Some("v1".to_owned()), to: Some("v2".to_owned()) }
    }

    #[test]
    fn a_pass_that_changed_nothing_leaves_no_run() {
        let report = Report { up_to_date: vec!["BigWigs".to_owned()], ..Report::default() };
        assert_eq!(Run::of(&report, Trigger::Automatic), None);
    }

    fn report_with_one_left_out() -> Report {
        Report {
            updated: vec![change("Details!")],
            unavailable: vec![Unavailable {
                name: "BigWigs".to_owned(),
                reason: "the author opted out".to_owned(),
                website_url: None,
                github: None,
            }],
            ..Report::default()
        }
    }

    #[test]
    fn an_addon_the_automatic_pass_left_out_is_a_problem_of_the_run() {
        let run = Run::of(&report_with_one_left_out(), Trigger::Automatic).unwrap();
        assert_eq!(run.updated, [change("Details!")]);
        assert_eq!(run.problems, ["BigWigs: the author opted out"]);
    }

    #[test]
    fn a_manual_run_records_only_what_it_updated() {
        let run = Run::of(&report_with_one_left_out(), Trigger::Manual).unwrap();
        assert_eq!(run.updated, [change("Details!")]);
        assert!(run.problems.is_empty());
    }

    #[test]
    fn the_newest_run_comes_first_and_the_oldest_falls_off() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("retail.history.json");
        for at in 0..=KEPT as i64 {
            History::append(&path, Run { at, ..Run::problem(Trigger::Automatic, "offline".to_owned()) }).unwrap();
        }
        let history = History::load(&path);
        assert_eq!(history.runs.len(), KEPT);
        assert_eq!(history.runs[0].at, KEPT as i64);
        assert_eq!(history.runs.last().unwrap().at, 1);
    }

    #[test]
    fn a_damaged_history_reads_as_empty() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("retail.history.json");
        std::fs::write(&path, b"{ not json").unwrap();
        assert_eq!(History::load(&path), History::default());
    }

    #[test]
    fn it_is_named_after_its_ledger() {
        let path = History::path_for(Path::new("/data/addons/wow_classic_era-1a2b3c4d.json"));
        assert_eq!(path, Path::new("/data/addons/wow_classic_era-1a2b3c4d.history.json"));
    }
}
