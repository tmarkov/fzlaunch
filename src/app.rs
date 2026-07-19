use std::fs;
use std::io::{Read, Write};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::config::{Config, FilesystemSourceConfig, PluginSourceConfig, PluginSourceMode};
use crate::history::History;
use crate::model::{Candidate, CandidateSource, ExecutionMode, ExecutionPlan, Queue, Value};
use crate::preview::{Preview, PreviewOutput, PreviewRunner};
use crate::shell;
use crate::sources::{
    AsyncSource, Calculator, CandidateReceiver, Executables, FilesystemRoot, PluginCandidateBatch,
    PluginCandidateReceiver, PluginCandidateSender, PluginSource,
};
use crate::state::LauncherState;
use crate::ui::tui;
use tokio::sync::mpsc::error::TryRecvError;

const CANDIDATE_CHANNEL_CAPACITY: usize = 128;
const MAX_CANDIDATE_BATCHES_PER_TICK: usize = 8;

pub struct App {
    state: LauncherState,
    config: Config,
    history: History,
    calculator: Option<Calculator>,
    triggered_plugins: Vec<TriggeredPluginRuntime>,
    candidate_receiver: CandidateReceiver,
    plugin_candidate_sender: PluginCandidateSender,
    plugin_candidate_receiver: PluginCandidateReceiver,
    source_tasks: Vec<tokio::task::JoinHandle<()>>,
    preview_command: Option<String>,
    preview: Preview,
    preview_receiver: tokio::sync::mpsc::Receiver<PreviewOutput>,
    preview_runner: PreviewRunner,
}

struct TriggeredPluginRuntime {
    config: PluginSourceConfig,
    generation: u64,
    task: Option<tokio::task::JoinHandle<()>>,
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

        sources.extend(
            config
                .sources
                .plugins
                .iter()
                .filter(|plugin| plugin.enabled && plugin.mode == PluginSourceMode::Startup)
                .cloned()
                .map(|plugin| Box::new(PluginSource::new(plugin)) as Box<dyn AsyncSource>),
        );

        let calculator = config
            .sources
            .calculator
            .enabled
            .then(|| Calculator::new(config.sources.calculator.clone()));

        Self::with_sources_and_config_and_history(sources, config, history, calculator)
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
        let calculator = config
            .sources
            .calculator
            .enabled
            .then(|| Calculator::new(config.sources.calculator.clone()));
        Self::with_sources_and_config_and_history(sources, config, History::default(), calculator)
    }

    fn with_sources_and_config_and_history(
        sources: impl IntoIterator<Item = Box<dyn AsyncSource>>,
        config: Config,
        history: History,
        calculator: Option<Calculator>,
    ) -> Self {
        let (sender, candidate_receiver) = tokio::sync::mpsc::channel(CANDIDATE_CHANNEL_CAPACITY);
        let (plugin_candidate_sender, plugin_candidate_receiver) =
            tokio::sync::mpsc::channel(CANDIDATE_CHANNEL_CAPACITY);
        let (preview_sender, preview_receiver) = tokio::sync::mpsc::channel(8);
        let source_tasks = sources
            .into_iter()
            .map(|source| source.stream_candidates(sender.clone()))
            .collect();
        let triggered_plugins = config
            .sources
            .plugins
            .iter()
            .filter(|plugin| plugin.enabled && plugin.mode == PluginSourceMode::Triggered)
            .cloned()
            .map(|config| TriggeredPluginRuntime {
                config,
                generation: 0,
                task: None,
            })
            .collect();

        drop(sender);
        let preview_runner = PreviewRunner::new(preview_sender);
        let mut state = LauncherState::default();
        state.feed(history.candidates());

        Self {
            state,
            config,
            history,
            calculator,
            triggered_plugins,
            candidate_receiver,
            plugin_candidate_sender,
            plugin_candidate_receiver,
            source_tasks,
            preview_command: None,
            preview: Preview::Unavailable,
            preview_receiver,
            preview_runner,
        }
    }

    pub fn update_input(&mut self, value: Value) {
        let previous_value = self.state.value();
        self.state.update_input(value);
        self.refresh_calculator_candidates(previous_value.editable_text());
        self.refresh_triggered_plugin_candidates(previous_value.editable_text());
    }

    pub fn select_next(&mut self) {
        self.state.select_next();
    }

    pub fn select_previous(&mut self) {
        self.state.select_previous();
    }

    pub fn press_backtick(&mut self) {
        self.state.press_backtick();
        self.clear_calculator_candidates();
        self.clear_triggered_plugin_candidates();
    }

    pub fn press_tab(&mut self) {
        self.record_history_choice();
        self.state.press_tab();
        self.clear_calculator_candidates();
        self.clear_triggered_plugin_candidates();
    }

    pub fn press_enter(&mut self) -> Option<ExecutionPlan> {
        self.record_history_choice();
        let command = self.state.press_enter();
        self.clear_calculator_candidates();
        self.clear_triggered_plugin_candidates();
        command
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

    #[cfg(test)]
    pub async fn receive_plugin_candidates(&mut self) -> bool {
        let Some(batch) = self.plugin_candidate_receiver.recv().await else {
            return false;
        };

        self.apply_plugin_candidates(batch)
    }

    pub fn receive_pending_candidates(&mut self) -> usize {
        let mut batches = 0;
        let mut candidates = Vec::new();

        while batches < MAX_CANDIDATE_BATCHES_PER_TICK {
            match self.plugin_candidate_receiver.try_recv() {
                Ok(batch) => {
                    self.apply_plugin_candidates(batch);
                    batches += 1;
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => break,
            }
        }

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

    fn apply_plugin_candidates(&mut self, batch: PluginCandidateBatch) -> bool {
        let Some(runtime) = self
            .triggered_plugins
            .iter()
            .find(|runtime| runtime.config.name == batch.source_id)
        else {
            return false;
        };

        if runtime.generation != batch.generation {
            return false;
        }

        self.feed_candidates(batch.candidates);
        true
    }

    fn refresh_calculator_candidates(&mut self, previous_input: &str) {
        if self.state.mode() != crate::state::InputMode::Search {
            self.clear_calculator_candidates();
            return;
        }

        let current_value = self.state.value();
        let current_input = current_value.editable_text();
        let update = triggered_source_update(previous_input, current_input, '=');
        let candidates = match update {
            TriggeredSourceUpdate::Trigger => self
                .calculator
                .as_ref()
                .map(|calculator| calculator.candidates(current_input))
                .unwrap_or_default(),
            TriggeredSourceUpdate::Clear => Vec::new(),
            TriggeredSourceUpdate::Preserve => return,
        };
        let candidates = candidates
            .into_iter()
            .map(|candidate| self.history.apply_preference(candidate));
        self.state
            .replace_candidates_from_source(CandidateSource::Calculator, candidates);
    }

    fn clear_calculator_candidates(&mut self) {
        self.state
            .replace_candidates_from_source(CandidateSource::Calculator, []);
    }

    fn refresh_triggered_plugin_candidates(&mut self, previous_input: &str) {
        if self.state.mode() != crate::state::InputMode::Search {
            self.clear_triggered_plugin_candidates();
            return;
        }

        let current_value = self.state.value();
        let current_input = current_value.editable_text().to_string();
        for index in 0..self.triggered_plugins.len() {
            let selector = self.triggered_plugins[index].config.selector;
            match triggered_source_update(previous_input, &current_input, selector) {
                TriggeredSourceUpdate::Trigger => {
                    let args = triggered_source_args(&current_input, selector);
                    self.restart_triggered_plugin(index, args);
                }
                TriggeredSourceUpdate::Clear => self.stop_triggered_plugin(index),
                TriggeredSourceUpdate::Preserve => {}
            }
        }
    }

    fn restart_triggered_plugin(&mut self, index: usize, args: Vec<String>) {
        let (source_id, config, generation) = {
            let runtime = &mut self.triggered_plugins[index];
            if let Some(task) = runtime.task.take() {
                task.abort();
            }
            runtime.generation = runtime.generation.saturating_add(1);
            (
                runtime.config.name.clone(),
                runtime.config.clone(),
                runtime.generation,
            )
        };

        self.state.replace_candidates_from_plugin(&source_id, []);
        let task = PluginSource::stream_triggered_candidates(
            config,
            args,
            generation,
            self.plugin_candidate_sender.clone(),
        );
        self.triggered_plugins[index].task = Some(task);
    }

    fn stop_triggered_plugin(&mut self, index: usize) {
        let source_id = {
            let runtime = &mut self.triggered_plugins[index];
            if let Some(task) = runtime.task.take() {
                task.abort();
            }
            runtime.generation = runtime.generation.saturating_add(1);
            runtime.config.name.clone()
        };

        self.state.replace_candidates_from_plugin(&source_id, []);
    }

    fn clear_triggered_plugin_candidates(&mut self) {
        for index in 0..self.triggered_plugins.len() {
            self.stop_triggered_plugin(index);
        }
    }

    fn record_history_choice(&mut self) {
        let Some(candidate) = self.state.history_candidate() else {
            return;
        };

        let _ = self.history.record(&candidate);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TriggeredSourceUpdate {
    Trigger,
    Preserve,
    Clear,
}

fn triggered_source_update(
    previous_input: &str,
    current_input: &str,
    selector: char,
) -> TriggeredSourceUpdate {
    let trigger = format!(";{selector}");
    let previous_trigger_count = trigger_token_count(previous_input, &trigger);
    let current_trigger_count = trigger_token_count(current_input, &trigger);

    if current_trigger_count == 0 {
        return TriggeredSourceUpdate::Clear;
    }

    if current_trigger_count > previous_trigger_count {
        return TriggeredSourceUpdate::Trigger;
    }

    TriggeredSourceUpdate::Preserve
}

fn trigger_token_count(input: &str, trigger: &str) -> usize {
    input
        .split_whitespace()
        .filter(|term| *term == trigger)
        .count()
}

fn triggered_source_args(input: &str, selector: char) -> Vec<String> {
    let trigger = format!(";{selector}");
    let terms = input.split_whitespace().collect::<Vec<_>>();
    let Some(trigger_index) = terms.iter().rposition(|term| *term == trigger) else {
        return Vec::new();
    };

    terms[..trigger_index]
        .iter()
        .copied()
        .filter(|term| *term != trigger)
        .map(str::to_string)
        .collect()
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
        CandidateSource::Calculator => None,
        CandidateSource::Plugin => None,
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
        for runtime in &mut self.triggered_plugins {
            if let Some(task) = runtime.task.take() {
                task.abort();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;
    use std::time::Duration;

    use tokio::task::JoinHandle;
    use tokio::time;

    use super::*;
    use crate::config::{
        CalculatorSourceConfig, Config, FilesystemSourceConfig, PathSourceConfig,
        PluginSourceConfig, PluginSourceMode, SourceConfig,
    };
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

    fn write_plugin_script(dir: &TempDir, text: impl AsRef<[u8]>) -> PathBuf {
        let path = dir.join("plugin");
        fs::write(&path, text).expect("plugin script should be written");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755))
            .expect("plugin script should be executable");
        path
    }

    fn plugin_config(path: PathBuf, mode: PluginSourceMode) -> PluginSourceConfig {
        plugin_config_with_selector(path, mode, 's')
    }

    fn plugin_config_with_selector(
        path: PathBuf,
        mode: PluginSourceMode,
        selector: char,
    ) -> PluginSourceConfig {
        PluginSourceConfig {
            name: "content".to_string(),
            enabled: true,
            path,
            selector,
            mode,
            direct_action: Some(crate::model::Action::detached(Value::raw("xdg-open {}"))),
        }
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
    async fn app_prioritizes_triggered_plugin_batches_over_startup_batches() {
        let root = temp_app_dir("app-prioritizes-triggered-plugin-batches");
        let batches = (0..MAX_CANDIDATE_BATCHES_PER_TICK + 2).map(|index| {
            vec![Candidate::new(
                Value::raw(format!("command-{index}")),
                'c',
                None,
            )]
        });
        let mut app = App::with_sources_and_config(
            [Box::new(BatchSource::new(batches)) as Box<dyn AsyncSource>],
            Config {
                sources: SourceConfig {
                    plugins: vec![plugin_config_with_selector(
                        root.join("unused-plugin"),
                        PluginSourceMode::Triggered,
                        'r',
                    )],
                    ..SourceConfig::default()
                },
            },
        );
        app.state.update_input(Value::raw("invoice ;r"));
        app.triggered_plugins[0].generation = 1;
        app.plugin_candidate_sender
            .try_send(PluginCandidateBatch {
                source_id: "content".to_string(),
                generation: 1,
                candidates: vec![Candidate::new(Value::raw("invoice.pdf"), 'r', None)
                    .with_source(CandidateSource::Plugin)
                    .with_source_id("content")
                    .with_haystack(";r invoice paid")],
            })
            .expect("plugin batch should fit in channel");
        tokio::task::yield_now().await;

        assert_eq!(
            app.receive_pending_candidates(),
            MAX_CANDIDATE_BATCHES_PER_TICK
        );
        assert_eq!(selected_value(&app), Some(Value::raw("invoice.pdf")));
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

    #[test]
    fn app_calculator_source_evaluates_standalone_trigger() {
        let mut app = App::with_sources([]);

        app.update_input(Value::raw("3 + 4 ;="));

        assert_eq!(selected_value(&app), Some(Value::raw("7")));
        assert_eq!(app.state().results()[0].haystack, ";= 3 + 4 = 7");
        assert_eq!(
            app.press_enter(),
            Some(Value::raw("printf %s 7 | wl-copy").into())
        );
    }

    #[test]
    fn triggered_source_update_fires_when_selector_count_increases() {
        assert_eq!(
            triggered_source_update("", "3 + 4 ;= + 5", '='),
            TriggeredSourceUpdate::Trigger
        );
        assert_eq!(
            triggered_source_update("3 + 4 ;=", "3 + 4 ;= + 5", '='),
            TriggeredSourceUpdate::Preserve
        );
        assert_eq!(
            triggered_source_update("3 + 4 ;= + 5", "3 + 4 ;= + 5 ;=", '='),
            TriggeredSourceUpdate::Trigger
        );
        assert_eq!(
            triggered_source_update("3 + 4 ;=", "3 + 4", '='),
            TriggeredSourceUpdate::Clear
        );
    }

    #[test]
    fn triggered_source_args_use_terms_before_latest_selector() {
        assert_eq!(
            triggered_source_args("alpha beta ;s gamma ;s", 's'),
            vec!["alpha".to_string(), "beta".to_string(), "gamma".to_string()]
        );
    }

    #[tokio::test]
    async fn app_startup_plugin_feeds_jsonl_candidates() {
        let root = temp_app_dir("app-startup-plugin");
        let script = write_plugin_script(
            &root,
            r#"#!/bin/sh
printf '%s\n' '{"value":"startup-result","haystack":"startup result"}'
"#,
        );
        let mut app = App::start_with_config(
            root.path().to_path_buf(),
            "",
            Config {
                sources: SourceConfig {
                    path: PathSourceConfig {
                        enabled: false,
                        ..PathSourceConfig::default()
                    },
                    filesystem: FilesystemSourceConfig {
                        enabled: false,
                        ..FilesystemSourceConfig::default()
                    },
                    calculator: CalculatorSourceConfig {
                        enabled: false,
                        ..CalculatorSourceConfig::default()
                    },
                    plugins: vec![plugin_config(script, PluginSourceMode::Startup)],
                },
            },
        );

        assert!(app.receive_candidates().await);
        app.update_input(Value::raw("startup ;s"));

        assert_eq!(selected_value(&app), Some(Value::raw("startup-result")));
    }

    #[tokio::test]
    async fn app_triggered_plugin_receives_prompt_args() {
        let root = temp_app_dir("app-triggered-plugin-args");
        let script = write_plugin_script(
            &root,
            r#"#!/bin/sh
printf '{"value":"%s-%s","haystack":"%s %s"}\n' "$1" "$2" "$1" "$2"
"#,
        );
        let mut app = App::with_sources_and_config(
            [],
            Config {
                sources: SourceConfig {
                    plugins: vec![plugin_config(script, PluginSourceMode::Triggered)],
                    ..SourceConfig::default()
                },
            },
        );

        app.update_input(Value::raw("memory leak ;s"));

        assert!(
            time::timeout(Duration::from_secs(1), app.receive_plugin_candidates())
                .await
                .expect("plugin should respond before timeout")
        );
        assert_eq!(selected_value(&app), Some(Value::raw("memory-leak")));
    }

    #[tokio::test]
    async fn app_triggered_plugin_applies_configured_selector_to_plugin_haystack() {
        let root = temp_app_dir("app-triggered-plugin-configured-selector");
        let script = write_plugin_script(
            &root,
            r#"#!/bin/sh
printf '%s\n' '{"value":"invoice.pdf","haystack":"invoice paid"}'
"#,
        );
        let mut app = App::with_sources_and_config(
            [],
            Config {
                sources: SourceConfig {
                    plugins: vec![plugin_config_with_selector(
                        script,
                        PluginSourceMode::Triggered,
                        'r',
                    )],
                    ..SourceConfig::default()
                },
            },
        );

        app.update_input(Value::raw("invoice ;r"));

        assert!(
            time::timeout(Duration::from_secs(1), app.receive_plugin_candidates())
                .await
                .expect("plugin should respond before timeout")
        );
        assert_eq!(selected_value(&app), Some(Value::raw("invoice.pdf")));
        assert_eq!(app.state().results()[0].haystack, ";r invoice paid");
    }

    #[tokio::test]
    async fn app_triggered_plugin_retrigger_stops_previous_invocation() {
        let root = temp_app_dir("app-triggered-plugin-cancel");
        let stale_marker = root.join("stale-marker");
        let script = write_plugin_script(
            &root,
            format!(
                r#"#!/bin/sh
if [ "$2" = "fast" ]; then
  printf '%s\n' '{{"value":"fast","haystack":"slow fast"}}'
else
  sleep 0.2
  printf stale > "{}"
  printf '%s\n' '{{"value":"slow","haystack":"slow"}}'
fi
"#,
                stale_marker.to_str().expect("path should be utf-8")
            ),
        );
        let mut app = App::with_sources_and_config(
            [],
            Config {
                sources: SourceConfig {
                    plugins: vec![plugin_config(script, PluginSourceMode::Triggered)],
                    ..SourceConfig::default()
                },
            },
        );

        app.update_input(Value::raw("slow ;s"));
        app.update_input(Value::raw("slow ;s fast ;s"));

        assert!(
            time::timeout(Duration::from_secs(1), app.receive_plugin_candidates())
                .await
                .expect("plugin should respond before timeout")
        );
        assert_eq!(selected_value(&app), Some(Value::raw("fast")));

        time::sleep(Duration::from_millis(300)).await;
        assert_eq!(app.receive_pending_candidates(), 0);
        assert!(
            !stale_marker.exists(),
            "re-trigger should stop the previous plugin process before it writes"
        );
        assert_eq!(selected_value(&app), Some(Value::raw("fast")));
    }

    #[tokio::test]
    async fn app_triggered_plugin_clear_rejects_queued_stale_output() {
        let root = temp_app_dir("app-triggered-plugin-clear-stale");
        let script = root.join("plugin");
        let mut app = App::with_sources_and_config(
            [],
            Config {
                sources: SourceConfig {
                    plugins: vec![plugin_config(script, PluginSourceMode::Triggered)],
                    ..SourceConfig::default()
                },
            },
        );

        app.update_input(Value::raw("query ;s"));
        app.update_input(Value::raw("query"));

        assert!(!app.apply_plugin_candidates(PluginCandidateBatch {
            source_id: "content".to_string(),
            generation: 1,
            candidates: vec![Candidate::new(Value::raw("stale"), 's', None)
                .with_source(CandidateSource::Plugin)
                .with_source_id("content")],
        }));
        assert_eq!(selected_value(&app), None);
    }

    #[test]
    fn app_calculator_source_does_not_retrigger_after_selector() {
        let mut app = App::with_sources([]);

        app.update_input(Value::raw("3 + 4 ;="));
        assert_eq!(selected_value(&app), Some(Value::raw("7")));

        app.update_input(Value::raw("3 + 4 ;= + 5"));

        assert_eq!(selected_value(&app), None);
        assert_eq!(app.state().results(), Vec::new());
    }

    #[test]
    fn app_calculator_source_retriggers_when_selector_is_entered_again() {
        let mut app = App::with_sources([]);

        app.update_input(Value::raw("3 + 4 ;="));
        assert_eq!(selected_value(&app), Some(Value::raw("7")));

        app.update_input(Value::raw("3 + 4 ;= + 5"));
        assert_eq!(selected_value(&app), None);

        app.update_input(Value::raw("3 + 4 ;= + 5 ;="));

        assert_eq!(selected_value(&app), Some(Value::raw("12")));
        assert_eq!(app.state().results()[0].haystack, ";= 3 + 4 + 5 = 12");
    }

    #[test]
    fn app_calculator_source_subtraction_is_not_an_append_term() {
        let mut app = App::with_sources([]);

        app.update_input(Value::raw("7 - 9 ;="));

        assert_eq!(selected_value(&app), Some(Value::raw("-2")));
        assert_eq!(
            app.press_enter(),
            Some(Value::raw("printf %s -2 | wl-copy").into())
        );
    }

    #[test]
    fn app_calculator_source_requires_standalone_trigger() {
        let mut app = App::with_sources([]);

        app.update_input(Value::raw("3+4;=+5"));

        assert_eq!(selected_value(&app), None);
    }

    #[test]
    fn app_calculator_source_clears_results_when_trigger_is_removed() {
        let mut app = App::with_sources([]);

        app.update_input(Value::raw("3 + 4 ;="));
        assert_eq!(selected_value(&app), Some(Value::raw("7")));

        app.update_input(Value::raw("3 + 4"));

        assert_eq!(selected_value(&app), None);
        assert_eq!(app.state().results(), Vec::new());
    }

    #[test]
    fn app_skips_disabled_calculator_source() {
        let mut app = App::with_sources_and_config(
            [],
            Config {
                sources: SourceConfig {
                    calculator: CalculatorSourceConfig {
                        enabled: false,
                        ..CalculatorSourceConfig::default()
                    },
                    ..SourceConfig::default()
                },
            },
        );

        app.update_input(Value::raw("3 + 4 ;="));

        assert_eq!(selected_value(&app), None);
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
            None,
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
            None,
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
            None,
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
        fs::write(bin.join("fzlaunch-run@me"), b"#!/bin/sh\n")
            .expect("test executable should be written");
        fs::set_permissions(
            bin.join("fzlaunch-run@me"),
            fs::Permissions::from_mode(0o755),
        )
        .expect("test executable permissions should be set");
        let mut app = App::start(root.path().to_path_buf(), &path_string([&bin]));

        app.update_input(Value::raw(";fpaper"));
        assert_eq!(
            receive_until_selected(&mut app).await,
            Value::escaped(file.to_str().expect("path should be utf-8"))
        );

        app.update_input(Value::raw(";c@"));
        assert_eq!(
            receive_until_selected(&mut app).await,
            Value::raw("fzlaunch-run@me")
        );
    }

    #[tokio::test]
    async fn app_skips_disabled_sources() {
        let root = temp_app_dir("app-disabled-sources-root");
        let file = root.join("paper.pdf");
        fs::write(&file, b"pdf").expect("test file should be written");
        let bin = temp_app_dir("app-disabled-sources-path");
        fs::write(bin.join("fzlaunch-run@me"), b"#!/bin/sh\n")
            .expect("test executable should be written");
        fs::set_permissions(
            bin.join("fzlaunch-run@me"),
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
        app_without_path.update_input(Value::raw(";c@"));
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
