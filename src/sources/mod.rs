use crate::model::Candidate;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

mod calculator;
mod executables;
mod filesystem;

pub use calculator::Calculator;
pub use executables::Executables;
pub use filesystem::FilesystemRoot;

#[cfg(test)]
pub type PathExecutables = Executables;

pub type CandidateSender = mpsc::Sender<Vec<Candidate>>;
pub type CandidateReceiver = mpsc::Receiver<Vec<Candidate>>;

pub trait AsyncSource: Send + 'static {
    fn stream_candidates(self: Box<Self>, sender: CandidateSender) -> JoinHandle<()>;
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;

    use crate::model::{Candidate, Value};
    use crate::state::LauncherState;
    use crate::test_support::{path_string, TempDir};
    use tokio::task::JoinHandle;

    use super::{AsyncSource, CandidateSender};

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
                Box::new(super::FilesystemRoot::new(root.path().to_path_buf())),
            ])
            .await,
        );

        state.update_input(Value::raw(";fpaper"));
        state.press_tab();

        state.update_input(Value::raw(";creadl"));
        state.press_backtick();
        state.update_input(Value::raw("readlink -f {}"));
        state.press_tab();

        state.update_input(Value::raw(";cnvim"));
        state.press_backtick();
        state.update_input(Value::raw("nvim $({})"));

        assert_eq!(
            state.press_enter(),
            Some(
                Value::raw(format!(
                    "nvim $(readlink -f '{}')",
                    file.to_str().expect("path should be utf-8")
                ))
                .into()
            )
        );
    }
}
