use std::collections::{BTreeSet, VecDeque};
use std::fs;
use std::path::PathBuf;

use crate::config::FilesystemSourceConfig;
use crate::model::{Candidate, CandidateSource, Value};
use tokio::task::JoinHandle;

use super::{AsyncSource, CandidateSender};

pub struct FilesystemRoot {
    pub root: PathBuf,
    config: FilesystemSourceConfig,
}

impl FilesystemRoot {
    #[cfg(test)]
    pub fn new(root: PathBuf) -> Self {
        Self::new_with_config(root, FilesystemSourceConfig::default())
    }

    pub fn new_with_config(root: PathBuf, config: FilesystemSourceConfig) -> Self {
        Self { root, config }
    }

    fn stream_candidate_batches(&self, sender: CandidateSender) {
        let mut pending = VecDeque::from([self.root.clone()]);

        while let Some(dir) = pending.pop_front() {
            let entries = filesystem_paths_in_dir(dir);
            pending.extend(entries.iter().filter_map(filesystem_directory_path));
            let candidates = entries
                .into_iter()
                .map(|entry| filesystem_candidate(entry, &self.config))
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
    File,
}

fn filesystem_paths_in_dir(dir: PathBuf) -> BTreeSet<(String, FilesystemEntryKind)> {
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
            FilesystemEntryKind::File
        } else if file_type.is_dir() {
            FilesystemEntryKind::Directory
        } else {
            continue;
        };

        paths.insert((path_text, kind));
    }

    paths
}

fn filesystem_directory_path(entry: &(String, FilesystemEntryKind)) -> Option<PathBuf> {
    let (path, kind) = entry;
    (*kind == FilesystemEntryKind::Directory).then(|| PathBuf::from(path))
}

fn filesystem_candidate(
    entry: (String, FilesystemEntryKind),
    config: &FilesystemSourceConfig,
) -> Candidate {
    let (path, match_char) = match entry {
        (path, FilesystemEntryKind::Directory) => (path, 'd'),
        (path, FilesystemEntryKind::File) => (path, 'f'),
    };

    Candidate::new_with_action(
        Value::escaped(path),
        match_char,
        Some(config.direct_action.clone()),
    )
    .with_source(CandidateSource::FilesystemPath)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::symlink;
    use std::path::Path;

    use crate::config::FilesystemSourceConfig;
    use crate::model::{Action, Candidate, CandidateSource, ExecutionMode, ExecutionPlan, Value};
    use crate::sources::AsyncSource;
    use crate::state::LauncherState;
    use crate::test_support::TempDir;

    use super::FilesystemRoot;

    fn temp_source_dir(name: &str) -> TempDir {
        TempDir::new(name)
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

    fn expected_directory(path: &Path) -> Candidate {
        Candidate::new_with_action(
            Value::escaped(path.to_str().expect("path should be utf-8")),
            'd',
            Some(Action::detached(Value::raw("xdg-open {}"))),
        )
        .with_source(CandidateSource::FilesystemPath)
    }

    fn expected_text_file(path: &Path) -> Candidate {
        Candidate::new_with_action(
            Value::escaped(path.to_str().expect("path should be utf-8")),
            'f',
            Some(Action::detached(Value::raw("xdg-open {}"))),
        )
        .with_source(CandidateSource::FilesystemPath)
    }

    fn expected_binary_file(path: &Path) -> Candidate {
        Candidate::new_with_action(
            Value::escaped(path.to_str().expect("path should be utf-8")),
            'f',
            Some(Action::detached(Value::raw("xdg-open {}"))),
        )
        .with_source(CandidateSource::FilesystemPath)
    }

    fn filesystem_root(root: &Path) -> FilesystemRoot {
        FilesystemRoot::new(root.to_path_buf())
    }

    #[tokio::test]
    async fn filesystem_source_returns_files_as_escaped_candidates() {
        let root = temp_source_dir("filesystem-source-file");
        let file = root.join("paper with spaces.pdf");
        fs::write(&file, b"pdf").expect("test file should be written");

        let candidates = collect_source(Box::new(filesystem_root(root.path()))).await;

        assert_eq!(candidates, vec![expected_text_file(&file)]);
    }

    #[tokio::test]
    async fn filesystem_source_returns_binary_files_as_escaped_candidates() {
        let root = temp_source_dir("filesystem-source-binary-file");
        let file = root.join("binary-file");
        fs::write(&file, b"\0binary").expect("test binary file should be written");

        let candidates = collect_source(Box::new(filesystem_root(root.path()))).await;

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

        let task = Box::new(filesystem_root(root.path())).stream_candidates(sender);
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
    async fn async_filesystem_source_emits_batches_breadth_first() {
        let root = temp_source_dir("filesystem-source-bfs");
        let first_dir = root.join("a-first");
        let second_dir = root.join("b-second");
        let first_file = first_dir.join("first.txt");
        let second_file = second_dir.join("second.txt");
        fs::create_dir(&first_dir).expect("first directory should be created");
        fs::create_dir(&second_dir).expect("second directory should be created");
        fs::write(&first_file, b"first").expect("first file should be written");
        fs::write(&second_file, b"second").expect("second file should be written");
        let (sender, mut receiver) = tokio::sync::mpsc::channel(8);

        let task = Box::new(filesystem_root(root.path())).stream_candidates(sender);

        assert_eq!(
            receiver
                .recv()
                .await
                .expect("filesystem source should emit root batch"),
            vec![
                expected_directory(&first_dir),
                expected_directory(&second_dir)
            ]
        );
        assert_eq!(
            receiver
                .recv()
                .await
                .expect("filesystem source should emit first child batch"),
            vec![expected_text_file(&first_file)]
        );
        assert_eq!(
            receiver
                .recv()
                .await
                .expect("filesystem source should emit second child batch"),
            vec![expected_text_file(&second_file)]
        );

        task.await.expect("filesystem source task should finish");
    }

    #[tokio::test]
    async fn filesystem_source_returns_directories_as_escaped_candidates() {
        let root = temp_source_dir("filesystem-source-directory");
        let dir = root.join("Documents");
        fs::create_dir(&dir).expect("test directory should be created");

        let candidates = collect_source(Box::new(filesystem_root(root.path()))).await;

        assert_eq!(candidates, vec![expected_directory(&dir)]);
    }

    #[tokio::test]
    async fn filesystem_source_returns_files_and_directories_in_sorted_order() {
        let root = temp_source_dir("filesystem-source-sorted");
        let file = root.join("z-file.txt");
        let dir = root.join("a-dir");
        fs::write(&file, b"text").expect("test file should be written");
        fs::create_dir(&dir).expect("test directory should be created");

        let candidates = collect_source(Box::new(filesystem_root(root.path()))).await;

        assert_eq!(
            candidates,
            vec![expected_directory(&dir), expected_text_file(&file)]
        );
    }

    #[tokio::test]
    async fn filesystem_source_ignores_missing_roots() {
        let missing_root = temp_source_dir("filesystem-source-missing");
        let root = missing_root.join("missing");

        let candidates = collect_source(Box::new(FilesystemRoot::new(root))).await;

        assert_eq!(candidates, Vec::<Candidate>::new());
    }

    #[tokio::test]
    async fn filesystem_file_candidates_feed_into_launcher_state() {
        let root = temp_source_dir("filesystem-source-file-launcher-state");
        let file = root.join("paper.pdf");
        fs::write(&file, b"pdf").expect("test file should be written");
        let mut state = LauncherState::default();

        state.feed(collect_source(Box::new(filesystem_root(root.path()))).await);
        state.update_input(Value::raw(";fpaper"));

        assert_eq!(
            state.press_enter(),
            Some(ExecutionPlan::new(
                Value::raw(format!(
                    "xdg-open '{}'",
                    file.to_str().expect("path should be utf-8")
                )),
                ExecutionMode::Detached,
            ))
        );
    }

    #[tokio::test]
    async fn filesystem_directory_candidates_feed_into_launcher_state() {
        let root = temp_source_dir("filesystem-source-directory-launcher-state");
        let dir = root.join("Documents");
        fs::create_dir(&dir).expect("test directory should be created");
        let mut state = LauncherState::default();

        state.feed(collect_source(Box::new(filesystem_root(root.path()))).await);
        state.update_input(Value::raw(";ddoc"));

        assert_eq!(
            state.press_enter(),
            Some(ExecutionPlan::new(
                Value::raw(format!(
                    "xdg-open '{}'",
                    dir.to_str().expect("path should be utf-8")
                )),
                ExecutionMode::Detached,
            ))
        );
    }

    #[tokio::test]
    async fn filesystem_source_recurses_into_nested_directories() {
        let root = temp_source_dir("filesystem-source-recursive");
        let nested = root.join("Documents").join("research");
        let file = nested.join("paper.pdf");
        fs::create_dir_all(&nested).expect("nested test directory should be created");
        fs::write(&file, b"pdf").expect("nested test file should be written");

        let candidates = collect_source(Box::new(filesystem_root(root.path()))).await;

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

        let candidates = collect_source(Box::new(filesystem_root(root.path()))).await;

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

        let candidates = collect_source(Box::new(filesystem_root(root.path()))).await;

        assert!(candidates.contains(&expected_text_file(&file)));
    }

    #[tokio::test]
    async fn filesystem_source_uses_configured_actions() {
        let root = temp_source_dir("filesystem-source-configured-actions");
        let dir = root.join("Documents");
        let file = root.join("paper.txt");
        fs::create_dir(&dir).expect("test directory should be created");
        fs::write(&file, b"text").expect("test file should be written");

        let candidates = collect_source(Box::new(FilesystemRoot::new_with_config(
            root.path().to_path_buf(),
            FilesystemSourceConfig {
                direct_action: Action::foreground(Value::raw("open-path {}")),
                ..FilesystemSourceConfig::default()
            },
        )))
        .await;

        assert_eq!(
            candidates,
            vec![
                Candidate::new(
                    Value::escaped(dir.to_str().expect("path should be utf-8")),
                    'd',
                    Some(Value::raw("open-path {}"))
                )
                .with_source(CandidateSource::FilesystemPath),
                Candidate::new(
                    Value::escaped(file.to_str().expect("path should be utf-8")),
                    'f',
                    Some(Value::raw("open-path {}"))
                )
                .with_source(CandidateSource::FilesystemPath),
            ]
        );
    }
}
