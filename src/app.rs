use std::path::PathBuf;

use crate::model::{Candidate, Value};
use crate::shell;
use crate::sources::{AsyncSource, CandidateReceiver, FilesystemRoot, PathExecutables};
use crate::state::{InputMode, LauncherState};
use crate::ui::tui;
use tokio::sync::mpsc::error::TryRecvError;

const CANDIDATE_CHANNEL_CAPACITY: usize = 128;

pub struct Governor {
    state: LauncherState,
    candidate_receiver: CandidateReceiver,
    source_tasks: Vec<tokio::task::JoinHandle<()>>,
}

pub fn run() {
    if let Err(error) = run_inner() {
        eprintln!("fzlaunch: {error}");
    }
}

fn run_inner() -> Result<(), Box<dyn std::error::Error>> {
    let cwd = std::env::current_dir()?;
    let path = std::env::var("PATH").unwrap_or_default();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()?;

    if let Some(command) = runtime.block_on(async {
        let mut governor = Governor::start(cwd, &path);
        tui::run(&mut governor).await
    })? {
        println!("{}", shell::render_value(&command));
    }

    Ok(())
}

impl Governor {
    pub fn start(cwd: PathBuf, path: &str) -> Self {
        Self::with_sources([
            Box::new(FilesystemRoot { root: cwd }) as Box<dyn AsyncSource>,
            Box::new(PathExecutables::from_path(path)) as Box<dyn AsyncSource>,
        ])
    }

    pub fn with_sources(sources: impl IntoIterator<Item = Box<dyn AsyncSource>>) -> Self {
        let (sender, candidate_receiver) = tokio::sync::mpsc::channel(CANDIDATE_CHANNEL_CAPACITY);
        let source_tasks = sources
            .into_iter()
            .map(|source| source.stream_candidates(sender.clone()))
            .collect();

        drop(sender);

        Self {
            state: LauncherState::default(),
            candidate_receiver,
            source_tasks,
        }
    }

    pub fn update_input(&mut self, value: Value) {
        self.state.update_input(value);
    }

    pub fn feed(&mut self, candidates: impl IntoIterator<Item = Candidate>) {
        self.state.feed(candidates);
    }

    pub fn select_next(&mut self) {
        self.state.select_next();
    }

    pub fn select_previous(&mut self) {
        self.state.select_previous();
    }

    pub fn press_tilde(&mut self) {
        self.state.press_tilde();
    }

    pub fn press_tab(&mut self) {
        self.state.press_tab();
    }

    pub fn press_enter(&mut self) -> Option<Value> {
        self.state.press_enter()
    }

    pub fn queue_status(&self) -> Option<String> {
        self.state.queue_status()
    }

    pub fn mode(&self) -> InputMode {
        self.state.mode()
    }

    pub fn value(&self) -> Value {
        self.state.value()
    }

    pub fn current(&self) -> Value {
        self.state.current()
    }

    pub fn selected(&self) -> Option<Value> {
        self.state.selected()
    }

    pub fn results(&self) -> Vec<String> {
        self.state.results()
    }

    pub fn selected_index(&self) -> Option<usize> {
        self.state.selected_index()
    }

    pub async fn receive_candidates(&mut self) -> bool {
        let Some(candidates) = self.candidate_receiver.recv().await else {
            return false;
        };

        self.state.feed(candidates);
        true
    }

    pub fn receive_pending_candidates(&mut self) -> usize {
        let mut batches = 0;

        loop {
            match self.candidate_receiver.try_recv() {
                Ok(candidates) => {
                    self.state.feed(candidates);
                    batches += 1;
                }
                Err(TryRecvError::Empty) => return batches,
                Err(TryRecvError::Disconnected) => return batches,
            }
        }
    }
}

impl Drop for Governor {
    fn drop(&mut self) {
        for task in &self.source_tasks {
            task.abort();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;
    use std::time::Duration;
    use std::time::{SystemTime, UNIX_EPOCH};

    use tokio::task::JoinHandle;
    use tokio::time;

    use super::*;
    use crate::sources::CandidateSender;

    struct MockSource {
        interval: Duration,
    }

    impl MockSource {
        fn new(interval: Duration) -> Self {
            Self { interval }
        }
    }

    impl AsyncSource for MockSource {
        fn stream_candidates(self: Box<Self>, sender: CandidateSender) -> JoinHandle<()> {
            tokio::spawn(async move {
                let mut counter = 0;

                loop {
                    time::sleep(self.interval).await;

                    let candidate = Candidate::new(Value::raw(format!("{counter} 00")), 'm', None);
                    if sender.send(vec![candidate]).await.is_err() {
                        break;
                    }

                    counter += 1;
                }
            })
        }
    }

    async fn receive_next_candidate(governor: &mut Governor) {
        time::advance(Duration::from_millis(100)).await;
        assert!(governor.receive_candidates().await);
    }

    fn temp_app_dir(name: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time should be after unix epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!("fzlaunch-{name}-{unique}"));
        fs::create_dir(&path).expect("temp app dir should be created");
        path
    }

    fn path_string(dirs: &[PathBuf]) -> String {
        std::env::join_paths(dirs)
            .expect("test paths should join")
            .to_str()
            .expect("test path should be utf-8")
            .to_string()
    }

    async fn receive_until_selected(governor: &mut Governor) -> Value {
        while governor.selected().is_none() {
            assert!(governor.receive_candidates().await);
        }

        governor
            .selected()
            .expect("governor should have selected value")
    }

    #[test]
    fn governor_forwards_launcher_state_operations() {
        let mut governor = Governor::with_sources([]);

        governor.feed([
            Candidate::new(Value::escaped("/home/me/paper.pdf"), 'f', None),
            Candidate::new(Value::raw("nvim"), 'c', None),
        ]);
        governor.update_input(Value::raw(";fpaper"));

        assert_eq!(
            governor.selected(),
            Some(Value::escaped("/home/me/paper.pdf"))
        );
        assert_eq!(governor.current(), Value::escaped("/home/me/paper.pdf"));

        governor.press_tab();
        assert_eq!(governor.queue_status(), Some("'/home/me/paper.pdf'".into()));

        governor.update_input(Value::raw(";cnvim"));
        assert_eq!(governor.selected(), Some(Value::raw("nvim")));

        governor.press_tilde();
        assert_eq!(governor.mode(), InputMode::Edit);
        assert_eq!(governor.value(), Value::raw("nvim"));

        governor.update_input(Value::raw("nvim {}"));
        assert_eq!(
            governor.press_enter(),
            Some(Value::raw("nvim '/home/me/paper.pdf'"))
        );
    }

    #[tokio::test(start_paused = true)]
    async fn governor_updates_ranking_as_input_and_candidates_arrive() {
        let sources =
            vec![Box::new(MockSource::new(Duration::from_millis(100))) as Box<dyn AsyncSource>];
        let mut governor = Governor::with_sources(sources);

        governor.update_input(Value::raw(";m 10"));
        receive_next_candidate(&mut governor).await;
        assert_eq!(governor.selected(), None);

        receive_next_candidate(&mut governor).await;
        assert_eq!(governor.selected(), Some(Value::raw("1 00")));

        governor.update_input(Value::raw(";m 50"));
        assert_eq!(governor.selected(), None);

        for _ in 2..=5 {
            receive_next_candidate(&mut governor).await;
        }

        assert_eq!(governor.selected(), Some(Value::raw("5 00")));

        governor.update_input(Value::raw(";m 10"));
        assert_eq!(governor.selected(), Some(Value::raw("1 00")));

        for _ in 6..=10 {
            receive_next_candidate(&mut governor).await;
        }

        assert_eq!(governor.selected(), Some(Value::raw("10 00")));
    }

    #[tokio::test]
    async fn governor_feeds_async_filesystem_candidates_into_state() {
        let root = temp_app_dir("governor-filesystem");
        let nested = root.join("Documents");
        let file = nested.join("paper.pdf");
        fs::create_dir(&nested).expect("nested test directory should be created");
        fs::write(&file, b"pdf").expect("test file should be written");
        let mut governor = Governor::start(root, "");

        governor.update_input(Value::raw(";fpaper"));
        assert_eq!(governor.selected(), None);

        assert_eq!(
            receive_until_selected(&mut governor).await,
            Value::escaped(file.to_str().expect("path should be utf-8"))
        );
    }

    #[tokio::test]
    async fn governor_receives_cwd_and_path_sources() {
        let root = temp_app_dir("governor-default-root");
        let file = root.join("paper.pdf");
        fs::write(&file, b"pdf").expect("test file should be written");
        let bin = temp_app_dir("governor-default-path");
        fs::write(bin.join("fzlaunch-run-me"), b"#!/bin/sh\n")
            .expect("test executable should be written");
        fs::set_permissions(
            bin.join("fzlaunch-run-me"),
            fs::Permissions::from_mode(0o755),
        )
        .expect("test executable permissions should be set");
        let mut governor = Governor::start(root, &path_string(&[bin]));

        governor.update_input(Value::raw(";fpaper"));
        assert_eq!(
            receive_until_selected(&mut governor).await,
            Value::escaped(file.to_str().expect("path should be utf-8"))
        );

        governor.update_input(Value::raw(";crun"));
        assert_eq!(
            receive_until_selected(&mut governor).await,
            Value::raw("fzlaunch-run-me")
        );
    }
}
