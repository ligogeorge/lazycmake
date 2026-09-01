use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use clap::Parser;
use crossterm::event::{self, Event, KeyCode, KeyEvent};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen};
use crossterm::{cursor, ExecutableCommand};
use crossbeam_channel::{unbounded, Receiver, Sender};
use lazycmake_core::{
    clean_cache, cmake_cache_matches_preset, ctest_output_indicates_failure, ensure_codemodel_query,
    executable_path, load_targets, test_all_steps, BuildCommand, Capabilities, ColumnState,
    CommandStep, ConfigOptions, ConfigureCommand, CtestDiscovery, CodemodelTarget, EnvOverlay,
    FocusedColumn, Generator, PersistedState, Project,
};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;

mod filter;
mod job;
mod output;
mod ui;

use filter::FilterIndex;
use job::run_job_captured;
use output::{
    apply_output_scroll, leave_fullscreen_output, sanitize_line, OutputScroll,
};
use ui::{draw, ConfirmAction, DrawState, Mode};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SelectionMove {
    Up,
    Down,
    PageUp,
    PageDown,
    Home,
    End,
}

#[derive(Parser, Debug)]
#[command(name = "lazycmake", about = "TUI for CMake presets")]
struct Cli {
    #[arg(short = 'C', long = "project")]
    project: Option<PathBuf>,
    /// Path to lazycmake config.toml, or a directory containing it (default: .zed/.lazycmake).
    #[arg(long = "config", value_name = "PATH")]
    config: Option<PathBuf>,
}

#[derive(Debug, Clone)]
enum AppEvent {
    OutputLine(String),
    JobFinished(i32),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum JobKind {
    Configure { clean: bool },
    Build { clean: bool },
    Run,
    TestOne,
    TestAll,
}

pub struct App {
    pub project: Project,
    pub capabilities: Capabilities,
    pub preset_names: Vec<String>,
    pub targets: Vec<CodemodelTarget>,
    pub tests: CtestDiscovery,
    pub state: PersistedState,
    pub state_path: PathBuf,
    pub selected_preset: Option<String>,
    pub binary_dir: Option<PathBuf>,
    pub generator: Generator,
    pub mode: Mode,
    pub confirm: Option<ConfirmAction>,
    pub filter_input: String,
    pub output: Vec<String>,
    pub output_scroll: usize,
    pub output_follow: bool,
    pub job_running: bool,
    pub(crate) pending_job: Option<JobKind>,
    pub status_message: String,
    pub preset_filter: FilterIndex,
    pub target_filter: FilterIndex,
    pub test_filter: FilterIndex,
    pub unicode_glyphs: bool,
    /// Full terminal clear before next draw (recovers from leaked subprocess writes).
    pub force_redraw: bool,
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let config_options = ConfigOptions {
        config_path: cli.config,
    };
    let project = Project::discover(cli.project.as_deref(), &config_options)?;
    let capabilities = Capabilities::detect();

    let override_names: Vec<&str> = project
        .config
        .presets
        .overrides
        .keys()
        .map(String::as_str)
        .collect();
    let preset_names = project
        .presets
        .as_ref()
        .map(|p| {
            p.visible_configure_presets_with_overrides(&override_names)
                .into_iter()
                .map(|preset| preset.name.clone())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    let state_path = project.config.resolve_state_path(&project.root)?;
    let legacy_state = project.root.join(".lazycmake/state.json");
    let mut state = PersistedState::load_with_fallbacks(&state_path, &[&legacy_state])?;
    if let Some(default) = project.config.general.default_preset.clone() {
        if state.last_preset.is_none() {
            state.last_preset = Some(default);
        }
    }
    if state.last_preset.is_none() {
        state.last_preset = preset_names.first().cloned();
    }

    let mut app = App {
        preset_filter: FilterIndex::new(preset_names.clone()),
        target_filter: FilterIndex::new(Vec::new()),
        test_filter: FilterIndex::new(Vec::new()),
        project,
        capabilities,
        preset_names,
        targets: Vec::new(),
        tests: CtestDiscovery { cases: Vec::new() },
        state,
        state_path,
        selected_preset: None,
        binary_dir: None,
        generator: Generator::Ninja,
        mode: Mode::Normal,
        confirm: None,
        filter_input: String::new(),
        output: Vec::new(),
        output_scroll: 0,
        output_follow: true,
        job_running: false,
        pending_job: None,
        status_message: String::new(),
        unicode_glyphs: true,
        force_redraw: false,
    };

    if let Some(name) = app.state.last_preset.clone() {
        app.select_preset(&name);
    }
    // Filter is ephemeral (used only while typing); never restore a sticky list filter.
    app.state.presets.filter.clear();
    app.state.targets.filter.clear();
    app.state.tests.filter.clear();

    run_tui(&mut app)?;
    app.state.save_to(&app.state_path)?;
    Ok(())
}

fn run_tui(app: &mut App) -> anyhow::Result<()> {
    let mut stdout = std::io::stdout();
    enable_raw_mode()?;
    stdout.execute(EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let result = run_tui_loop(app, &mut terminal);

    // Always leave alternate screen / raw mode, even if the loop failed —
    // otherwise error text paints over the TUI and the terminal stays broken.
    let _ = disable_raw_mode();
    let _ = terminal.backend_mut().execute(LeaveAlternateScreen);
    result
}

fn run_tui_loop(
    app: &mut App,
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
) -> anyhow::Result<()> {
    let mut job_rx: Option<Receiver<AppEvent>> = None;
    let mut last_draw = Instant::now();

    loop {
        if last_draw.elapsed() >= Duration::from_millis(16) {
            if app.force_redraw {
                terminal.clear()?;
                app.force_redraw = false;
            }
            terminal.draw(|f| {
                draw(
                    f,
                    DrawState {
                        app,
                        preset_visible: app.preset_filter.visible_indices(),
                        target_visible: app.target_filter.visible_indices(),
                        test_visible: &app.test_visible_indices(),
                    },
                )
            })?;
            // Park the cursor in the corner so any accidental tty write cannot
            // mid-line overwrite the status bar (raw-mode \n staircases from there).
            terminal.backend_mut().execute(cursor::MoveTo(0, 0))?;
            terminal.backend_mut().execute(cursor::Hide)?;
            last_draw = Instant::now();
        }

        if let Some(rx) = job_rx.as_ref() {
            let mut finished = None;
            while let Ok(event) = rx.try_recv() {
                match event {
                    AppEvent::OutputLine(line) => app.push_output(line),
                    AppEvent::JobFinished(code) => finished = Some(code),
                }
            }
            if let Some(code) = finished {
                app.job_running = false;
                job_rx = None;
                app.force_redraw = true;
                let is_test_job =
                    matches!(app.pending_job, Some(JobKind::TestOne | JobKind::TestAll));
                let failed = code != 0
                    || (is_test_job
                        && ctest_output_indicates_failure(&app.output.join("\n")));
                if failed {
                    if is_test_job {
                        app.tests.apply_run_output(&app.output.join("\n"));
                    }
                    app.pending_job = None;
                    app.status_message = if code != 0 {
                        format!("Command failed (exit {code})")
                    } else {
                        "Tests failed".into()
                    };
                } else {
                    app.on_job_success();
                    app.status_message.clear();
                }
            }
        }

        if event::poll(Duration::from_millis(16))? {
            if let Event::Key(key) = event::read()? {
                let page = terminal.size().map(|s| s.height as usize).unwrap_or(24).max(8);
                if handle_key(app, key, &mut job_rx, page) {
                    break;
                }
            }
        }
    }

    Ok(())
}

impl App {
    fn push_output(&mut self, line: String) {
        let Some(line) = sanitize_line(&line) else {
            return;
        };
        if self.output.len() >= 10_000 {
            self.output.remove(0);
            self.output_scroll = self.output_scroll.saturating_sub(1);
        }
        self.output.push(line);
    }

    fn scroll_output(&mut self, action: OutputScroll, viewport: usize) {
        apply_output_scroll(
            &mut self.output_scroll,
            &mut self.output_follow,
            self.output.len(),
            viewport,
            action,
        );
    }

    fn select_preset(&mut self, name: &str) {
        self.state.last_preset = Some(name.to_string());
        self.selected_preset = Some(name.to_string());

        if let Some(store) = &self.project.presets {
            if let Some(preset) = store.get(name) {
                self.generator = preset
                    .generator
                    .as_deref()
                    .map(Generator::parse)
                    .unwrap_or(Generator::Ninja);
                self.binary_dir = Some(store.resolve_binary_dir(preset));
            }
        }

        self.refresh_targets_after_preset_change();
        self.refresh_tests();
    }

    fn preset_is_configured(&self) -> bool {
        let Some(binary_dir) = self.binary_dir.as_ref() else {
            return false;
        };
        if !binary_dir.join("CMakeCache.txt").exists() {
            return false;
        }
        self.cache_matches_selected_preset(binary_dir)
    }

    fn cache_matches_selected_preset(&self, binary_dir: &Path) -> bool {
        let Some(name) = self.selected_preset.as_deref() else {
            return false;
        };
        let Some(store) = &self.project.presets else {
            return false;
        };
        let Some(preset) = store.get(name) else {
            return false;
        };
        let override_cfg = self.project.config.preset_override(name);
        cmake_cache_matches_preset(
            binary_dir,
            preset,
            override_cfg,
            &self.project.root,
        )
        .unwrap_or(false)
    }

    /// Select a preset; returns true when configure should run so targets can load.
    fn select_preset_on_enter(&mut self, name: &str) -> bool {
        self.select_preset(name);
        !self.preset_is_configured()
    }

    fn refresh_targets(&mut self) {
        self.refresh_targets_inner(false);
    }

    fn refresh_targets_after_preset_change(&mut self) {
        self.refresh_targets_inner(true);
    }

    fn refresh_targets_inner(&mut self, reselect: bool) {
        let Some(binary_dir) = self.binary_dir.clone() else {
            self.targets.clear();
            self.target_filter = FilterIndex::new(Vec::new());
            return;
        };

        if !binary_dir.join("CMakeCache.txt").exists() {
            self.targets.clear();
            self.target_filter = FilterIndex::new(Vec::new());
            return;
        }

        if !self.cache_matches_selected_preset(&binary_dir) {
            self.targets.clear();
            self.target_filter = FilterIndex::new(Vec::new());
            return;
        }

        let targets = load_targets(&binary_dir, self.generator).unwrap_or_default();
        // Do not call regenerate_codemodel here: it runs a full `cmake <builddir>` on the
        // UI thread. Configure jobs already write the File API query; refresh after success.
        let names: Vec<String> = targets.iter().map(|t| t.name.clone()).collect();
        self.target_filter = FilterIndex::new(names);
        self.targets = targets;
        self.apply_target_selection(reselect);
    }

    /// Select the configured or remembered target after the target list changes.
    fn apply_target_selection(&mut self, reselect: bool) {
        let visible = self.target_filter.visible_indices();
        if visible.is_empty() {
            self.state.targets.selected = 0;
            return;
        }

        if !reselect {
            if let Some(current) = self
                .target_filter
                .selected_item(self.state.targets.selected)
                .cloned()
            {
                if self.targets.iter().any(|t| t.name == current) {
                    if let Some(pos) = visible.iter().position(|&idx| {
                        self.target_filter
                            .items()
                            .get(idx)
                            .is_some_and(|n| n == &current)
                    }) {
                        self.state.targets.selected = pos;
                        return;
                    }
                }
            }
        }

        let candidates = [
            self.project
                .config
                .resolve_default_target(self.selected_preset.as_deref())
                .map(str::to_string),
            self.state.last_target.clone(),
        ];

        for name in candidates.into_iter().flatten() {
            if let Some(pos) = visible.iter().position(|&idx| {
                self.target_filter
                    .items()
                    .get(idx)
                    .is_some_and(|n| n == &name)
            }) {
                self.state.targets.selected = pos;
                return;
            }
        }

        if let Some(pos) = visible.iter().position(|&idx| {
            self.target_filter
                .items()
                .get(idx)
                .is_some_and(|n| n == "all")
        }) {
            self.state.targets.selected = pos;
        } else {
            self.state.targets.selected = 0;
        }
    }

    fn refresh_tests(&mut self) {
        self.tests = CtestDiscovery { cases: Vec::new() };
        self.test_filter = FilterIndex::new(Vec::new());

        let Ok(test_dir) = self.resolve_test_dir() else {
            return;
        };
        if !test_dir.exists() {
            return;
        }
        if let Ok(tests) = CtestDiscovery::discover(&test_dir, self.capabilities.ctest_json) {
            let names: Vec<String> = tests.cases.iter().map(|c| c.name.clone()).collect();
            self.test_filter = FilterIndex::new(names);
            self.tests = tests;
        }
    }

    fn on_job_success(&mut self) {
        let apply_test_output = self
            .pending_job
            .take()
            .is_some_and(|kind| matches!(kind, JobKind::TestOne | JobKind::TestAll));
        self.refresh_targets();
        self.refresh_tests();
        if apply_test_output {
            self.tests.apply_run_output(&self.output.join("\n"));
        }
    }

    /// Binary dir of the configure preset that backs the Tests column.
    fn testing_binary_dir(&self) -> Option<PathBuf> {
        let name = self
            .project
            .config
            .active_testing_preset(self.selected_preset.as_deref())?;
        let store = self.project.presets.as_ref()?;
        let preset = store.get(&name)?;
        Some(store.resolve_binary_dir(preset))
    }

    fn resolve_test_dir(&self) -> anyhow::Result<PathBuf> {
        let selected_binary = self.binary_dir.clone().unwrap_or_else(|| self.project.root.clone());
        self.project
            .config
            .resolve_testing_dir(
                self.selected_preset.as_deref(),
                self.testing_binary_dir().as_deref(),
                &selected_binary,
                &self.project.root,
            )
            .ok_or_else(|| anyhow::anyhow!("no testing directory configured"))
    }

    fn active_testing_preset_name(&self) -> Option<String> {
        self.project
            .config
            .active_testing_preset(self.selected_preset.as_deref())
    }

    fn move_selection(&mut self, action: SelectionMove) {
        let visible_len = self.focused_visible_len();
        if visible_len == 0 {
            return;
        }
        let col = self.column_state_mut();
        col.selected = match action {
            SelectionMove::Up => col.selected.saturating_sub(1),
            SelectionMove::Down => (col.selected + 1).min(visible_len - 1),
            SelectionMove::PageUp => col.selected.saturating_sub(10),
            SelectionMove::PageDown => (col.selected + 10).min(visible_len - 1),
            SelectionMove::Home => 0,
            SelectionMove::End => visible_len - 1,
        };
    }

    fn focused_visible_len(&self) -> usize {
        match self.state.focused_column {
            FocusedColumn::Presets => self.preset_filter.visible_indices().len(),
            FocusedColumn::Targets => self.target_filter.visible_indices().len(),
            FocusedColumn::Tests => self.test_visible_indices().len(),
        }
    }

    fn test_visible_indices(&self) -> Vec<usize> {
        self.test_filter
            .visible_indices()
            .iter()
            .copied()
            .filter(|&idx| {
                if !self.state.tests_failing_only {
                    return true;
                }
                matches!(
                    self.tests.cases.get(idx).map(|c| c.status),
                    Some(lazycmake_core::TestStatus::Fail | lazycmake_core::TestStatus::Skip)
                )
            })
            .collect()
    }

    fn clamp_selection(&mut self) {
        let len = self.focused_visible_len();
        let col = self.column_state_mut();
        if len == 0 {
            col.selected = 0;
        } else if col.selected >= len {
            col.selected = len - 1;
        }
    }

    fn apply_focused_filter(&mut self, query: &str) {
        match self.state.focused_column {
            FocusedColumn::Presets => self.preset_filter.set_query(query),
            FocusedColumn::Targets => self.target_filter.set_query(query),
            FocusedColumn::Tests => self.test_filter.set_query(query),
        }
        self.clamp_selection();
    }

    fn save_focused_filter(&mut self, query: &str) {
        match self.state.focused_column {
            FocusedColumn::Presets => self.state.presets.filter = query.to_string(),
            FocusedColumn::Targets => self.state.targets.filter = query.to_string(),
            FocusedColumn::Tests => self.state.tests.filter = query.to_string(),
        }
    }

    fn focused_selected_name(&self) -> Option<String> {
        match self.state.focused_column {
            FocusedColumn::Presets => self
                .preset_filter
                .selected_item(self.state.presets.selected)
                .cloned(),
            FocusedColumn::Targets => self
                .target_filter
                .selected_item(self.state.targets.selected)
                .cloned(),
            FocusedColumn::Tests => self.selected_test_name(),
        }
    }

    fn select_focused_by_name(&mut self, name: &str) {
        match self.state.focused_column {
            FocusedColumn::Presets => {
                if let Some(pos) = self
                    .preset_filter
                    .visible_indices()
                    .iter()
                    .position(|&idx| self.preset_filter.items().get(idx).map(String::as_str) == Some(name))
                {
                    self.state.presets.selected = pos;
                }
            }
            FocusedColumn::Targets => {
                if let Some(pos) = self
                    .target_filter
                    .visible_indices()
                    .iter()
                    .position(|&idx| self.target_filter.items().get(idx).map(String::as_str) == Some(name))
                {
                    self.state.targets.selected = pos;
                }
            }
            FocusedColumn::Tests => {
                let visible = self.test_visible_indices();
                if let Some(pos) = visible
                    .iter()
                    .position(|&idx| self.tests.cases.get(idx).map(|c| c.name.as_str()) == Some(name))
                {
                    self.state.tests.selected = pos;
                }
            }
        }
    }

    /// Exit filter mode: restore the full list, keep the highlighted item selected.
    fn accept_focused_filter(&mut self) {
        let selected_name = self.focused_selected_name();
        self.filter_input.clear();
        self.apply_focused_filter("");
        self.save_focused_filter("");
        if let Some(name) = selected_name {
            self.select_focused_by_name(&name);
        }
        self.mode = Mode::Normal;
    }

    fn column_state_mut(&mut self) -> &mut ColumnState {
        match self.state.focused_column {
            FocusedColumn::Presets => &mut self.state.presets,
            FocusedColumn::Targets => &mut self.state.targets,
            FocusedColumn::Tests => &mut self.state.tests,
        }
    }

    fn selected_target_name(&self) -> Option<String> {
        self.target_filter
            .selected_item(self.state.targets.selected)
            .cloned()
    }

    fn selected_target(&self) -> Option<&CodemodelTarget> {
        let name = self.selected_target_name()?;
        self.targets.iter().find(|t| t.name == name)
    }

    fn selected_test_name(&self) -> Option<String> {
        let visible = self.test_visible_indices();
        visible
            .get(self.state.tests.selected)
            .and_then(|&idx| self.tests.cases.get(idx))
            .map(|c| c.name.clone())
    }

    fn job_steps(&self, kind: JobKind) -> anyhow::Result<Vec<CommandStep>> {
        let root = self.project.root.clone();
        match kind {
            JobKind::Configure { clean } => {
                let name = self
                    .selected_preset
                    .clone()
                    .ok_or_else(|| anyhow::anyhow!("no preset selected"))?;
                if let Some(dir) = &self.binary_dir {
                    let stale = dir.join("CMakeCache.txt").exists()
                        && !self.cache_matches_selected_preset(dir);
                    if clean || stale {
                        clean_cache(dir)?;
                    }
                }
                if let Some(dir) = &self.binary_dir {
                    ensure_codemodel_query(dir)?;
                }
                let store = self
                    .project
                    .presets
                    .as_ref()
                    .ok_or_else(|| anyhow::anyhow!("no presets"))?;
                let preset = store
                    .get(&name)
                    .ok_or_else(|| anyhow::anyhow!("preset not found: {name}"))?;
                let binary_dir = self
                    .binary_dir
                    .clone()
                    .unwrap_or_else(|| store.resolve_binary_dir(preset));
                let extra_env = self
                    .project
                    .config
                    .resolve_override_env(&name, &self.project.root)?;
                let _overlay = EnvOverlay::apply(&extra_env);
                let cmd = ConfigureCommand::for_preset(
                    preset,
                    &binary_dir,
                    self.project.config.preset_override(&name),
                    &self.project.root,
                )?;
                Ok(vec![CommandStep::new(cmd.argv(&self.project.root), root)
                    .with_env(extra_env)])
            }
            JobKind::Build { clean } => {
                let binary_dir = self
                    .binary_dir
                    .clone()
                    .ok_or_else(|| anyhow::anyhow!("no binary dir"))?;
                let extra_env = self.selected_preset_env()?;
                Ok(vec![CommandStep::new(
                    BuildCommand {
                        binary_dir,
                        target: self.selected_target_name(),
                        clean_first: clean,
                        generator: self.generator,
                        config: Some("Debug".into()),
                    }
                    .argv(),
                    root,
                )
                .with_env(extra_env)])
            }
            JobKind::Run => {
                let binary_dir = self.binary_dir.clone().ok_or_else(|| anyhow::anyhow!("no binary dir"))?;
                let target = self.selected_target().ok_or_else(|| anyhow::anyhow!("no target"))?;
                let path = executable_path(target, &binary_dir, self.generator)
                    .ok_or_else(|| anyhow::anyhow!("not executable"))?;
                let extra_env = self.selected_preset_env()?;
                Ok(vec![CommandStep::new(vec![path.display().to_string()], root)
                    .with_env(extra_env)])
            }
            JobKind::TestOne => {
                let binary_dir = self
                    .testing_binary_dir()
                    .ok_or_else(|| anyhow::anyhow!("no testing binary dir"))?;
                let test_name = self
                    .selected_test_name()
                    .ok_or_else(|| anyhow::anyhow!("no test selected"))?;
                Ok(vec![CommandStep::new(
                    BuildCommand {
                        binary_dir,
                        target: Some(format!("{test_name}_run")),
                        clean_first: false,
                        generator: self.generator,
                        config: Some("Debug".into()),
                    }
                    .argv(),
                    root,
                )])
            }
            JobKind::TestAll => {
                let testing_binary_dir = self
                    .testing_binary_dir()
                    .ok_or_else(|| anyhow::anyhow!("no testing binary dir"))?;
                let test_dir = self.resolve_test_dir()?;
                let preset_name = self
                    .active_testing_preset_name()
                    .ok_or_else(|| anyhow::anyhow!("no testing preset"))?;
                let extra = self.project.config.testing_preset(&preset_name).extra_args;
                Ok(test_all_steps(
                    &testing_binary_dir,
                    &test_dir,
                    &self.project.root,
                    self.generator,
                    extra,
                ))
            }
        }
    }

    fn selected_preset_env(&self) -> anyhow::Result<std::collections::HashMap<String, String>> {
        let Some(name) = &self.selected_preset else {
            return Ok(std::collections::HashMap::new());
        };
        Ok(self
            .project
            .config
            .resolve_override_env(name, &self.project.root)?)
    }
}

fn report_job_error(app: &mut App, err: &anyhow::Error) {
    let msg = format!("Error: {err}");
    app.push_output(msg.clone());
    app.status_message = msg;
    app.force_redraw = true;
}

fn spawn_job(app: &mut App, kind: JobKind) -> Option<Receiver<AppEvent>> {
    let steps = match app.job_steps(kind) {
        Ok(steps) => steps,
        Err(err) => {
            report_job_error(app, &err);
            return None;
        }
    };
    if steps.is_empty() {
        report_job_error(app, &anyhow::anyhow!("empty job"));
        return None;
    }
    app.job_running = true;
    app.pending_job = Some(kind);
    app.force_redraw = true;
    for step in &steps {
        let cwd_display = step.cwd.display();
        app.push_output(format!("$ (cd {cwd_display} && {})", step.args.join(" ")));
    }
    let (tx, rx) = unbounded();
    std::thread::spawn(move || job_thread(steps, tx));
    Some(rx)
}

fn job_thread(steps: Vec<CommandStep>, tx: Sender<AppEvent>) {
    let mut last_code = 0;
    for step in steps {
        let line_tx = tx.clone();
        let code = match run_job_captured(&step.args, &step.cwd, &step.env, move |line| {
            let _ = line_tx.send(AppEvent::OutputLine(line));
        }) {
            Ok(code) => code,
            Err(err) => {
                let _ = tx.send(AppEvent::OutputLine(format!(
                    "Failed to spawn {}: {err}",
                    step.args.first().unwrap_or(&"?".into())
                )));
                1
            }
        };
        last_code = code;
        if code != 0 {
            break;
        }
    }
    let _ = tx.send(AppEvent::JobFinished(last_code));
}

fn handle_key(
    app: &mut App,
    key: KeyEvent,
    job_rx: &mut Option<Receiver<AppEvent>>,
    output_page: usize,
) -> bool {
    if app.mode == Mode::Output {
        return handle_output_key(app, key, output_page);
    }
    if app.mode == Mode::Filter {
        return handle_filter_key(app, key, job_rx);
    }
    if let Some(action) = app.confirm {
        return handle_confirm_key(app, key, action, job_rx);
    }

    match key.code {
        KeyCode::Char('q') => return true,
        KeyCode::Char('?') => app.mode = Mode::Help,
        KeyCode::Esc if app.mode == Mode::Help => app.mode = Mode::Normal,
        KeyCode::Tab => {
            app.state.focused_column = match app.state.focused_column {
                FocusedColumn::Presets => FocusedColumn::Targets,
                FocusedColumn::Targets => FocusedColumn::Tests,
                FocusedColumn::Tests => FocusedColumn::Presets,
            };
        }
        KeyCode::BackTab => {
            app.state.focused_column = match app.state.focused_column {
                FocusedColumn::Presets => FocusedColumn::Tests,
                FocusedColumn::Targets => FocusedColumn::Presets,
                FocusedColumn::Tests => FocusedColumn::Targets,
            };
        }
        KeyCode::Up | KeyCode::Char('k') => app.move_selection(SelectionMove::Up),
        KeyCode::Down | KeyCode::Char('j') => app.move_selection(SelectionMove::Down),
        KeyCode::PageUp => app.move_selection(SelectionMove::PageUp),
        KeyCode::PageDown => app.move_selection(SelectionMove::PageDown),
        KeyCode::Home => app.move_selection(SelectionMove::Home),
        KeyCode::End => app.move_selection(SelectionMove::End),
        KeyCode::Char('/') => {
            app.filter_input.clear();
            app.apply_focused_filter("");
            app.mode = Mode::Filter;
        }
        KeyCode::Char('F') if app.state.focused_column == FocusedColumn::Tests => {
            app.state.tests_failing_only = !app.state.tests_failing_only;
            app.clamp_selection();
        }
        KeyCode::Enter if app.state.focused_column == FocusedColumn::Presets && !app.job_running => {
            if let Some(name) = app
                .preset_filter
                .selected_item(app.state.presets.selected)
                .cloned()
            {
                if app.select_preset_on_enter(&name) {
                    if let Some(rx) = spawn_job(app, JobKind::Configure { clean: false }) {
                        *job_rx = Some(rx);
                    }
                }
            }
        }
        KeyCode::Enter if app.state.focused_column == FocusedColumn::Targets && !app.job_running => {
            if let Some(name) = app.selected_target_name() {
                app.state.last_target = Some(name);
            }
            if let Some(rx) = spawn_job(app, JobKind::Build { clean: false }) {
                *job_rx = Some(rx);
            }
        }
        KeyCode::Enter if app.state.focused_column == FocusedColumn::Tests && !app.job_running => {
            if let Some(rx) = spawn_job(app, JobKind::TestOne) {
                *job_rx = Some(rx);
            }
        }
        KeyCode::Char('c') if !app.job_running => {
            if let Some(rx) = spawn_job(app, JobKind::Configure { clean: false }) {
                *job_rx = Some(rx);
            }
        }
        KeyCode::Char('C') if !app.job_running => app.confirm = Some(ConfirmAction::CleanConfigure),
        KeyCode::Char('b') if !app.job_running => {
            if let Some(rx) = spawn_job(app, JobKind::Build { clean: false }) {
                *job_rx = Some(rx);
            }
        }
        KeyCode::Char('B') if !app.job_running => app.confirm = Some(ConfirmAction::CleanBuild),
        KeyCode::Char('r') if !app.job_running => {
            if let Some(rx) = spawn_job(app, JobKind::Run) {
                *job_rx = Some(rx);
            }
        }
        KeyCode::Char('t') if !app.job_running => {
            if let Some(rx) = spawn_job(app, JobKind::TestOne) {
                *job_rx = Some(rx);
            }
        }
        KeyCode::Char('T') if !app.job_running => {
            if let Some(rx) = spawn_job(app, JobKind::TestAll) {
                *job_rx = Some(rx);
            }
        }
        KeyCode::Char('o') => app.mode = Mode::Output,
        _ => {}
    }

    false
}

fn handle_output_key(app: &mut App, key: KeyEvent, page: usize) -> bool {
    match key.code {
        KeyCode::Esc | KeyCode::Char('o') => {
            leave_fullscreen_output(&mut app.output_follow);
            app.mode = Mode::Normal;
        }
        KeyCode::Char('q') => return true,
        KeyCode::Char('f') => {
            app.output_follow = !app.output_follow;
            if app.output_follow {
                app.output_scroll = app
                    .output
                    .len()
                    .saturating_sub(page.max(1));
            }
        }
        KeyCode::Up | KeyCode::Char('k') => app.scroll_output(OutputScroll::Up, page),
        KeyCode::Down | KeyCode::Char('j') => app.scroll_output(OutputScroll::Down, page),
        KeyCode::PageUp => app.scroll_output(OutputScroll::PageUp, page),
        KeyCode::PageDown => app.scroll_output(OutputScroll::PageDown, page),
        KeyCode::Home | KeyCode::Char('g') => app.scroll_output(OutputScroll::Home, page),
        KeyCode::End | KeyCode::Char('G') => app.scroll_output(OutputScroll::End, page),
        _ => {}
    }
    false
}

fn handle_confirm_key(
    app: &mut App,
    key: KeyEvent,
    action: ConfirmAction,
    job_rx: &mut Option<Receiver<AppEvent>>,
) -> bool {
    match key.code {
        KeyCode::Char('y') | KeyCode::Char('Y') => {
            app.confirm = None;
            let kind = match action {
                ConfirmAction::CleanConfigure => JobKind::Configure { clean: true },
                ConfirmAction::CleanBuild => JobKind::Build { clean: true },
            };
            if let Some(rx) = spawn_job(app, kind) {
                *job_rx = Some(rx);
            }
        }
        KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => app.confirm = None,
        _ => {}
    }
    false
}

fn handle_filter_key(
    app: &mut App,
    key: KeyEvent,
    job_rx: &mut Option<Receiver<AppEvent>>,
) -> bool {
    match key.code {
        KeyCode::Esc => {
            app.filter_input.clear();
            app.apply_focused_filter("");
            app.save_focused_filter("");
            app.mode = Mode::Normal;
        }
        KeyCode::Enter => {
            app.accept_focused_filter();
            match app.state.focused_column {
                FocusedColumn::Presets if !app.job_running => {
                    if let Some(name) = app
                        .preset_filter
                        .selected_item(app.state.presets.selected)
                        .cloned()
                    {
                        if app.select_preset_on_enter(&name) {
                            if let Some(rx) = spawn_job(app, JobKind::Configure { clean: false }) {
                                *job_rx = Some(rx);
                            }
                        }
                    }
                }
                FocusedColumn::Targets if !app.job_running => {
                    if let Some(name) = app.selected_target_name() {
                        app.state.last_target = Some(name);
                    }
                    if let Some(rx) = spawn_job(app, JobKind::Build { clean: false }) {
                        *job_rx = Some(rx);
                    }
                }
                FocusedColumn::Tests if !app.job_running => {
                    if let Some(rx) = spawn_job(app, JobKind::TestOne) {
                        *job_rx = Some(rx);
                    }
                }
                _ => {}
            }
        }
        KeyCode::Up => app.move_selection(SelectionMove::Up),
        KeyCode::Down => app.move_selection(SelectionMove::Down),
        KeyCode::Backspace => {
            app.filter_input.pop();
            let query = app.filter_input.clone();
            app.apply_focused_filter(&query);
        }
        KeyCode::Char(ch) => {
            app.filter_input.push(ch);
            let query = app.filter_input.clone();
            app.apply_focused_filter(&query);
        }
        _ => {}
    }
    false
}
