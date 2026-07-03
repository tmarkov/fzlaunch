use std::path::PathBuf;

use crate::config::Config;
use crate::model::Value;
use crate::preview::{Preview, PreviewOutput, PreviewRunner};
use crate::shell;
use crate::sources::{AsyncSource, CandidateReceiver, FilesystemRoot, PathExecutables};
use crate::state::LauncherState;
use crate::ui::tui;
use tokio::sync::mpsc::error::TryRecvError;

const CANDIDATE_CHANNEL_CAPACITY: usize = 128;

pub struct App {
    state: LauncherState,
    candidate_receiver: CandidateReceiver,
    source_tasks: Vec<tokio::task::JoinHandle<()>>,
    preview_command: Option<String>,
    preview: Preview,
    preview_receiver: tokio::sync::mpsc::Receiver<PreviewOutput>,
    preview_runner: PreviewRunner,
}

pub fn run() {
    if let Err(error) = run_inner() {
        eprintln!("fzlaunch: {error}");
    }
}

fn run_inner() -> Result<(), Box<dyn std::error::Error>> {
    let cwd = std::env::current_dir()?;
    let path = std::env::var("PATH").unwrap_or_default();
    let config = Config::load()?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;

    if let Some(command) = runtime.block_on(async {
        let mut app = App::start_with_config(cwd, &path, config);
        tui::run(&mut app).await
    })? {
        println!("{}", shell::render_value(&command));
    }

    Ok(())
}

impl App {
    #[cfg(test)]
    pub fn start(cwd: PathBuf, path: &str) -> Self {
        Self::start_with_config(cwd, path, Config::default())
    }

    pub fn start_with_config(cwd: PathBuf, path: &str, config: Config) -> Self {
        let mut sources = Vec::new();

        if config.sources.filesystem.enabled {
            sources.push(Box::new(FilesystemRoot::new_with_config(
                cwd,
                config.sources.filesystem,
            )) as Box<dyn AsyncSource>);
        }

        if config.sources.path.enabled {
            sources.push(Box::new(PathExecutables::from_path_with_config(
                path,
                config.sources.path,
            )) as Box<dyn AsyncSource>);
        }

        Self::with_sources(sources)
    }

    pub fn with_sources(sources: impl IntoIterator<Item = Box<dyn AsyncSource>>) -> Self {
        let (sender, candidate_receiver) = tokio::sync::mpsc::channel(CANDIDATE_CHANNEL_CAPACITY);
        let (preview_sender, preview_receiver) = tokio::sync::mpsc::channel(8);
        let source_tasks = sources
            .into_iter()
            .map(|source| source.stream_candidates(sender.clone()))
            .collect();

        drop(sender);
        let preview_runner = PreviewRunner::new(preview_sender);

        Self {
            state: LauncherState::default(),
            candidate_receiver,
            source_tasks,
            preview_command: None,
            preview: Preview::Unavailable,
            preview_receiver,
            preview_runner,
        }
    }

    pub fn update_input(&mut self, value: Value) {
        self.state.update_input(value);
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

    pub fn state(&self) -> &LauncherState {
        &self.state
    }

    pub fn refresh_preview(&mut self) {
        let command = self.state.selected_preview_command();
        if self.preview_command == command {
            return;
        }

        self.preview_command = command.clone();
        let Some(command) = command else {
            self.preview_runner.abort();
            self.preview = Preview::Unavailable;
            return;
        };

        self.preview = Preview::Loading;
        self.preview_runner.start(command);
    }

    pub fn preview(&self) -> &Preview {
        &self.preview
    }

    #[cfg(test)]
    pub async fn receive_preview(&mut self) -> bool {
        let Some(preview) = self.preview_receiver.recv().await else {
            return false;
        };

        self.apply_preview(preview)
    }

    #[cfg(test)]
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

    pub fn receive_pending_preview(&mut self) -> bool {
        let mut updated = false;

        loop {
            match self.preview_receiver.try_recv() {
                Ok(preview) => updated |= self.apply_preview(preview),
                Err(TryRecvError::Empty) => return updated,
                Err(TryRecvError::Disconnected) => return updated,
            }
        }
    }

    fn apply_preview(&mut self, preview: PreviewOutput) -> bool {
        if self.preview_command.as_deref() != Some(preview.command.as_str()) {
            return false;
        }

        self.preview = preview.preview;
        true
    }
}

impl Drop for App {
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
    use std::time::Duration;

    use tokio::task::JoinHandle;
    use tokio::time;

    use super::*;
    use crate::config::{Config, SourceConfig};
    use crate::model::Candidate;
    use crate::sources::CandidateSender;
    use crate::state::InputMode;
    use crate::test_support::{path_string, TempDir};

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

    struct StaticSource {
        candidates: Vec<Candidate>,
    }

    impl StaticSource {
        fn new(candidates: impl IntoIterator<Item = Candidate>) -> Self {
            Self {
                candidates: candidates.into_iter().collect(),
            }
        }
    }

    impl AsyncSource for StaticSource {
        fn stream_candidates(self: Box<Self>, sender: CandidateSender) -> JoinHandle<()> {
            tokio::spawn(async move {
                let _ = sender.send(self.candidates).await;
            })
        }
    }

    async fn receive_next_candidate(app: &mut App) {
        time::advance(Duration::from_millis(100)).await;
        assert!(app.receive_candidates().await);
    }

    fn temp_app_dir(name: &str) -> TempDir {
        TempDir::new(name)
    }

    async fn receive_until_selected(app: &mut App) -> Value {
        while app.state().selected().is_none() {
            assert!(app.receive_candidates().await);
        }

        app.state()
            .selected()
            .expect("app should have selected value")
    }

    async fn receive_all_candidates(app: &mut App) {
        while app.receive_candidates().await {}
    }

    #[tokio::test]
    async fn app_refreshes_preview_for_selected_candidate() {
        let mut app = App::with_sources([Box::new(StaticSource::new([Candidate::new(
            Value::escaped("/home/me/paper.pdf"),
            'f',
            None,
        )
        .with_preview_command(Some(Value::raw("printf 'paper preview'")))]))
            as Box<dyn AsyncSource>]);

        assert!(app.receive_candidates().await);
        app.update_input(Value::raw(";fpaper"));
        app.refresh_preview();

        assert_eq!(app.preview(), &Preview::Loading);
        assert!(app.receive_preview().await);
        assert_eq!(app.preview(), &Preview::Ready("paper preview".into()));
    }

    #[tokio::test]
    async fn app_forwards_launcher_state_operations() {
        let mut app = App::with_sources([Box::new(StaticSource::new([
            Candidate::new(Value::escaped("/home/me/paper.pdf"), 'f', None),
            Candidate::new(Value::raw("nvim"), 'c', None),
        ])) as Box<dyn AsyncSource>]);

        assert!(app.receive_candidates().await);
        app.update_input(Value::raw(";fpaper"));

        assert_eq!(
            app.state().selected(),
            Some(Value::escaped("/home/me/paper.pdf"))
        );
        assert_eq!(app.state().current(), Value::escaped("/home/me/paper.pdf"));

        app.press_tab();
        assert_eq!(
            app.state().queue_status(),
            Some("'/home/me/paper.pdf'".into())
        );

        app.update_input(Value::raw(";cnvim"));
        assert_eq!(app.state().selected(), Some(Value::raw("nvim")));

        app.press_tilde();
        assert_eq!(app.state().mode(), InputMode::Edit);
        assert_eq!(app.state().value(), Value::raw("nvim"));

        app.update_input(Value::raw("nvim {}"));
        assert_eq!(
            app.press_enter(),
            Some(Value::raw("nvim '/home/me/paper.pdf'"))
        );
    }

    #[tokio::test(start_paused = true)]
    async fn app_updates_ranking_as_input_and_candidates_arrive() {
        let sources =
            vec![Box::new(MockSource::new(Duration::from_millis(100))) as Box<dyn AsyncSource>];
        let mut app = App::with_sources(sources);

        app.update_input(Value::raw(";m 10"));
        receive_next_candidate(&mut app).await;
        assert_eq!(app.state().selected(), None);

        receive_next_candidate(&mut app).await;
        assert_eq!(app.state().selected(), Some(Value::raw("1 00")));

        app.update_input(Value::raw(";m 50"));
        assert_eq!(app.state().selected(), None);

        for _ in 2..=5 {
            receive_next_candidate(&mut app).await;
        }

        assert_eq!(app.state().selected(), Some(Value::raw("5 00")));

        app.update_input(Value::raw(";m 10"));
        assert_eq!(app.state().selected(), Some(Value::raw("1 00")));

        for _ in 6..=10 {
            receive_next_candidate(&mut app).await;
        }

        assert_eq!(app.state().selected(), Some(Value::raw("10 00")));
    }

    #[tokio::test]
    async fn app_feeds_async_filesystem_candidates_into_state() {
        let root = temp_app_dir("app-filesystem");
        let nested = root.join("Documents");
        let file = nested.join("paper.pdf");
        fs::create_dir(&nested).expect("nested test directory should be created");
        fs::write(&file, b"pdf").expect("test file should be written");
        let mut app = App::start(root.path().to_path_buf(), "");

        app.update_input(Value::raw(";fpaper"));
        assert_eq!(app.state().selected(), None);

        assert_eq!(
            receive_until_selected(&mut app).await,
            Value::escaped(file.to_str().expect("path should be utf-8"))
        );
    }

    #[tokio::test]
    async fn app_receives_cwd_and_path_sources() {
        let root = temp_app_dir("app-default-root");
        let file = root.join("paper.pdf");
        fs::write(&file, b"pdf").expect("test file should be written");
        let bin = temp_app_dir("app-default-path");
        fs::write(bin.join("fzlaunch-run-me"), b"#!/bin/sh\n")
            .expect("test executable should be written");
        fs::set_permissions(
            bin.join("fzlaunch-run-me"),
            fs::Permissions::from_mode(0o755),
        )
        .expect("test executable permissions should be set");
        let mut app = App::start(root.path().to_path_buf(), &path_string([&bin]));

        app.update_input(Value::raw(";fpaper"));
        assert_eq!(
            receive_until_selected(&mut app).await,
            Value::escaped(file.to_str().expect("path should be utf-8"))
        );

        app.update_input(Value::raw(";crun"));
        assert_eq!(
            receive_until_selected(&mut app).await,
            Value::raw("fzlaunch-run-me")
        );
    }

    #[tokio::test]
    async fn app_skips_disabled_sources() {
        let root = temp_app_dir("app-disabled-sources-root");
        let file = root.join("paper.pdf");
        fs::write(&file, b"pdf").expect("test file should be written");
        let bin = temp_app_dir("app-disabled-sources-path");
        fs::write(bin.join("fzlaunch-run-me"), b"#!/bin/sh\n")
            .expect("test executable should be written");
        fs::set_permissions(
            bin.join("fzlaunch-run-me"),
            fs::Permissions::from_mode(0o755),
        )
        .expect("test executable permissions should be set");

        let mut app_without_path = App::start_with_config(
            root.path().to_path_buf(),
            &path_string([&bin]),
            Config {
                sources: SourceConfig {
                    path: crate::config::PathSourceConfig {
                        enabled: false,
                        ..Default::default()
                    },
                    ..Default::default()
                },
            },
        );
        receive_all_candidates(&mut app_without_path).await;
        app_without_path.update_input(Value::raw(";crun"));
        assert_eq!(app_without_path.state().selected(), None);

        let mut app_without_filesystem = App::start_with_config(
            root.path().to_path_buf(),
            &path_string([&bin]),
            Config {
                sources: SourceConfig {
                    filesystem: crate::config::FilesystemSourceConfig {
                        enabled: false,
                        ..Default::default()
                    },
                    ..Default::default()
                },
            },
        );
        receive_all_candidates(&mut app_without_filesystem).await;
        app_without_filesystem.update_input(Value::raw(";fpaper"));
        assert_eq!(app_without_filesystem.state().selected(), None);
    }
}
