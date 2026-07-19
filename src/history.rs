use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::model::{Action, Candidate, CandidateSource, ExecutionMode, InsertionPolicy, Value};

const SCORE_UNIT: u64 = 1_000;
const HALF_LIFE_SECS: u64 = 30 * 24 * 60 * 60;
const FIELD_COUNT: usize = 10;
const LEGACY_FIELD_COUNT: usize = 9;

#[derive(Debug, Clone, Default)]
pub struct History {
    path: Option<PathBuf>,
    records: BTreeMap<String, HistoryRecord>,
}

#[derive(Debug, Clone)]
struct HistoryRecord {
    key: String,
    score: u64,
    last_chosen: u64,
    candidate: Candidate,
}

#[derive(Debug)]
pub enum HistoryError {
    CreateDir {
        path: PathBuf,
        source: std::io::Error,
    },
    Read {
        path: PathBuf,
        source: std::io::Error,
    },
    Write {
        path: PathBuf,
        source: std::io::Error,
    },
}

impl History {
    pub fn load() -> Result<Self, HistoryError> {
        let Some(path) = history_path() else {
            return Ok(Self::default());
        };

        Self::load_from_path(path)
    }

    pub fn load_from_path(path: impl Into<PathBuf>) -> Result<Self, HistoryError> {
        let path = path.into();
        if !path.exists() {
            return Ok(Self {
                path: Some(path),
                records: BTreeMap::new(),
            });
        }

        let text = fs::read_to_string(&path).map_err(|source| HistoryError::Read {
            path: path.clone(),
            source,
        })?;

        Ok(Self {
            path: Some(path),
            records: text.lines().filter_map(parse_record).collect(),
        })
    }

    pub fn score(&self, candidate: &Candidate) -> u64 {
        if candidate.source() == CandidateSource::Calculator {
            return 0;
        }

        let now = now_secs();
        self.records
            .get(&candidate_key(candidate))
            .map(|record| decayed_score(record.score, record.last_chosen, now))
            .unwrap_or(0)
    }

    pub fn apply_preference(&self, candidate: Candidate) -> Candidate {
        let score = self.score(&candidate);
        candidate.with_preference_score_millis(preference_score_millis(score))
    }

    pub fn candidates(&self) -> Vec<Candidate> {
        let now = now_secs();
        self.records
            .values()
            .filter(|record| record.candidate.source() == CandidateSource::History)
            .map(|record| {
                record
                    .candidate
                    .clone()
                    .with_preference_score_millis(preference_score_millis(decayed_score(
                        record.score,
                        record.last_chosen,
                        now,
                    )))
            })
            .collect()
    }

    pub fn record(&mut self, candidate: &Candidate) -> Result<u64, HistoryError> {
        if candidate.source() == CandidateSource::Calculator {
            return Ok(0);
        }

        let now = now_secs();
        let key = candidate_key(candidate);
        let previous_score = self
            .records
            .get(&key)
            .map(|record| decayed_score(record.score, record.last_chosen, now))
            .unwrap_or(0);
        let score = previous_score.saturating_add(SCORE_UNIT);

        self.records.insert(
            key.clone(),
            HistoryRecord {
                key,
                score,
                last_chosen: now,
                candidate: candidate
                    .clone()
                    .with_preference_score_millis(preference_score_millis(score)),
            },
        );
        self.write()?;

        Ok(score)
    }

    fn write(&self) -> Result<(), HistoryError> {
        let Some(path) = &self.path else {
            return Ok(());
        };

        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|source| HistoryError::CreateDir {
                path: parent.to_path_buf(),
                source,
            })?;
        }

        let mut text = String::new();
        for record in self.records.values() {
            text.push_str(&format_record(record));
            text.push('\n');
        }

        fs::write(path, text).map_err(|source| HistoryError::Write {
            path: path.clone(),
            source,
        })
    }
}

pub(crate) fn edited_history_candidate(value: Value, origin: &Candidate) -> Candidate {
    Candidate::new_with_action(value, origin.selector(), origin.direct_action().cloned())
        .with_source(CandidateSource::History)
}

fn candidate_key(candidate: &Candidate) -> String {
    format!(
        "{:?}\t{}\t{:?}\t{}",
        candidate.source(),
        candidate.selector(),
        candidate.value().insertion_policy(),
        candidate.value().editable_text()
    )
}

fn history_path() -> Option<PathBuf> {
    if let Some(path) = non_empty_env("XDG_STATE_HOME") {
        return Some(PathBuf::from(path).join("fzlaunch").join("history.tsv"));
    }

    non_empty_env("HOME").map(|home| {
        PathBuf::from(home)
            .join(".local")
            .join("state")
            .join("fzlaunch")
            .join("history.tsv")
    })
}

fn non_empty_env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|value| !value.is_empty())
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time should be after unix epoch")
        .as_secs()
}

fn decayed_score(score: u64, last_chosen: u64, now: u64) -> u64 {
    if score == 0 || now <= last_chosen {
        return score;
    }

    let age = now - last_chosen;
    (score as f64 * 0.5_f64.powf(age as f64 / HALF_LIFE_SECS as f64)).round() as u64
}

fn preference_score_millis(score: u64) -> u32 {
    let selections = score as f64 / SCORE_UNIT as f64;

    ((selections.min(1.0) + 4.0 * selections.min(2.0) + 2.0 * selections.min(6.0)) * 1_000.0)
        .round() as u32
}

fn format_record(record: &HistoryRecord) -> String {
    let direct_action = record.candidate.direct_action();
    [
        record.key.as_str(),
        &record.score.to_string(),
        &record.last_chosen.to_string(),
        source_name(record.candidate.source()),
        &record.candidate.selector().to_string(),
        policy_name(record.candidate.value().insertion_policy()),
        record.candidate.value().editable_text(),
        direct_action
            .map(|action| policy_name(action.value().insertion_policy()))
            .unwrap_or(""),
        direct_action
            .map(|action| action.value().editable_text())
            .unwrap_or_default(),
        direct_action
            .map(|action| execution_mode_name(action.execution_mode()))
            .unwrap_or_default(),
    ]
    .into_iter()
    .map(escape_field)
    .collect::<Vec<_>>()
    .join("\t")
}

fn parse_record(line: &str) -> Option<(String, HistoryRecord)> {
    let fields = line
        .split('\t')
        .map(unescape_field)
        .collect::<Option<Vec<_>>>()?;
    if fields.len() != FIELD_COUNT && fields.len() != LEGACY_FIELD_COUNT {
        return None;
    }

    let key = fields[0].clone();
    let score = fields[1].parse().ok()?;
    let last_chosen = fields[2].parse().ok()?;
    let source = parse_source(&fields[3])?;
    let selector = parse_selector(&fields[4])?;
    let value = parse_value(&fields[5], &fields[6])?;
    let direct_action = if fields[7].is_empty() {
        None
    } else {
        let mode = fields
            .get(9)
            .and_then(|field| parse_execution_mode(field))
            .unwrap_or(ExecutionMode::Foreground);
        Some(Action::new(parse_value(&fields[7], &fields[8])?, mode))
    };
    let candidate = Candidate::new_with_action(value, selector, direct_action).with_source(source);

    Some((
        key.clone(),
        HistoryRecord {
            key,
            score,
            last_chosen,
            candidate,
        },
    ))
}

fn parse_selector(text: &str) -> Option<char> {
    let mut chars = text.chars();
    let selector = chars.next()?;
    chars.next().is_none().then_some(selector)
}

fn parse_value(policy: &str, text: &str) -> Option<Value> {
    match policy {
        "raw" => Some(Value::raw(text)),
        "escaped" => Some(Value::escaped(text)),
        _ => None,
    }
}

fn policy_name(policy: InsertionPolicy) -> &'static str {
    match policy {
        InsertionPolicy::Raw => "raw",
        InsertionPolicy::Escaped => "escaped",
    }
}

fn parse_execution_mode(mode: &str) -> Option<ExecutionMode> {
    match mode {
        "foreground" => Some(ExecutionMode::Foreground),
        "detached" => Some(ExecutionMode::Detached),
        _ => None,
    }
}

fn execution_mode_name(mode: ExecutionMode) -> &'static str {
    match mode {
        ExecutionMode::Foreground => "foreground",
        ExecutionMode::Detached => "detached",
    }
}

fn parse_source(source: &str) -> Option<CandidateSource> {
    match source {
        "generic" => Some(CandidateSource::Generic),
        "path" => Some(CandidateSource::PathExecutable),
        "filesystem" => Some(CandidateSource::FilesystemPath),
        "calculator" => Some(CandidateSource::Calculator),
        "plugin" => Some(CandidateSource::Plugin),
        "history" => Some(CandidateSource::History),
        _ => None,
    }
}

fn source_name(source: CandidateSource) -> &'static str {
    match source {
        CandidateSource::Generic => "generic",
        CandidateSource::PathExecutable => "path",
        CandidateSource::FilesystemPath => "filesystem",
        CandidateSource::Calculator => "calculator",
        CandidateSource::Plugin => "plugin",
        CandidateSource::History => "history",
    }
}

fn escape_field(text: &str) -> String {
    let mut escaped = String::new();
    for character in text.chars() {
        match character {
            '\\' => escaped.push_str("\\\\"),
            '\t' => escaped.push_str("\\t"),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            _ => escaped.push(character),
        }
    }

    escaped
}

fn unescape_field(text: &str) -> Option<String> {
    let mut unescaped = String::new();
    let mut chars = text.chars();

    while let Some(character) = chars.next() {
        if character != '\\' {
            unescaped.push(character);
            continue;
        }

        match chars.next()? {
            '\\' => unescaped.push('\\'),
            't' => unescaped.push('\t'),
            'n' => unescaped.push('\n'),
            'r' => unescaped.push('\r'),
            _ => return None,
        }
    }

    Some(unescaped)
}

impl fmt::Display for HistoryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CreateDir { path, source } => {
                write!(
                    formatter,
                    "failed to create history directory {}: {source}",
                    path.display()
                )
            }
            Self::Read { path, source } => {
                write!(formatter, "failed to read {}: {source}", path.display())
            }
            Self::Write { path, source } => {
                write!(formatter, "failed to write {}: {source}", path.display())
            }
        }
    }
}

impl std::error::Error for HistoryError {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TempDir;

    #[test]
    fn records_and_loads_candidate_scores() {
        let root = TempDir::new("history-score");
        let path = root.join("history.tsv");
        let candidate = Candidate::new(Value::raw("bash"), 'c', Some(Value::raw("{}")))
            .with_source(CandidateSource::PathExecutable);
        let mut history = History::load_from_path(&path).expect("history should load");

        history
            .record(&candidate)
            .expect("history should record candidate");
        let history = History::load_from_path(&path).expect("history should reload");

        assert!(history.score(&candidate) > 0);
    }

    #[test]
    fn calculator_candidates_are_not_recorded_or_scored() {
        let root = TempDir::new("history-calculator");
        let path = root.join("history.tsv");
        let candidate = Candidate::new(
            Value::raw("7"),
            '=',
            Some(Value::raw("printf %s {} | wl-copy")),
        )
        .with_source(CandidateSource::Calculator)
        .with_haystack(";= 3 + 4 = 7");
        let mut history = History::load_from_path(&path).expect("history should load");

        assert_eq!(
            history
                .record(&candidate)
                .expect("history should ignore calculator candidate"),
            0
        );

        let history = History::load_from_path(&path).expect("history should reload");
        assert_eq!(history.score(&candidate), 0);
        assert!(history.candidates().is_empty());
    }

    #[test]
    fn loads_edited_choices_as_history_candidates() {
        let root = TempDir::new("history-candidates");
        let path = root.join("history.tsv");
        let origin = Candidate::new(Value::raw("mv"), 'c', Some(Value::raw("{}")))
            .with_source(CandidateSource::PathExecutable);
        let edited = edited_history_candidate(Value::raw("mv {} {}"), &origin);
        let mut history = History::load_from_path(&path).expect("history should load");

        history
            .record(&edited)
            .expect("history should record edited candidate");
        let history = History::load_from_path(&path).expect("history should reload");

        assert_eq!(history.candidates(), vec![edited.with_preference_score(7)]);
    }

    #[test]
    fn preference_score_uses_decayed_selection_formula() {
        assert_eq!(preference_score_millis(0), 0);
        assert_eq!(preference_score_millis(SCORE_UNIT / 2), 3_500);
        assert_eq!(preference_score_millis(SCORE_UNIT), 7_000);
        assert_eq!(preference_score_millis(2 * SCORE_UNIT), 13_000);
        assert_eq!(preference_score_millis(3 * SCORE_UNIT), 15_000);
        assert_eq!(preference_score_millis(4 * SCORE_UNIT), 17_000);
        assert_eq!(preference_score_millis(5 * SCORE_UNIT), 19_000);
        assert_eq!(preference_score_millis(6 * SCORE_UNIT), 21_000);
        assert_eq!(preference_score_millis(7 * SCORE_UNIT), 21_000);
    }
}
