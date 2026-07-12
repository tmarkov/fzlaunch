use std::fs;
use std::io::{Read, Write};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::config::{Config, FilesystemSourceConfig};
use crate::history::History;
use crate::model::{Candidate, CandidateSource, ExecutionMode, ExecutionPlan, Queue, Value};
use crate::preview::{Preview, PreviewOutput, PreviewRunner};
use crate::shell;
use crate::sources::{AsyncSource, CandidateReceiver, Executables, FilesystemRoot};
use crate::state::LauncherState;
use crate::ui::tui;
use tokio::sync::mpsc::error::TryRecvError;

const CANDIDATE_CHANNEL_CAPACITY: usize = 128;
const MAX_CANDIDATE_BATCHES_PER_TICK: usize = 8;

pub struct App {
    state: LauncherState,
    config: Config,
    history: History,
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
    let data_dirs = executable_data_dirs();
    let config = Config::load()?;
    let history = History::load()?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;

    if let Some(command) = runtime.block_on(async {
        let mut app = App::start_with_config_and_history(cwd, &path, &data_dirs, config, history);
        tui::run(&mut app).await
    })? {
        println!("{}", shell::render_value(command.command()));
        println!("plan: {}", render_execution_plan(&command));
        std::io::stdout().flush()?;
        execute_plan(&command)?;
    }

    Ok(())
}

fn execute_plan(plan: &ExecutionPlan) -> std::io::Result<()> {
    let command = shell::render_value(plan.command());

    match plan.execution_mode() {
        ExecutionMode::Foreground => exec_foreground(command),
        ExecutionMode::Detached => spawn_detached(command),
    }
}

fn exec_foreground(command: String) -> std::io::Result<()> {
    Err(Command::new("sh").arg("-c").arg(command).exec())
}

fn spawn_detached(command: String) -> std::io::Result<()> {
    let mut child = Command::new("sh");
    child
        .arg("-c")
        .arg(command)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    // Only async-signal-safe work is allowed after fork and before exec.
    unsafe {
        child.pre_exec(|| {
            if libc::setsid() == -1 {
                Err(std::io::Error::last_os_error())
            } else {
                Ok(())
            }
        });
    }

    child.spawn()?;
    Ok(())
}

fn render_execution_plan(plan: &ExecutionPlan) -> String {
    format!(
        "execution_mode={} command={}",
        execution_mode_name(plan.execution_mode()),
        shell::render_value(plan.command())
    )
}

fn execution_mode_name(mode: ExecutionMode) -> &'static str {
    match mode {
        ExecutionMode::Foreground => "foreground",
        ExecutionMode::Detached => "detached",
    }
}

fn executable_data_dirs() -> String {
    let mut dirs = Vec::new();

    if let Some(home) = std::env::var_os("XDG_DATA_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
    {
        dirs.push(home);
    } else if let Some(home) = std::env::var_os("HOME").filter(|value| !value.is_empty()) {
        dirs.push(PathBuf::from(home).join(".local/share"));
    }

    if let Some(data_dirs) = std::env::var_os("XDG_DATA_DIRS").filter(|value| !value.is_empty()) {
        dirs.extend(std::env::split_paths(&data_dirs));
    } else {
        dirs.extend([
            PathBuf::from("/usr/local/share"),
            PathBuf::from("/usr/share"),
        ]);
    }

    std::env::join_paths(dirs)
        .expect("data dirs should be joinable")
        .to_string_lossy()
        .into_owned()
}

impl App {
    #[cfg(test)]
    pub fn start(cwd: PathBuf, path: &str) -> Self {
        Self::start_with_config(cwd, path, Config::default())
    }

    #[cfg(test)]
    pub fn start_with_config(cwd: PathBuf, path: &str, config: Config) -> Self {
        Self::start_with_config_and_history(cwd, path, "", config, History::default())
    }

    fn start_with_config_and_history(
        cwd: PathBuf,
        path: &str,
        data_dirs: &str,
        config: Config,
        history: History,
    ) -> Self {
        let mut sources = Vec::new();

        if config.sources.filesystem.enabled {
            sources.push(Box::new(FilesystemRoot::new_with_config(
                cwd,
                config.sources.filesystem.clone(),
            )) as Box<dyn AsyncSource>);
        }

        if config.sources.path.enabled {
            sources.push(Box::new(Executables::from_path_and_data_dirs_with_config(
                path,
                data_dirs,
                config.sources.path.clone(),
            )) as Box<dyn AsyncSource>);
        }

        Self::with_sources_and_config_and_history(sources, config, history)
    }

    #[cfg(test)]
    pub fn with_sources(sources: impl IntoIterator<Item = Box<dyn AsyncSource>>) -> Self {
        Self::with_sources_and_config(sources, Config::default())
    }

    #[cfg(test)]
    fn with_sources_and_config(
        sources: impl IntoIterator<Item = Box<dyn AsyncSource>>,
        config: Config,
    ) -> Self {
        Self::with_sources_and_config_and_history(sources, config, History::default())
    }

    fn with_sources_and_config_and_history(
        sources: impl IntoIterator<Item = Box<dyn AsyncSource>>,
        config: Config,
        history: History,
    ) -> Self {
        let (sender, candidate_receiver) = tokio::sync::mpsc::channel(CANDIDATE_CHANNEL_CAPACITY);
        let (preview_sender, preview_receiver) = tokio::sync::mpsc::channel(8);
        let source_tasks = sources
            .into_iter()
            .map(|source| source.stream_candidates(sender.clone()))
            .collect();

        drop(sender);
        let preview_runner = PreviewRunner::new(preview_sender);
        let mut state = LauncherState::default();
        state.feed(history.candidates());

        Self {
            state,
            config,
            history,
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

    pub fn press_backtick(&mut self) {
        self.state.press_backtick();
    }

    pub fn press_tab(&mut self) {
        self.record_history_choice();
        self.state.press_tab();
    }

    pub fn press_enter(&mut self) -> Option<ExecutionPlan> {
        self.record_history_choice();
        self.state.press_enter()
    }

    pub fn state(&self) -> &LauncherState {
        &self.state
    }

    pub fn refresh_preview(&mut self) {
        let command = selected_preview_command(self.state.selected(), &self.config);
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

        self.feed_candidates(candidates);
        true
    }

    pub fn receive_pending_candidates(&mut self) -> usize {
        let mut batches = 0;
        let mut candidates = Vec::new();

        while batches < MAX_CANDIDATE_BATCHES_PER_TICK {
            match self.candidate_receiver.try_recv() {
                Ok(batch) => {
                    candidates.extend(batch);
                    batches += 1;
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => break,
            }
        }

        if !candidates.is_empty() {
            self.feed_candidates(candidates);
        }

        batches
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

    fn feed_candidates(&mut self, candidates: Vec<Candidate>) {
        let candidates = candidates
            .into_iter()
            .map(|candidate| self.history.apply_preference(candidate));
        self.state.feed(candidates);
    }

    fn record_history_choice(&mut self) {
        let Some(candidate) = self.state.history_candidate() else {
            return;
        };

        let _ = self.history.record(&candidate);
    }
}

fn selected_preview_command(candidate: Option<Candidate>, config: &Config) -> Option<String> {
    let candidate = candidate?;
    let preview_command = match candidate.source() {
        CandidateSource::Generic => None,
        CandidateSource::PathExecutable => Some(config.sources.path.preview_command.clone()),
        CandidateSource::FilesystemPath => filesystem_preview_command(
            candidate.value().editable_text(),
            &config.sources.filesystem,
        ),
        CandidateSource::History => None,
    }?;

    let mut queue = Queue::from_values([candidate.value().clone()]);
    queue.compose(preview_command);
    queue.status()
}

fn filesystem_preview_command(path: &str, config: &FilesystemSourceConfig) -> Option<Value> {
    let path = Path::new(path);
    let Ok(metadata) = fs::metadata(path) else {
        return config.binary_preview_command.clone();
    };

    if metadata.is_dir() {
        return config.directory_preview_command.clone();
    }

    if !metadata.is_file() {
        return config.binary_preview_command.clone();
    }

    match filesystem_file_kind(path) {
        FilesystemFileKind::Text => config.text_file_preview_command.clone(),
        FilesystemFileKind::Document => config.document_preview_command.clone(),
        FilesystemFileKind::Image => config.image_preview_command.clone(),
        FilesystemFileKind::Archive => config.archive_preview_command.clone(),
        FilesystemFileKind::Media => config.media_preview_command.clone(),
        FilesystemFileKind::Binary => config.binary_preview_command.clone(),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FilesystemFileKind {
    Text,
    Document,
    Image,
    Archive,
    Media,
    Binary,
}

fn filesystem_file_kind(path: &Path) -> FilesystemFileKind {
    if is_text_file(path) {
        return FilesystemFileKind::Text;
    }

    match file_extension(path).as_deref() {
        Some(
            "pdf" | "ps" | "epub" | "djvu" | "doc" | "docx" | "odt" | "rtf" | "xls" | "xlsx"
            | "ods" | "ppt" | "pptx" | "odp",
        ) => FilesystemFileKind::Document,
        Some(
            "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "tif" | "tiff" | "avif" | "heic"
            | "ico",
        ) => FilesystemFileKind::Image,
        Some(
            "zip" | "tar" | "gz" | "tgz" | "bz2" | "tbz2" | "xz" | "txz" | "zst" | "7z" | "rar",
        ) => FilesystemFileKind::Archive,
        Some("mp3" | "flac" | "wav" | "ogg" | "m4a" | "mp4" | "mkv" | "webm" | "avi" | "mov") => {
            FilesystemFileKind::Media
        }
        _ => FilesystemFileKind::Binary,
    }
}

fn file_extension(path: &Path) -> Option<String> {
    path.extension()?
        .to_str()
        .map(|extension| extension.to_lowercase())
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
    use crate::config::{Config, FilesystemSourceConfig, PathSourceConfig, SourceConfig};
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

    struct BatchSource {
        batches: Vec<Vec<Candidate>>,
    }

    impl BatchSource {
        fn new(batches: impl IntoIterator<Item = Vec<Candidate>>) -> Self {
            Self {
                batches: batches.into_iter().collect(),
            }
        }
    }

    impl AsyncSource for BatchSource {
        fn stream_candidates(self: Box<Self>, sender: CandidateSender) -> JoinHandle<()> {
            tokio::spawn(async move {
                for batch in self.batches {
                    if sender.send(batch).await.is_err() {
                        break;
                    }
                }
            })
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

    fn selected_value(app: &App) -> Option<Value> {
        app.state()
            .selected()
            .map(|candidate| candidate.value().clone())
    }

    #[test]
    fn execution_plan_rendering_includes_command_and_execution_mode() {
        let plan = ExecutionPlan::new(
            Value::raw("less '/home/me/paper.pdf'"),
            ExecutionMode::Foreground,
        );

        assert_eq!(
            render_execution_plan(&plan),
            "execution_mode=foreground command=less '/home/me/paper.pdf'"
        );
    }

    fn preview_for_path(path: &std::path::Path, config: &Config) -> Option<String> {
        selected_preview_command(
            Some(
                Candidate::new(
                    Value::escaped(path.to_str().expect("path should be utf-8")),
                    'f',
                    None,
                )
                .with_source(CandidateSource::FilesystemPath),
            ),
            config,
        )
    }

    async fn receive_until_selected(app: &mut App) -> Value {
        while selected_value(app).is_none() {
            assert!(app.receive_candidates().await);
        }

        selected_value(app).expect("app should have selected value")
    }

    async fn receive_all_candidates(app: &mut App) {
        while app.receive_candidates().await {}
    }

    #[tokio::test]
    async fn app_limits_pending_candidate_batches_per_tick() {
        let batches = (0..MAX_CANDIDATE_BATCHES_PER_TICK + 2).map(|index| {
            vec![Candidate::new(
                Value::raw(format!("command-{index}")),
                'c',
                None,
            )]
        });
        let mut app =
            App::with_sources([Box::new(BatchSource::new(batches)) as Box<dyn AsyncSource>]);
        tokio::task::yield_now().await;

        assert_eq!(
            app.receive_pending_candidates(),
            MAX_CANDIDATE_BATCHES_PER_TICK
        );
        assert_eq!(app.state().results().len(), MAX_CANDIDATE_BATCHES_PER_TICK);

        assert_eq!(app.receive_pending_candidates(), 2);
        assert_eq!(
            app.state().results().len(),
            MAX_CANDIDATE_BATCHES_PER_TICK + 2
        );
    }

    #[tokio::test]
    async fn app_refreshes_preview_for_selected_candidate() {
        let mut app = App::with_sources_and_config(
            [Box::new(StaticSource::new([Candidate::new(
                Value::raw("paper"),
                'c',
                None,
            )
            .with_source(CandidateSource::PathExecutable)]))
                as Box<dyn AsyncSource>],
            Config {
                sources: SourceConfig {
                    path: PathSourceConfig {
                        preview_command: Value::raw("printf 'paper preview'"),
                        ..PathSourceConfig::default()
                    },
                    ..SourceConfig::default()
                },
            },
        );

        assert!(app.receive_candidates().await);
        app.update_input(Value::raw(";cpaper"));
        app.refresh_preview();

        assert_eq!(app.preview(), &Preview::Loading);
        assert!(app.receive_preview().await);
        assert_eq!(app.preview(), &Preview::Ready("paper preview".into()));
    }

    #[test]
    fn filesystem_preview_command_is_selected_by_file_kind() {
        let root = temp_app_dir("app-filesystem-preview-kind");
        let directory = root.join("Documents");
        let text = root.join("notes.txt");
        let document = root.join("paper.pdf");
        let image = root.join("photo.png");
        let archive = root.join("backup.zip");
        let media = root.join("song.mp3");
        let binary = root.join("program.bin");
        fs::create_dir(&directory).expect("test directory should be created");
        fs::write(&text, b"plain text").expect("text file should be written");
        fs::write(&document, b"%PDF\0").expect("document file should be written");
        fs::write(&image, b"\x89PNG\r\n\x1a\n\0").expect("image file should be written");
        fs::write(&archive, b"PK\x03\x04\0").expect("archive file should be written");
        fs::write(&media, b"ID3\0").expect("media file should be written");
        fs::write(&binary, b"\0binary").expect("binary file should be written");
        let config = Config {
            sources: SourceConfig {
                filesystem: FilesystemSourceConfig {
                    directory_preview_command: Some(Value::raw("preview-dir {}")),
                    text_file_preview_command: Some(Value::raw("preview-text {}")),
                    document_preview_command: Some(Value::raw("preview-document {}")),
                    image_preview_command: Some(Value::raw("preview-image {}")),
                    archive_preview_command: Some(Value::raw("preview-archive {}")),
                    media_preview_command: Some(Value::raw("preview-media {}")),
                    binary_preview_command: Some(Value::raw("preview-binary {}")),
                    ..FilesystemSourceConfig::default()
                },
                ..SourceConfig::default()
            },
        };

        assert_eq!(
            preview_for_path(&directory, &config),
            Some(format!(
                "preview-dir '{}'",
                directory.to_str().expect("path should be utf-8")
            ))
        );
        assert_eq!(
            preview_for_path(&text, &config),
            Some(format!(
                "preview-text '{}'",
                text.to_str().expect("path should be utf-8")
            ))
        );
        assert_eq!(
            preview_for_path(&document, &config),
            Some(format!(
                "preview-document '{}'",
                document.to_str().expect("path should be utf-8")
            ))
        );
        assert_eq!(
            preview_for_path(&image, &config),
            Some(format!(
                "preview-image '{}'",
                image.to_str().expect("path should be utf-8")
            ))
        );
        assert_eq!(
            preview_for_path(&archive, &config),
            Some(format!(
                "preview-archive '{}'",
                archive.to_str().expect("path should be utf-8")
            ))
        );
        assert_eq!(
            preview_for_path(&media, &config),
            Some(format!(
                "preview-media '{}'",
                media.to_str().expect("path should be utf-8")
            ))
        );
        assert_eq!(
            preview_for_path(&binary, &config),
            Some(format!(
                "preview-binary '{}'",
                binary.to_str().expect("path should be utf-8")
            ))
        );
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
            selected_value(&app),
            Some(Value::escaped("/home/me/paper.pdf"))
        );
        assert_eq!(app.state().current(), Value::escaped("/home/me/paper.pdf"));

        app.press_tab();
        assert_eq!(
            app.state().queue_status(),
            Some("'/home/me/paper.pdf'".into())
        );

        app.update_input(Value::raw(";cnvim"));
        assert_eq!(selected_value(&app), Some(Value::raw("nvim")));

        app.press_backtick();
        assert_eq!(app.state().mode(), InputMode::Edit);
        assert_eq!(app.state().value(), Value::raw("nvim"));

        app.update_input(Value::raw("nvim {}"));
        assert_eq!(
            app.press_enter(),
            Some(Value::raw("nvim '/home/me/paper.pdf'").into())
        );
    }

    #[tokio::test]
    async fn app_records_selected_choices_on_tab_and_enter() {
        let root = temp_app_dir("app-history-record");
        let path = root.join("history.tsv");
        let bash = Candidate::new(Value::raw("bash"), 'c', Some(Value::raw("{}")))
            .with_source(CandidateSource::PathExecutable);
        let zsh = Candidate::new(Value::raw("zsh"), 'c', Some(Value::raw("{}")))
            .with_source(CandidateSource::PathExecutable);
        let mut app = App::with_sources_and_config_and_history(
            [Box::new(StaticSource::new([bash.clone(), zsh.clone()])) as Box<dyn AsyncSource>],
            Config::default(),
            History::load_from_path(&path).expect("history should load"),
        );

        assert!(app.receive_candidates().await);
        app.update_input(Value::raw(";cbash"));
        app.press_tab();
        app.update_input(Value::raw(";czsh"));
        let _ = app.press_enter();

        let history = History::load_from_path(&path).expect("history should reload");
        assert!(history.score(&bash) > 0);
        assert!(history.score(&zsh) > 0);
    }

    #[tokio::test]
    async fn app_saves_edited_choices_as_future_history_candidates() {
        let root = temp_app_dir("app-history-edited");
        let path = root.join("history.tsv");
        let mv = Candidate::new(Value::raw("mv"), 'c', Some(Value::raw("{}")))
            .with_source(CandidateSource::PathExecutable);
        let mut app = App::with_sources_and_config_and_history(
            [Box::new(StaticSource::new([mv])) as Box<dyn AsyncSource>],
            Config::default(),
            History::load_from_path(&path).expect("history should load"),
        );

        assert!(app.receive_candidates().await);
        app.update_input(Value::raw(";cmv"));
        app.press_backtick();
        app.update_input(Value::raw("mv {} {}"));
        app.press_tab();

        let mut app = App::with_sources_and_config_and_history(
            Vec::<Box<dyn AsyncSource>>::new(),
            Config::default(),
            History::load_from_path(&path).expect("history should reload"),
        );
        app.update_input(Value::raw(";cmv"));

        assert_eq!(selected_value(&app), Some(Value::raw("mv {} {}")));
    }

    #[tokio::test(start_paused = true)]
    async fn app_updates_ranking_as_input_and_candidates_arrive() {
        let sources =
            vec![Box::new(MockSource::new(Duration::from_millis(100))) as Box<dyn AsyncSource>];
        let mut app = App::with_sources(sources);

        app.update_input(Value::raw(";m 10"));
        receive_next_candidate(&mut app).await;
        assert_eq!(selected_value(&app), None);

        receive_next_candidate(&mut app).await;
        assert_eq!(selected_value(&app), Some(Value::raw("1 00")));

        app.update_input(Value::raw(";m 50"));
        assert_eq!(selected_value(&app), None);

        for _ in 2..=5 {
            receive_next_candidate(&mut app).await;
        }

        assert_eq!(selected_value(&app), Some(Value::raw("5 00")));

        app.update_input(Value::raw(";m 10"));
        assert_eq!(selected_value(&app), Some(Value::raw("1 00")));

        for _ in 6..=10 {
            receive_next_candidate(&mut app).await;
        }

        assert_eq!(selected_value(&app), Some(Value::raw("10 00")));
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
        assert_eq!(selected_value(&app), None);

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
        assert_eq!(selected_value(&app_without_path), None);

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
        assert_eq!(selected_value(&app_without_filesystem), None);
    }
}
