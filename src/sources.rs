use std::collections::BTreeSet;
use std::fs;
use std::io::Read;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use crate::model::{Candidate, Value};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

pub type CandidateSender = mpsc::Sender<Vec<Candidate>>;
pub type CandidateReceiver = mpsc::Receiver<Vec<Candidate>>;

pub trait AsyncSource: Send + 'static {
    fn stream_candidates(self: Box<Self>, sender: CandidateSender) -> JoinHandle<()>;
}

pub struct PathExecutables {
    pub dirs: Vec<PathBuf>,
}

pub struct FilesystemRoot {
    pub root: PathBuf,
}

impl PathExecutables {
    pub fn from_path(path: &str) -> Self {
        let dirs = if path.is_empty() {
            Vec::new()
        } else {
            std::env::split_paths(path).collect()
        };

        Self { dirs }
    }

    fn stream_candidate_batches(&self, sender: CandidateSender) {
        let mut seen = BTreeSet::new();

        for dir in &self.dirs {
            let candidates = executable_commands_in_dir(dir)
                .into_iter()
                .filter(|command| seen.insert(command.clone()))
                .map(executable_candidate)
                .collect::<Vec<_>>();

            if !candidates.is_empty() && sender.blocking_send(candidates).is_err() {
                break;
            }
        }
    }
}

impl AsyncSource for PathExecutables {
    fn stream_candidates(self: Box<Self>, sender: CandidateSender) -> JoinHandle<()> {
        tokio::task::spawn_blocking(move || {
            self.stream_candidate_batches(sender);
        })
    }
}

impl FilesystemRoot {
    fn stream_candidate_batches(&self, sender: CandidateSender) {
        let mut pending = vec![self.root.clone()];

        while let Some(dir) = pending.pop() {
            let candidates = filesystem_paths_in_dir(dir, &mut pending)
                .into_iter()
                .map(filesystem_candidate)
                .collect::<Vec<_>>();
            if !candidates.is_empty() && sender.blocking_send(candidates).is_err() {
                break;
            }
        }
    }
}

impl AsyncSource for FilesystemRoot {
    fn stream_candidates(self: Box<Self>, sender: CandidateSender) -> JoinHandle<()> {
        tokio::task::spawn_blocking(move || {
            self.stream_candidate_batches(sender);
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum FilesystemEntryKind {
    Directory,
    TextFile,
    BinaryFile,
}

fn filesystem_paths_in_dir(
    dir: PathBuf,
    pending: &mut Vec<PathBuf>,
) -> BTreeSet<(String, FilesystemEntryKind)> {
    let mut paths = BTreeSet::new();
    let Ok(entries) = fs::read_dir(dir) else {
        return paths;
    };

    for entry in entries.flatten() {
        let Ok(file_type) = entry.file_type() else {
            continue;
        };

        let path = entry.path();
        let Some(path_text) = path.to_str().map(str::to_owned) else {
            continue;
        };

        let kind = if file_type.is_file() {
            if is_text_file(&path) {
                FilesystemEntryKind::TextFile
            } else {
                FilesystemEntryKind::BinaryFile
            }
        } else if file_type.is_dir() {
            pending.push(path);
            FilesystemEntryKind::Directory
        } else {
            continue;
        };

        paths.insert((path_text, kind));
    }

    paths
}

fn is_text_file(path: &Path) -> bool {
    let Ok(mut file) = fs::File::open(path) else {
        return false;
    };
    let mut buffer = [0; 1024];
    let Ok(bytes_read) = file.read(&mut buffer) else {
        return false;
    };
    let bytes = &buffer[..bytes_read];

    !bytes.contains(&0) && std::str::from_utf8(bytes).is_ok()
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

fn is_executable_file(path: &Path) -> bool {
    let Ok(metadata) = fs::metadata(path) else {
        return false;
    };

    metadata.is_file() && metadata.permissions().mode() & 0o111 != 0
}

fn executable_candidate(command: String) -> Candidate {
    Candidate::new(Value::raw(command), 'c', Some(Value::raw("{}")))
        .with_preview_command(Some(Value::raw("man {}")))
}

fn filesystem_candidate(entry: (String, FilesystemEntryKind)) -> Candidate {
    let (path, match_char, preview_command) = match entry {
        (path, FilesystemEntryKind::Directory) => (path, 'd', Some(Value::raw("ls {}"))),
        (path, FilesystemEntryKind::TextFile) => (path, 'f', Some(Value::raw("cat {}"))),
        (path, FilesystemEntryKind::BinaryFile) => (path, 'f', None),
    };

    Candidate::new(
        Value::escaped(path),
        match_char,
        Some(Value::raw("xdg-open {}")),
    )
    .with_preview_command(preview_command)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::{symlink, PermissionsExt};
    use std::path::{Path, PathBuf};

    use crate::model::{Candidate, Value};
    use crate::sources::{AsyncSource, CandidateSender};
    use crate::state::LauncherState;
    use crate::test_support::{path_string, TempDir};
    use tokio::task::JoinHandle;

    fn temp_source_dir(name: &str) -> TempDir {
        TempDir::new(name)
    }

    fn write_file(path: PathBuf, mode: u32) {
        fs::write(&path, b"#!/bin/sh\n").expect("test executable should be written");
        fs::set_permissions(&path, fs::Permissions::from_mode(mode))
            .expect("test executable permissions should be set");
    }

    struct StaticSource {
        candidates: Vec<Candidate>,
    }

    impl AsyncSource for StaticSource {
        fn stream_candidates(self: Box<Self>, sender: CandidateSender) -> JoinHandle<()> {
            tokio::spawn(async move {
                let _ = sender.send(self.candidates).await;
            })
        }
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

    async fn collect_sources(sources: Vec<Box<dyn AsyncSource>>) -> Vec<Candidate> {
        let mut candidates = Vec::new();

        for source in sources {
            candidates.extend(collect_source(source).await);
        }

        candidates
    }

    fn expected_executable(command: &str) -> Candidate {
        Candidate::new(Value::raw(command), 'c', Some(Value::raw("{}")))
            .with_preview_command(Some(Value::raw("man {}")))
    }

    fn expected_directory(path: &Path) -> Candidate {
        Candidate::new(
            Value::escaped(path.to_str().expect("path should be utf-8")),
            'd',
            Some(Value::raw("xdg-open {}")),
        )
        .with_preview_command(Some(Value::raw("ls {}")))
    }

    fn expected_text_file(path: &Path) -> Candidate {
        Candidate::new(
            Value::escaped(path.to_str().expect("path should be utf-8")),
            'f',
            Some(Value::raw("xdg-open {}")),
        )
        .with_preview_command(Some(Value::raw("cat {}")))
    }

    fn expected_binary_file(path: &Path) -> Candidate {
        Candidate::new(
            Value::escaped(path.to_str().expect("path should be utf-8")),
            'f',
            Some(Value::raw("xdg-open {}")),
        )
    }

    #[tokio::test]
    async fn collect_sources_combines_multiple_sources() {
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

        let candidates = collect_sources(vec![Box::new(commands), Box::new(files)]).await;

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

    #[tokio::test]
    async fn path_source_returns_executables_as_raw_command_candidates() {
        let bin = temp_source_dir("path-source-executable");
        write_file(bin.join("fzlaunch-test-command"), 0o755);

        let candidates = collect_source(Box::new(super::PathExecutables::from_path(
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

        let candidates = collect_source(Box::new(super::PathExecutables::from_path(
            bin.to_str().expect("path should be utf-8"),
        )))
        .await;

        assert_eq!(candidates, vec![expected_executable("fzlaunch-test-link")]);
    }

    #[tokio::test]
    async fn path_source_ignores_non_executable_files() {
        let bin = temp_source_dir("path-source-non-executable");
        write_file(bin.join("not-executable"), 0o644);

        let candidates = collect_source(Box::new(super::PathExecutables::from_path(
            bin.to_str().expect("path should be utf-8"),
        )))
        .await;

        assert_eq!(candidates, Vec::<Candidate>::new());
    }

    #[tokio::test]
    async fn path_source_deduplicates_commands_from_multiple_path_entries() {
        let first = temp_source_dir("path-source-first");
        let second = temp_source_dir("path-source-second");
        write_file(first.join("shared-command"), 0o755);
        write_file(second.join("shared-command"), 0o755);

        let candidates =
            collect_source(Box::new(super::PathExecutables::from_path(&path_string([
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

        let candidates =
            collect_source(Box::new(super::PathExecutables::from_path(&path_string([
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

        let candidates = collect_source(Box::new(super::PathExecutables::from_path(
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

        let task = Box::new(super::PathExecutables::from_path(&path_string([
            &first, &second,
        ])))
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
            collect_source(Box::new(super::PathExecutables::from_path(
                bin.to_str().expect("path should be utf-8"),
            )))
            .await,
        );
        state.update_input(Value::raw(";cfzrun"));

        assert_eq!(state.press_enter(), Some(Value::raw("fzlaunch-run-me")));
    }

    #[tokio::test]
    async fn collected_sources_compose_nested_command_from_file_and_executables() {
        let bin = temp_source_dir("path-source-composition");
        write_file(bin.join("readlink"), 0o755);
        write_file(bin.join("nvim"), 0o755);
        let path = path_string([&bin]);

        let root = temp_source_dir("filesystem-source-composition");
        let file = root.join("paper.pdf");
        fs::write(&file, b"pdf").expect("test file should be written");

        let mut state = LauncherState::default();

        state.feed(
            collect_sources(vec![
                Box::new(super::PathExecutables::from_path(&path)),
                Box::new(super::FilesystemRoot {
                    root: root.path().to_path_buf(),
                }),
            ])
            .await,
        );

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

    #[tokio::test]
    async fn filesystem_source_returns_files_as_escaped_candidates() {
        let root = temp_source_dir("filesystem-source-file");
        let file = root.join("paper with spaces.pdf");
        fs::write(&file, b"pdf").expect("test file should be written");

        let candidates = collect_source(Box::new(super::FilesystemRoot {
            root: root.path().to_path_buf(),
        }))
        .await;

        assert_eq!(candidates, vec![expected_text_file(&file)]);
    }

    #[tokio::test]
    async fn filesystem_source_does_not_preview_binary_files() {
        let root = temp_source_dir("filesystem-source-binary-file");
        let file = root.join("binary-file");
        fs::write(&file, b"\0binary").expect("test binary file should be written");

        let candidates = collect_source(Box::new(super::FilesystemRoot {
            root: root.path().to_path_buf(),
        }))
        .await;

        assert_eq!(candidates, vec![expected_binary_file(&file)]);
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

        let task = Box::new(super::FilesystemRoot {
            root: root.path().to_path_buf(),
        })
        .stream_candidates(sender);
        let first_batch = receiver
            .recv()
            .await
            .expect("filesystem source should emit first batch");

        assert_eq!(
            first_batch,
            vec![expected_text_file(&first), expected_directory(&nested)]
        );

        let remaining = receiver
            .recv()
            .await
            .expect("filesystem source should emit nested batch");
        assert_eq!(remaining, vec![expected_text_file(&second)]);

        task.await.expect("filesystem source task should finish");
    }

    #[tokio::test]
    async fn filesystem_source_returns_directories_as_escaped_candidates() {
        let root = temp_source_dir("filesystem-source-directory");
        let dir = root.join("Documents");
        fs::create_dir(&dir).expect("test directory should be created");

        let candidates = collect_source(Box::new(super::FilesystemRoot {
            root: root.path().to_path_buf(),
        }))
        .await;

        assert_eq!(candidates, vec![expected_directory(&dir)]);
    }

    #[tokio::test]
    async fn filesystem_source_returns_files_and_directories_in_sorted_order() {
        let root = temp_source_dir("filesystem-source-sorted");
        let file = root.join("z-file.txt");
        let dir = root.join("a-dir");
        fs::write(&file, b"text").expect("test file should be written");
        fs::create_dir(&dir).expect("test directory should be created");

        let candidates = collect_source(Box::new(super::FilesystemRoot {
            root: root.path().to_path_buf(),
        }))
        .await;

        assert_eq!(
            candidates,
            vec![expected_directory(&dir), expected_text_file(&file)]
        );
    }

    #[tokio::test]
    async fn filesystem_source_ignores_missing_roots() {
        let missing_root = temp_source_dir("filesystem-source-missing");
        let root = missing_root.join("missing");

        let candidates = collect_source(Box::new(super::FilesystemRoot { root })).await;

        assert_eq!(candidates, Vec::<Candidate>::new());
    }

    #[tokio::test]
    async fn filesystem_file_candidates_feed_into_launcher_state() {
        let root = temp_source_dir("filesystem-source-file-launcher-state");
        let file = root.join("paper.pdf");
        fs::write(&file, b"pdf").expect("test file should be written");
        let mut state = LauncherState::default();

        state.feed(
            collect_source(Box::new(super::FilesystemRoot {
                root: root.path().to_path_buf(),
            }))
            .await,
        );
        state.update_input(Value::raw(";fpaper"));

        assert_eq!(
            state.press_enter(),
            Some(Value::raw(format!(
                "xdg-open '{}'",
                file.to_str().expect("path should be utf-8")
            )))
        );
    }

    #[tokio::test]
    async fn filesystem_directory_candidates_feed_into_launcher_state() {
        let root = temp_source_dir("filesystem-source-directory-launcher-state");
        let dir = root.join("Documents");
        fs::create_dir(&dir).expect("test directory should be created");
        let mut state = LauncherState::default();

        state.feed(
            collect_source(Box::new(super::FilesystemRoot {
                root: root.path().to_path_buf(),
            }))
            .await,
        );
        state.update_input(Value::raw(";ddoc"));

        assert_eq!(
            state.press_enter(),
            Some(Value::raw(format!(
                "xdg-open '{}'",
                dir.to_str().expect("path should be utf-8")
            )))
        );
    }

    #[tokio::test]
    async fn filesystem_source_recurses_into_nested_directories() {
        let root = temp_source_dir("filesystem-source-recursive");
        let nested = root.join("Documents").join("research");
        let file = nested.join("paper.pdf");
        fs::create_dir_all(&nested).expect("nested test directory should be created");
        fs::write(&file, b"pdf").expect("nested test file should be written");

        let candidates = collect_source(Box::new(super::FilesystemRoot {
            root: root.path().to_path_buf(),
        }))
        .await;

        assert!(candidates.contains(&expected_directory(&nested)));
        assert!(candidates.contains(&expected_text_file(&file)));
    }

    #[tokio::test]
    async fn filesystem_source_does_not_recurse_into_symlinked_directories() {
        let root = temp_source_dir("filesystem-source-symlink-loop");
        let nested = root.join("nested");
        let file = nested.join("paper.pdf");
        let loop_link = root.join("loop");
        fs::create_dir(&nested).expect("nested test directory should be created");
        fs::write(&file, b"pdf").expect("nested test file should be written");
        symlink(&root, &loop_link).expect("symlink loop should be created");

        let candidates = collect_source(Box::new(super::FilesystemRoot {
            root: root.path().to_path_buf(),
        }))
        .await;

        assert!(candidates.contains(&expected_directory(&nested)));
        assert!(candidates.contains(&expected_text_file(&file)));
        assert!(!candidates.contains(&expected_directory(&loop_link)));
    }

    #[tokio::test]
    async fn filesystem_source_has_no_depth_cutoff() {
        let root = temp_source_dir("filesystem-source-deep");
        let deep = root.join("a").join("b").join("c").join("d");
        let file = deep.join("deep.txt");
        fs::create_dir_all(&deep).expect("deep test directory should be created");
        fs::write(&file, b"text").expect("deep test file should be written");

        let candidates = collect_source(Box::new(super::FilesystemRoot {
            root: root.path().to_path_buf(),
        }))
        .await;

        assert!(candidates.contains(&expected_text_file(&file)));
    }
}
