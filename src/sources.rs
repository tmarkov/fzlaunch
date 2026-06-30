use std::collections::BTreeSet;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use crate::model::{Candidate, Value};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

pub type CandidateSender = mpsc::Sender<Vec<Candidate>>;
pub type CandidateReceiver = mpsc::Receiver<Vec<Candidate>>;

pub trait Source {
    fn candidates(&self) -> Vec<Candidate>;
}

pub trait AsyncSource: Send + 'static {
    fn stream_candidates(self: Box<Self>, sender: CandidateSender) -> JoinHandle<()>;
}

pub struct PathExecutables<'a> {
    pub path: &'a str,
}

pub struct FilesystemRoot {
    pub root: PathBuf,
}

pub fn collect_candidates(sources: &[&dyn Source]) -> Vec<Candidate> {
    sources
        .iter()
        .flat_map(|source| source.candidates())
        .collect()
}

pub fn executables_from_path(path: &str) -> Vec<Candidate> {
    PathExecutables { path }.candidates()
}

pub fn filesystem_entries(root: &Path) -> Vec<Candidate> {
    FilesystemRoot {
        root: root.to_path_buf(),
    }
    .candidates()
}

impl Source for PathExecutables<'_> {
    fn candidates(&self) -> Vec<Candidate> {
        let mut commands = BTreeSet::new();

        for dir in std::env::split_paths(self.path) {
            let Ok(entries) = fs::read_dir(dir) else {
                continue;
            };

            for entry in entries.flatten() {
                let Ok(metadata) = entry.metadata() else {
                    continue;
                };

                if !metadata.is_file() || metadata.permissions().mode() & 0o111 == 0 {
                    continue;
                }

                let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                    continue;
                };

                commands.insert(name);
            }
        }

        commands
            .into_iter()
            .map(|command| Candidate::new(Value::raw(command), 'c', Some(Value::raw("{}"))))
            .collect()
    }
}

impl Source for FilesystemRoot {
    fn candidates(&self) -> Vec<Candidate> {
        let mut paths = BTreeSet::new();
        let mut pending = vec![self.root.clone()];

        while let Some(dir) = pending.pop() {
            paths.extend(filesystem_paths_in_dir(dir, &mut pending));
        }

        paths.into_iter().map(filesystem_candidate).collect()
    }
}

impl AsyncSource for FilesystemRoot {
    fn stream_candidates(self: Box<Self>, sender: CandidateSender) -> JoinHandle<()> {
        tokio::task::spawn_blocking(move || {
            let mut pending = vec![self.root];

            while let Some(dir) = pending.pop() {
                let candidates = filesystem_paths_in_dir(dir, &mut pending)
                    .into_iter()
                    .map(filesystem_candidate)
                    .collect::<Vec<_>>();
                if !candidates.is_empty() && sender.blocking_send(candidates).is_err() {
                    break;
                }
            }
        })
    }
}

fn filesystem_paths_in_dir(dir: PathBuf, pending: &mut Vec<PathBuf>) -> BTreeSet<(String, char)> {
    let mut paths = BTreeSet::new();
    let Ok(entries) = fs::read_dir(dir) else {
        return paths;
    };

    for entry in entries.flatten() {
        let Ok(metadata) = entry.metadata() else {
            continue;
        };

        let match_char = if metadata.is_file() {
            'f'
        } else if metadata.is_dir() {
            pending.push(entry.path());
            'd'
        } else {
            continue;
        };

        let Some(path) = entry.path().to_str().map(str::to_owned) else {
            continue;
        };

        paths.insert((path, match_char));
    }

    paths
}

fn filesystem_candidate((path, match_char): (String, char)) -> Candidate {
    Candidate::new(
        Value::escaped(path),
        match_char,
        Some(Value::raw("xdg-open {}")),
    )
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    use crate::model::{Candidate, Value};
    use crate::sources::AsyncSource;
    use crate::state::LauncherState;

    fn temp_source_dir(name: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time should be after unix epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!("fzlaunch-{name}-{unique}"));
        fs::create_dir(&path).expect("temp source dir should be created");
        path
    }

    fn write_file(path: PathBuf, mode: u32) {
        fs::write(&path, b"#!/bin/sh\n").expect("test executable should be written");
        fs::set_permissions(&path, fs::Permissions::from_mode(mode))
            .expect("test executable permissions should be set");
    }

    fn path_string(dirs: &[PathBuf]) -> String {
        std::env::join_paths(dirs)
            .expect("test paths should join")
            .to_str()
            .expect("test path should be utf-8")
            .to_string()
    }

    struct StaticSource {
        candidates: Vec<Candidate>,
    }

    impl super::Source for StaticSource {
        fn candidates(&self) -> Vec<Candidate> {
            self.candidates.clone()
        }
    }

    #[test]
    fn collect_candidates_combines_multiple_sources() {
        let commands = StaticSource {
            candidates: vec![Candidate::new(
                Value::raw("firefox"),
                'c',
                Some(Value::raw("{}")),
            )],
        };
        let files = StaticSource {
            candidates: vec![Candidate::new(
                Value::escaped("/home/me/paper.pdf"),
                'f',
                Some(Value::raw("xdg-open {}")),
            )],
        };

        let candidates = super::collect_candidates(&[&commands, &files]);

        assert_eq!(
            candidates,
            vec![
                Candidate::new(Value::raw("firefox"), 'c', Some(Value::raw("{}"))),
                Candidate::new(
                    Value::escaped("/home/me/paper.pdf"),
                    'f',
                    Some(Value::raw("xdg-open {}"))
                ),
            ]
        );
    }

    #[test]
    fn path_source_returns_executables_as_raw_command_candidates() {
        let bin = temp_source_dir("path-source-executable");
        write_file(bin.join("fzlaunch-test-command"), 0o755);

        let candidates = super::executables_from_path(bin.to_str().expect("path should be utf-8"));

        assert_eq!(
            candidates,
            vec![Candidate::new(
                Value::raw("fzlaunch-test-command"),
                'c',
                Some(Value::raw("{}"))
            )]
        );
    }

    #[test]
    fn path_source_ignores_non_executable_files() {
        let bin = temp_source_dir("path-source-non-executable");
        write_file(bin.join("not-executable"), 0o644);

        let candidates = super::executables_from_path(bin.to_str().expect("path should be utf-8"));

        assert_eq!(candidates, Vec::<Candidate>::new());
    }

    #[test]
    fn path_source_deduplicates_commands_from_multiple_path_entries() {
        let first = temp_source_dir("path-source-first");
        let second = temp_source_dir("path-source-second");
        write_file(first.join("shared-command"), 0o755);
        write_file(second.join("shared-command"), 0o755);

        let candidates = super::executables_from_path(&path_string(&[first, second]));

        assert_eq!(
            candidates,
            vec![Candidate::new(
                Value::raw("shared-command"),
                'c',
                Some(Value::raw("{}"))
            )]
        );
    }

    #[test]
    fn path_source_ignores_missing_path_entries() {
        let missing = temp_source_dir("path-source-missing").join("missing");
        let bin = temp_source_dir("path-source-existing");
        write_file(bin.join("existing-command"), 0o755);

        let candidates = super::executables_from_path(&path_string(&[missing, bin]));

        assert_eq!(
            candidates,
            vec![Candidate::new(
                Value::raw("existing-command"),
                'c',
                Some(Value::raw("{}"))
            )]
        );
    }

    #[test]
    fn path_source_returns_commands_in_sorted_order() {
        let bin = temp_source_dir("path-source-sorted");
        write_file(bin.join("z-command"), 0o755);
        write_file(bin.join("a-command"), 0o755);

        let candidates = super::executables_from_path(bin.to_str().expect("path should be utf-8"));

        assert_eq!(
            candidates,
            vec![
                Candidate::new(Value::raw("a-command"), 'c', Some(Value::raw("{}"))),
                Candidate::new(Value::raw("z-command"), 'c', Some(Value::raw("{}"))),
            ]
        );
    }

    #[test]
    fn executable_source_candidates_feed_into_launcher_state() {
        let bin = temp_source_dir("path-source-launcher-state");
        write_file(bin.join("fzlaunch-run-me"), 0o755);
        let mut state = LauncherState::default();

        state.feed(super::executables_from_path(
            bin.to_str().expect("path should be utf-8"),
        ));
        state.update_input(Value::raw(";cfzrun"));

        assert_eq!(state.press_enter(), Some(Value::raw("fzlaunch-run-me")));
    }

    #[test]
    fn collected_sources_compose_nested_command_from_file_and_executables() {
        let bin = temp_source_dir("path-source-composition");
        write_file(bin.join("readlink"), 0o755);
        write_file(bin.join("nvim"), 0o755);
        let path = path_string(&[bin]);

        let root = temp_source_dir("filesystem-source-composition");
        let file = root.join("paper.pdf");
        fs::write(&file, b"pdf").expect("test file should be written");

        let commands = super::PathExecutables { path: &path };
        let files = super::FilesystemRoot { root };
        let mut state = LauncherState::default();

        state.feed(super::collect_candidates(&[&commands, &files]));

        state.update_input(Value::raw(";fpaper"));
        state.press_tab();

        state.update_input(Value::raw(";creadl"));
        state.press_tilde();
        state.update_input(Value::raw("readlink -f {}"));
        state.press_tab();

        state.update_input(Value::raw(";cnvim"));
        state.press_tilde();
        state.update_input(Value::raw("nvim $({})"));

        assert_eq!(
            state.press_enter(),
            Some(Value::raw(format!(
                "nvim $(readlink -f '{}')",
                file.to_str().expect("path should be utf-8")
            )))
        );
    }

    #[test]
    fn filesystem_source_returns_files_as_escaped_candidates() {
        let root = temp_source_dir("filesystem-source-file");
        let file = root.join("paper with spaces.pdf");
        fs::write(&file, b"pdf").expect("test file should be written");

        let candidates = super::filesystem_entries(&root);

        assert_eq!(
            candidates,
            vec![Candidate::new(
                Value::escaped(file.to_str().expect("path should be utf-8")),
                'f',
                Some(Value::raw("xdg-open {}"))
            )]
        );
    }

    #[tokio::test]
    async fn async_filesystem_source_emits_directory_batches_before_finishing() {
        let root = temp_source_dir("filesystem-source-async");
        let first = root.join("first.txt");
        let nested = root.join("nested");
        let second = nested.join("second.txt");
        fs::write(&first, b"first").expect("first test file should be written");
        fs::create_dir(&nested).expect("nested test directory should be created");
        fs::write(&second, b"second").expect("second test file should be written");
        let (sender, mut receiver) = tokio::sync::mpsc::channel(8);

        let task = Box::new(super::FilesystemRoot { root }).stream_candidates(sender);
        let first_batch = receiver
            .recv()
            .await
            .expect("filesystem source should emit first batch");

        assert_eq!(
            first_batch,
            vec![
                Candidate::new(
                    Value::escaped(first.to_str().expect("path should be utf-8")),
                    'f',
                    Some(Value::raw("xdg-open {}"))
                ),
                Candidate::new(
                    Value::escaped(nested.to_str().expect("path should be utf-8")),
                    'd',
                    Some(Value::raw("xdg-open {}"))
                ),
            ]
        );

        let remaining = receiver
            .recv()
            .await
            .expect("filesystem source should emit nested batch");
        assert_eq!(
            remaining,
            vec![Candidate::new(
                Value::escaped(second.to_str().expect("path should be utf-8")),
                'f',
                Some(Value::raw("xdg-open {}"))
            )]
        );

        task.await.expect("filesystem source task should finish");
    }

    #[test]
    fn filesystem_source_returns_directories_as_escaped_candidates() {
        let root = temp_source_dir("filesystem-source-directory");
        let dir = root.join("Documents");
        fs::create_dir(&dir).expect("test directory should be created");

        let candidates = super::filesystem_entries(&root);

        assert_eq!(
            candidates,
            vec![Candidate::new(
                Value::escaped(dir.to_str().expect("path should be utf-8")),
                'd',
                Some(Value::raw("xdg-open {}"))
            )]
        );
    }

    #[test]
    fn filesystem_source_returns_files_and_directories_in_sorted_order() {
        let root = temp_source_dir("filesystem-source-sorted");
        let file = root.join("z-file.txt");
        let dir = root.join("a-dir");
        fs::write(&file, b"text").expect("test file should be written");
        fs::create_dir(&dir).expect("test directory should be created");

        let candidates = super::filesystem_entries(&root);

        assert_eq!(
            candidates,
            vec![
                Candidate::new(
                    Value::escaped(dir.to_str().expect("path should be utf-8")),
                    'd',
                    Some(Value::raw("xdg-open {}"))
                ),
                Candidate::new(
                    Value::escaped(file.to_str().expect("path should be utf-8")),
                    'f',
                    Some(Value::raw("xdg-open {}"))
                ),
            ]
        );
    }

    #[test]
    fn filesystem_source_ignores_missing_roots() {
        let root = temp_source_dir("filesystem-source-missing").join("missing");

        let candidates = super::filesystem_entries(&root);

        assert_eq!(candidates, Vec::<Candidate>::new());
    }

    #[test]
    fn filesystem_file_candidates_feed_into_launcher_state() {
        let root = temp_source_dir("filesystem-source-file-launcher-state");
        let file = root.join("paper.pdf");
        fs::write(&file, b"pdf").expect("test file should be written");
        let mut state = LauncherState::default();

        state.feed(super::filesystem_entries(&root));
        state.update_input(Value::raw(";fpaper"));

        assert_eq!(
            state.press_enter(),
            Some(Value::raw(format!(
                "xdg-open '{}'",
                file.to_str().expect("path should be utf-8")
            )))
        );
    }

    #[test]
    fn filesystem_directory_candidates_feed_into_launcher_state() {
        let root = temp_source_dir("filesystem-source-directory-launcher-state");
        let dir = root.join("Documents");
        fs::create_dir(&dir).expect("test directory should be created");
        let mut state = LauncherState::default();

        state.feed(super::filesystem_entries(&root));
        state.update_input(Value::raw(";ddoc"));

        assert_eq!(
            state.press_enter(),
            Some(Value::raw(format!(
                "xdg-open '{}'",
                dir.to_str().expect("path should be utf-8")
            )))
        );
    }

    #[test]
    fn filesystem_source_recurses_into_nested_directories() {
        let root = temp_source_dir("filesystem-source-recursive");
        let nested = root.join("Documents").join("research");
        let file = nested.join("paper.pdf");
        fs::create_dir_all(&nested).expect("nested test directory should be created");
        fs::write(&file, b"pdf").expect("nested test file should be written");

        let candidates = super::filesystem_entries(&root);

        assert!(candidates.contains(&Candidate::new(
            Value::escaped(nested.to_str().expect("path should be utf-8")),
            'd',
            Some(Value::raw("xdg-open {}"))
        )));
        assert!(candidates.contains(&Candidate::new(
            Value::escaped(file.to_str().expect("path should be utf-8")),
            'f',
            Some(Value::raw("xdg-open {}"))
        )));
    }

    #[test]
    fn filesystem_source_has_no_depth_cutoff() {
        let root = temp_source_dir("filesystem-source-deep");
        let deep = root.join("a").join("b").join("c").join("d");
        let file = deep.join("deep.txt");
        fs::create_dir_all(&deep).expect("deep test directory should be created");
        fs::write(&file, b"text").expect("deep test file should be written");

        let candidates = super::filesystem_entries(&root);

        assert!(candidates.contains(&Candidate::new(
            Value::escaped(file.to_str().expect("path should be utf-8")),
            'f',
            Some(Value::raw("xdg-open {}"))
        )));
    }
}
