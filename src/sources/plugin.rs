use std::process::Stdio;

use serde::Deserialize;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::config::PluginSourceConfig;
use crate::model::{Action, Candidate, CandidateSource, ExecutionMode, Value};

use super::{AsyncSource, CandidateSender};

pub type PluginCandidateSender = mpsc::Sender<PluginCandidateBatch>;
pub type PluginCandidateReceiver = mpsc::Receiver<PluginCandidateBatch>;

#[derive(Debug)]
pub struct PluginCandidateBatch {
    pub source_id: String,
    pub generation: u64,
    pub candidates: Vec<Candidate>,
}

pub struct PluginSource {
    config: PluginSourceConfig,
}

impl PluginSource {
    pub fn new(config: PluginSourceConfig) -> Self {
        Self { config }
    }

    pub fn stream_triggered_candidates(
        config: PluginSourceConfig,
        args: Vec<String>,
        generation: u64,
        sender: PluginCandidateSender,
    ) -> JoinHandle<()> {
        tokio::spawn(async move {
            stream_triggered_plugin(config, args, generation, sender).await;
        })
    }
}

impl AsyncSource for PluginSource {
    fn stream_candidates(self: Box<Self>, sender: CandidateSender) -> JoinHandle<()> {
        tokio::spawn(async move {
            stream_startup_plugin(self.config, sender).await;
        })
    }
}

async fn stream_startup_plugin(config: PluginSourceConfig, sender: CandidateSender) {
    let Some(mut child) = spawn_plugin_process(&config, Vec::new()) else {
        return;
    };
    let Some(stdout) = child.stdout.take() else {
        return;
    };
    let mut lines = BufReader::new(stdout).lines();

    while let Ok(Some(line)) = lines.next_line().await {
        let Some(candidate) = plugin_candidate_from_line(&line, &config) else {
            continue;
        };
        if sender.send(vec![candidate]).await.is_err() {
            return;
        }
    }

    let _ = child.wait().await;
}

async fn stream_triggered_plugin(
    config: PluginSourceConfig,
    args: Vec<String>,
    generation: u64,
    sender: PluginCandidateSender,
) {
    let Some(mut child) = spawn_plugin_process(&config, args) else {
        return;
    };
    let Some(stdout) = child.stdout.take() else {
        return;
    };
    let mut lines = BufReader::new(stdout).lines();

    while let Ok(Some(line)) = lines.next_line().await {
        let Some(candidate) = plugin_candidate_from_line(&line, &config) else {
            continue;
        };
        let batch = PluginCandidateBatch {
            source_id: config.name.clone(),
            generation,
            candidates: vec![candidate],
        };
        if sender.send(batch).await.is_err() {
            return;
        }
    }

    let _ = child.wait().await;
}

fn spawn_plugin_process(
    config: &PluginSourceConfig,
    args: impl IntoIterator<Item = String>,
) -> Option<tokio::process::Child> {
    let mut command = Command::new(&config.path);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);

    command.spawn().ok()
}

#[derive(Debug, Deserialize)]
struct PluginCandidateRecord {
    value: String,
    haystack: Option<String>,
    insertion_policy: Option<String>,
    direct_action: Option<String>,
    direct_action_execution: Option<String>,
}

fn plugin_candidate_from_line(line: &str, config: &PluginSourceConfig) -> Option<Candidate> {
    let record = serde_json::from_str::<PluginCandidateRecord>(line).ok()?;
    let direct_action = plugin_direct_action(&record, config);
    let value = match record.insertion_policy.as_deref().unwrap_or("raw") {
        "raw" => Value::raw(record.value),
        "escaped" => Value::escaped(record.value),
        _ => return None,
    };
    let mut candidate = Candidate::new_with_action(value, config.selector, direct_action)
        .with_source(CandidateSource::Plugin)
        .with_source_id(config.name.clone());

    if let Some(haystack) = record.haystack {
        candidate = candidate.with_haystack(plugin_haystack(config.selector, &haystack));
    }

    Some(candidate)
}

fn plugin_haystack(selector: char, haystack: &str) -> String {
    let haystack = haystack.trim();
    if haystack.is_empty() {
        format!(";{selector}")
    } else {
        format!(";{selector} {haystack}")
    }
}

fn plugin_direct_action(
    record: &PluginCandidateRecord,
    config: &PluginSourceConfig,
) -> Option<Action> {
    record
        .direct_action
        .as_ref()
        .map(|value| {
            Action::new(
                Value::raw(value),
                record
                    .direct_action_execution
                    .as_deref()
                    .and_then(parse_execution_mode)
                    .or_else(|| config.direct_action.as_ref().map(Action::execution_mode))
                    .unwrap_or(ExecutionMode::Foreground),
            )
        })
        .or_else(|| config.direct_action.clone())
}

fn parse_execution_mode(mode: &str) -> Option<ExecutionMode> {
    match mode {
        "foreground" => Some(ExecutionMode::Foreground),
        "detached" => Some(ExecutionMode::Detached),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;

    use crate::config::{PluginSourceConfig, PluginSourceMode};
    use crate::model::{Candidate, CandidateSource, Value};
    use crate::sources::AsyncSource;
    use crate::test_support::TempDir;

    use super::PluginSource;

    fn plugin_config(path: PathBuf) -> PluginSourceConfig {
        PluginSourceConfig {
            name: "content".to_string(),
            enabled: true,
            path,
            selector: 's',
            mode: PluginSourceMode::Startup,
            direct_action: Some(crate::model::Action::detached(Value::raw("xdg-open {}"))),
        }
    }

    fn write_script(dir: &TempDir, text: &str) -> PathBuf {
        let path = dir.join("plugin");
        fs::write(&path, text).expect("plugin script should be written");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755))
            .expect("plugin script should be executable");
        path
    }

    async fn collect_source(source: Box<dyn AsyncSource>) -> Vec<Candidate> {
        let (sender, mut receiver) = tokio::sync::mpsc::channel(8);
        let task = source.stream_candidates(sender);
        let mut candidates = Vec::new();

        while let Some(batch) = receiver.recv().await {
            candidates.extend(batch);
        }

        task.await.expect("source task should finish");
        candidates
    }

    #[test]
    fn plugin_candidate_jsonl_uses_configured_source_fields() {
        let dir = TempDir::new("plugin-candidate-json");
        let config = plugin_config(dir.join("plugin"));

        let candidate = super::plugin_candidate_from_line(
            r#"{"value":"/home/me/note.md","insertion_policy":"escaped","haystack":"matched note"}"#,
            &config,
        )
        .expect("candidate json should parse");

        assert_eq!(
            candidate,
            Candidate::new_with_action(
                Value::escaped("/home/me/note.md"),
                's',
                Some(crate::model::Action::detached(Value::raw("xdg-open {}"))),
            )
            .with_source(CandidateSource::Plugin)
            .with_source_id("content")
            .with_haystack(";s matched note")
        );
    }

    #[tokio::test]
    async fn startup_plugin_streams_jsonl_candidates() {
        let dir = TempDir::new("startup-plugin-source");
        let script = write_script(
            &dir,
            r#"#!/bin/sh
printf '%s\n' '{"value":"first","haystack":"first result"}'
printf '%s\n' '{"value":"second","haystack":"second result"}'
"#,
        );

        let candidates = collect_source(Box::new(PluginSource::new(plugin_config(script)))).await;

        assert_eq!(
            candidates,
            vec![
                Candidate::new_with_action(
                    Value::raw("first"),
                    's',
                    Some(crate::model::Action::detached(Value::raw("xdg-open {}"))),
                )
                .with_source(CandidateSource::Plugin)
                .with_source_id("content")
                .with_haystack(";s first result"),
                Candidate::new_with_action(
                    Value::raw("second"),
                    's',
                    Some(crate::model::Action::detached(Value::raw("xdg-open {}"))),
                )
                .with_source(CandidateSource::Plugin)
                .with_source_id("content")
                .with_haystack(";s second result"),
            ]
        );
    }
}
