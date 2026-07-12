use std::collections::BTreeSet;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use crate::config::PathSourceConfig;
use crate::model::{Action, Candidate, CandidateSource, ExecutionMode, Value};
use tokio::task::JoinHandle;

use super::{AsyncSource, CandidateSender};

pub struct Executables {
    pub path_dirs: Vec<PathBuf>,
    pub desktop_dirs: Vec<PathBuf>,
    config: PathSourceConfig,
}

impl Executables {
    #[cfg(test)]
    pub fn from_path(path: &str) -> Self {
        Self::from_path_with_config(path, PathSourceConfig::default())
    }

    #[cfg(test)]
    pub fn from_path_with_config(path: &str, config: PathSourceConfig) -> Self {
        Self::from_path_and_data_dirs_with_config(path, "", config)
    }

    pub fn from_path_and_data_dirs_with_config(
        path: &str,
        data_dirs: &str,
        config: PathSourceConfig,
    ) -> Self {
        let path_dirs = split_env_paths(path);
        let desktop_dirs = split_env_paths(data_dirs)
            .into_iter()
            .map(|dir| dir.join("applications"))
            .collect();

        Self {
            path_dirs,
            desktop_dirs,
            config,
        }
    }

    fn stream_candidate_batches(&self, sender: CandidateSender) {
        let mut seen = BTreeSet::new();

        for dir in &self.desktop_dirs {
            let candidates = desktop_executables_in_dir(dir)
                .into_iter()
                .filter(|executable| seen.insert(executable.command.clone()))
                .map(|executable| desktop_executable_candidate(executable, &self.config))
                .collect::<Vec<_>>();

            if !candidates.is_empty() && sender.blocking_send(candidates).is_err() {
                break;
            }
        }

        for dir in &self.path_dirs {
            let candidates = executable_commands_in_dir(dir)
                .into_iter()
                .filter(|command| seen.insert(command.clone()))
                .map(|command| executable_candidate(command, &self.config))
                .collect::<Vec<_>>();

            if !candidates.is_empty() && sender.blocking_send(candidates).is_err() {
                break;
            }
        }
    }
}

impl AsyncSource for Executables {
    fn stream_candidates(self: Box<Self>, sender: CandidateSender) -> JoinHandle<()> {
        tokio::task::spawn_blocking(move || {
            self.stream_candidate_batches(sender);
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct DesktopExecutable {
    command: String,
    execution_mode: ExecutionMode,
}

fn split_env_paths(paths: &str) -> Vec<PathBuf> {
    if paths.is_empty() {
        Vec::new()
    } else {
        std::env::split_paths(paths).collect()
    }
}

fn executable_commands_in_dir(dir: &Path) -> BTreeSet<String> {
    let mut commands = BTreeSet::new();
    let Ok(entries) = fs::read_dir(dir) else {
        return commands;
    };

    for entry in entries.flatten() {
        if !is_executable_file(&entry.path()) {
            continue;
        }

        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };

        commands.insert(name);
    }

    commands
}

fn desktop_executables_in_dir(dir: &Path) -> Vec<DesktopExecutable> {
    let mut executables = Vec::new();
    let Ok(entries) = fs::read_dir(dir) else {
        return executables;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|extension| extension.to_str()) != Some("desktop") {
            continue;
        }

        let Ok(text) = fs::read_to_string(path) else {
            continue;
        };
        if let Some(executable) = parse_desktop_executable(&text) {
            executables.push(executable);
        }
    }

    executables.sort();
    executables
}

fn parse_desktop_executable(text: &str) -> Option<DesktopExecutable> {
    let mut in_desktop_entry = false;
    let mut exec = None;
    let mut terminal = false;
    let mut hidden = false;

    for line in text.lines().map(str::trim) {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        if line.starts_with('[') && line.ends_with(']') {
            in_desktop_entry = line == "[Desktop Entry]";
            continue;
        }

        if !in_desktop_entry {
            continue;
        }

        if let Some(value) = line.strip_prefix("Exec=") {
            exec = desktop_exec_binary(value);
        } else if let Some(value) = line.strip_prefix("Terminal=") {
            terminal = value.eq_ignore_ascii_case("true");
        } else if let Some(value) = line.strip_prefix("NoDisplay=") {
            hidden |= value.eq_ignore_ascii_case("true");
        } else if let Some(value) = line.strip_prefix("Hidden=") {
            hidden |= value.eq_ignore_ascii_case("true");
        }
    }

    if hidden {
        return None;
    }

    Some(DesktopExecutable {
        command: exec?,
        execution_mode: if terminal {
            ExecutionMode::Foreground
        } else {
            ExecutionMode::Detached
        },
    })
}

fn desktop_exec_binary(exec: &str) -> Option<String> {
    exec.split_whitespace()
        .find(|term| !term.starts_with('%'))
        .map(|term| term.trim_matches('"').trim_matches('\'').to_string())
        .filter(|term| !term.is_empty())
        .and_then(|term| {
            Path::new(&term)
                .file_name()
                .and_then(|name| name.to_str())
                .map(str::to_string)
        })
}

fn is_executable_file(path: &Path) -> bool {
    let Ok(metadata) = fs::metadata(path) else {
        return false;
    };

    metadata.is_file() && metadata.permissions().mode() & 0o111 != 0
}

fn executable_candidate(command: String, config: &PathSourceConfig) -> Candidate {
    Candidate::new_with_action(Value::raw(command), 'c', Some(config.direct_action.clone()))
        .with_source(CandidateSource::PathExecutable)
}

fn desktop_executable_candidate(
    executable: DesktopExecutable,
    config: &PathSourceConfig,
) -> Candidate {
    Candidate::new_with_action(
        Value::raw(executable.command),
        'c',
        Some(Action::new(
            config.direct_action.value().clone(),
            executable.execution_mode,
        )),
    )
    .with_source(CandidateSource::PathExecutable)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::{symlink, PermissionsExt};
    use std::path::{Path, PathBuf};

    use crate::config::PathSourceConfig;
    use crate::model::{Action, Candidate, CandidateSource, ExecutionMode, Value};
    use crate::sources::AsyncSource;
    use crate::state::LauncherState;
    use crate::test_support::{path_string, TempDir};

    use super::Executables;

    fn temp_source_dir(name: &str) -> TempDir {
        TempDir::new(name)
    }

    fn write_file(path: PathBuf, mode: u32) {
        fs::write(&path, b"#!/bin/sh\n").expect("test executable should be written");
        fs::set_permissions(&path, fs::Permissions::from_mode(mode))
            .expect("test executable permissions should be set");
    }

    fn write_desktop_file(dir: &Path, name: &str, text: &str) {
        fs::create_dir_all(dir).expect("desktop directory should be created");
        fs::write(dir.join(name), text).expect("desktop file should be written");
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

    fn expected_executable(command: &str) -> Candidate {
        Candidate::new(Value::raw(command), 'c', Some(Value::raw("{}")))
            .with_source(CandidateSource::PathExecutable)
    }

    fn expected_executable_with_mode(command: &str, mode: ExecutionMode) -> Candidate {
        Candidate::new_with_action(
            Value::raw(command),
            'c',
            Some(Action::new(Value::raw("{}"), mode)),
        )
        .with_source(CandidateSource::PathExecutable)
    }

    #[tokio::test]
    async fn path_source_returns_executables_as_raw_command_candidates() {
        let bin = temp_source_dir("path-source-executable");
        write_file(bin.join("fzlaunch-test-command"), 0o755);

        let candidates = collect_source(Box::new(Executables::from_path(
            bin.to_str().expect("path should be utf-8"),
        )))
        .await;

        assert_eq!(
            candidates,
            vec![expected_executable("fzlaunch-test-command")]
        );
    }

    #[tokio::test]
    async fn path_source_returns_symlinked_executables() {
        let target_dir = temp_source_dir("path-source-symlink-target");
        let bin = temp_source_dir("path-source-symlink-bin");
        let target = target_dir.join("fzlaunch-test-command");
        write_file(target.clone(), 0o755);
        symlink(target, bin.join("fzlaunch-test-link")).expect("test symlink should be created");

        let candidates = collect_source(Box::new(Executables::from_path(
            bin.to_str().expect("path should be utf-8"),
        )))
        .await;

        assert_eq!(candidates, vec![expected_executable("fzlaunch-test-link")]);
    }

    #[tokio::test]
    async fn path_source_ignores_non_executable_files() {
        let bin = temp_source_dir("path-source-non-executable");
        write_file(bin.join("not-executable"), 0o644);

        let candidates = collect_source(Box::new(Executables::from_path(
            bin.to_str().expect("path should be utf-8"),
        )))
        .await;

        assert_eq!(candidates, Vec::<Candidate>::new());
    }

    #[tokio::test]
    async fn path_source_uses_configured_actions() {
        let bin = temp_source_dir("path-source-configured-actions");
        write_file(bin.join("fzlaunch-test-command"), 0o755);

        let candidates = collect_source(Box::new(Executables::from_path_with_config(
            bin.to_str().expect("path should be utf-8"),
            PathSourceConfig {
                direct_action: Action::foreground(Value::raw("run-command {}")),
                preview_command: Value::raw("help-command {}"),
                ..PathSourceConfig::default()
            },
        )))
        .await;

        assert_eq!(
            candidates,
            vec![Candidate::new(
                Value::raw("fzlaunch-test-command"),
                'c',
                Some(Value::raw("run-command {}"))
            )
            .with_source(CandidateSource::PathExecutable)]
        );
    }

    #[tokio::test]
    async fn executable_source_reads_detached_desktop_entries() {
        let data = temp_source_dir("executable-source-desktop-detached");
        let applications = data.join("applications");
        write_desktop_file(
            &applications,
            "monitor.desktop",
            "[Desktop Entry]\nExec=/usr/bin/gnome-system-monitor\nTerminal=false\n",
        );

        let candidates =
            collect_source(Box::new(Executables::from_path_and_data_dirs_with_config(
                "",
                data.to_str().expect("path should be utf-8"),
                PathSourceConfig::default(),
            )))
            .await;

        assert_eq!(
            candidates,
            vec![expected_executable_with_mode(
                "gnome-system-monitor",
                ExecutionMode::Detached
            )]
        );
    }

    #[tokio::test]
    async fn executable_source_reads_foreground_desktop_entries() {
        let data = temp_source_dir("executable-source-desktop-foreground");
        let applications = data.join("applications");
        write_desktop_file(
            &applications,
            "htop.desktop",
            "[Desktop Entry]\nExec=htop\nTerminal=true\n",
        );

        let candidates =
            collect_source(Box::new(Executables::from_path_and_data_dirs_with_config(
                "",
                data.to_str().expect("path should be utf-8"),
                PathSourceConfig::default(),
            )))
            .await;

        assert_eq!(
            candidates,
            vec![expected_executable_with_mode(
                "htop",
                ExecutionMode::Foreground
            )]
        );
    }

    #[tokio::test]
    async fn executable_source_ignores_hidden_desktop_entries() {
        let data = temp_source_dir("executable-source-desktop-hidden");
        let applications = data.join("applications");
        write_desktop_file(
            &applications,
            "hidden.desktop",
            "[Desktop Entry]\nExec=hidden-command\nNoDisplay=true\n",
        );
        write_desktop_file(
            &applications,
            "deleted.desktop",
            "[Desktop Entry]\nExec=deleted-command\nHidden=true\n",
        );

        let candidates =
            collect_source(Box::new(Executables::from_path_and_data_dirs_with_config(
                "",
                data.to_str().expect("path should be utf-8"),
                PathSourceConfig::default(),
            )))
            .await;

        assert_eq!(candidates, Vec::<Candidate>::new());
    }

    #[tokio::test]
    async fn executable_source_skips_path_executables_covered_by_desktop_entries() {
        let data = temp_source_dir("executable-source-desktop-deduplicate");
        let applications = data.join("applications");
        write_desktop_file(
            &applications,
            "monitor.desktop",
            "[Desktop Entry]\nExec=gnome-system-monitor\nTerminal=false\n",
        );
        let bin = temp_source_dir("executable-source-desktop-deduplicate-bin");
        write_file(bin.join("gnome-system-monitor"), 0o755);

        let candidates =
            collect_source(Box::new(Executables::from_path_and_data_dirs_with_config(
                bin.to_str().expect("path should be utf-8"),
                data.to_str().expect("path should be utf-8"),
                PathSourceConfig::default(),
            )))
            .await;

        assert_eq!(
            candidates,
            vec![expected_executable_with_mode(
                "gnome-system-monitor",
                ExecutionMode::Detached
            )]
        );
    }

    #[tokio::test]
    async fn hidden_desktop_entries_do_not_shadow_path_executables() {
        let data = temp_source_dir("executable-source-hidden-desktop-path");
        let applications = data.join("applications");
        write_desktop_file(
            &applications,
            "userapp-nvim.desktop",
            "[Desktop Entry]\nExec=nvim %f\nName=nvim\nNoDisplay=true\n",
        );
        let bin = temp_source_dir("executable-source-hidden-desktop-path-bin");
        write_file(bin.join("nvim"), 0o755);

        let candidates =
            collect_source(Box::new(Executables::from_path_and_data_dirs_with_config(
                bin.to_str().expect("path should be utf-8"),
                data.to_str().expect("path should be utf-8"),
                PathSourceConfig::default(),
            )))
            .await;

        assert_eq!(
            candidates,
            vec![expected_executable_with_mode(
                "nvim",
                ExecutionMode::Foreground
            )]
        );
    }

    #[tokio::test]
    async fn path_source_deduplicates_commands_from_multiple_path_entries() {
        let first = temp_source_dir("path-source-first");
        let second = temp_source_dir("path-source-second");
        write_file(first.join("shared-command"), 0o755);
        write_file(second.join("shared-command"), 0o755);

        let candidates = collect_source(Box::new(Executables::from_path(&path_string([
            &first, &second,
        ]))))
        .await;

        assert_eq!(candidates, vec![expected_executable("shared-command")]);
    }

    #[tokio::test]
    async fn path_source_ignores_missing_path_entries() {
        let missing_root = temp_source_dir("path-source-missing");
        let missing = missing_root.join("missing");
        let bin = temp_source_dir("path-source-existing");
        write_file(bin.join("existing-command"), 0o755);

        let candidates = collect_source(Box::new(Executables::from_path(&path_string([
            missing.as_os_str(),
            bin.as_os_str(),
        ]))))
        .await;

        assert_eq!(candidates, vec![expected_executable("existing-command")]);
    }

    #[tokio::test]
    async fn path_source_returns_commands_in_sorted_order() {
        let bin = temp_source_dir("path-source-sorted");
        write_file(bin.join("z-command"), 0o755);
        write_file(bin.join("a-command"), 0o755);

        let candidates = collect_source(Box::new(Executables::from_path(
            bin.to_str().expect("path should be utf-8"),
        )))
        .await;

        assert_eq!(
            candidates,
            vec![
                expected_executable("a-command"),
                expected_executable("z-command"),
            ]
        );
    }

    #[tokio::test]
    async fn async_path_source_streams_multiple_path_dirs() {
        let first = temp_source_dir("path-source-async-first");
        let second = temp_source_dir("path-source-async-second");
        write_file(first.join("first-command"), 0o755);
        write_file(second.join("second-command"), 0o755);
        let (sender, mut receiver) = tokio::sync::mpsc::channel(8);

        let task = Box::new(Executables::from_path(&path_string([&first, &second])))
            .stream_candidates(sender);
        let first_batch = receiver
            .recv()
            .await
            .expect("path source should emit first dir batch");
        let second_batch = receiver
            .recv()
            .await
            .expect("path source should emit second dir batch");

        assert_eq!(first_batch, vec![expected_executable("first-command")]);
        assert_eq!(second_batch, vec![expected_executable("second-command")]);

        task.await.expect("path source task should finish");
    }

    #[tokio::test]
    async fn executable_source_candidates_feed_into_launcher_state() {
        let bin = temp_source_dir("path-source-launcher-state");
        write_file(bin.join("fzlaunch-run-me"), 0o755);
        let mut state = LauncherState::default();

        state.feed(
            collect_source(Box::new(Executables::from_path(
                bin.to_str().expect("path should be utf-8"),
            )))
            .await,
        );
        state.update_input(Value::raw(";cfzrun"));

        assert_eq!(
            state.press_enter(),
            Some(Value::raw("fzlaunch-run-me").into())
        );
    }
}
