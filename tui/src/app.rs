use crate::access::{policy_path, AccessPolicy, AccessState, Scope};
use crate::agents_state::{AgentsScreenState, JobDetailState};
use crate::composer::Composer;
use crate::jobs::{self, Job, JobsHandle};
use crate::kernel::{KernelConfig, KernelEvent, KernelHandle, SessionRow};
use crate::keys::{action_for, Action};
use crate::overlays::picker::{PickItem, Picker, PickerOutcome};
use crate::overlays::{help_lines, render_text_panel, Overlay, PickerKind};
use crate::providers::{kernel_binary, load_catalog, ProviderInfo};
use crate::render::agents::{render_agents_list, render_job_detail, AgentsListInfo};
use crate::render::chrome::{
    render_activity, render_header, render_status, HeaderInfo, StatusInfo, SPINNER, SPINNER_ASCII,
};
use crate::render::markdown::Highlighter;
use crate::render::scrollbar::render_scrollbar;
use crate::render::theme::{IconMode, Theme, ThemeKind};
use crate::render::transcript::RenderCache;
use crate::skills::{self, expand_skill_prompt, SkillEntry};
use crate::state::{state_path, PersistedState};
use crate::status::{self, ContextInfo, Line1Input, Line2Input};
use crate::timeline::timeline_blocks;
use crate::transcript::{BannerKind, SourceEntry, SourceKind, Transcript, VerifyClaim, VerifyStatus};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseEvent, MouseEventKind};
use ratatui::layout::{Constraint, Flex, Layout, Margin, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block as FrameBlock, BorderType, Borders, Paragraph};
use ratatui::Frame;
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const SLASH_COMMANDS: &[(&str, &str)] = &[
    ("provider", "list or switch provider"),
    ("model", "list or switch model"),
    ("theme", "preview or switch the color theme"),
    ("weight", "switch between thin and regular text"),
    ("icons", "switch between nerd, unicode, and ascii glyphs"),
    ("session", "start, list, or resume sessions"),
    ("add-dir", "grant sandbox write access"),
    ("clear", "clear the transcript view"),
    ("events", "open the raw Lazar event feed"),
    ("hooks", "toggle hook trace visibility"),
    ("debug", "toggle debug/verbose view (hooks, internal events, full detail)"),
    ("help", "open the help panel"),
];

#[derive(Debug, Clone)]
pub struct LaunchOptions {
    pub lazar_home: PathBuf,
    pub cwd: PathBuf,
    pub add_dirs: Vec<PathBuf>,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub session_id: Option<String>,
    pub no_sandbox: bool,
    pub kernel: Option<PathBuf>,
    pub thin: Option<bool>,
}

pub struct App {
    options: LaunchOptions,
    kernel_binary: PathBuf,
    kernel: KernelHandle,
    providers: Vec<ProviderInfo>,
    skills: Vec<SkillEntry>,
    access: AccessState,
    persisted: PersistedState,
    provider: Option<String>,
    model: Option<String>,
    session_id: Option<String>,
    pub transcript: Transcript,
    composer: Composer,
    theme: Theme,
    highlighter: Highlighter,
    render_cache: RenderCache,
    overlay: Option<Overlay>,
    overlay_scroll: u16,
    event_feed: Vec<String>,
    status: String,
    notice: String,
    ready: bool,
    effective_sandbox_off: bool,
    turn_running: bool,
    queued: Option<String>,
    scroll_from_bottom: usize,
    previous_visual_lines: usize,
    /// Whether the transcript needed the narrower (scrollbar-reserving)
    /// width last frame. Rendering the transcript is expensive (markdown
    /// re-parse, syntax re-highlight per block), so we pick the width up
    /// front from this remembered decision instead of always rendering once
    /// at full width and, on overflow, rendering the whole transcript again
    /// at width-2 every single frame.
    needs_scrollbar: bool,
    spinner_frame: usize,
    input_tokens: u64,
    output_tokens: u64,
    last_theme_refresh: Instant,
    turn_started: Option<Instant>,
    /// The kernel's own username/host, so the status bar doesn't shell out
    /// on every frame.
    user: String,
    host: String,
    /// Most recent `context` event: token usage against the model's window.
    context: Option<ContextInfo>,
    /// Background sub-agent jobs, polled from the kernel (`<lazar> jobs`).
    jobs: Vec<Job>,
    jobs_handle: Option<JobsHandle>,
    /// The kernel's latest `phase` event for the running turn, reset once it
    /// finishes.
    phase: Option<String>,
    /// When the last assistant text delta arrived, for the "Writing" phase
    /// of the activity indicator.
    last_text_delta_at: Option<Instant>,
    /// The `notice` text as of the last `tick` — lets `tick` detect a change
    /// made anywhere (there's no single setter) and start that notice's own
    /// 3s countdown, without every call site having to manage a timestamp.
    last_notice: String,
    notice_set_at: Instant,
    /// mtime+inode of this process's own executable and of the kernel binary
    /// it launched, recorded at startup so a later rebuild/reinstall of
    /// either can be detected (see `check_stale_binary`).
    exe_fingerprint: Option<status::FileFingerprint>,
    kernel_fingerprint: Option<status::FileFingerprint>,
    last_binary_check: Instant,
    /// Set once a rebuild is detected and never cleared — the notice is
    /// meant to persist until the user actually restarts.
    stale_binary_notice: Option<String>,
    /// The full-screen Agents view (Left from the composer, or Enter from
    /// the focused footer panel), and its own job-detail sub-screen.
    /// `None` shows the ordinary chat view.
    agents_screen: Option<AgentsScreenState>,
    /// Down from an empty composer focuses the footer agents panel: this is
    /// the selected row (capped at the panel's own 4-row max). `None` means
    /// the composer has focus, as usual.
    footer_agent_focus: Option<usize>,
}

impl App {
    pub fn new(mut options: LaunchOptions) -> Self {
        let state_file = state_path(&options.lazar_home);
        let persisted = PersistedState::load(&state_file);
        let kernel_binary = options
            .kernel
            .clone()
            .unwrap_or_else(|| kernel_binary(&options.lazar_home));
        // The kernel is the source of truth for installed skills (it's what
        // resolves `$name`); INDEX.md parsing is only a fallback.
        let skills = skills::load(&kernel_binary, &options.lazar_home);
        let (providers, catalog_error) = match load_catalog(&kernel_binary, &options.lazar_home) {
            Ok(rows) => (rows, None),
            Err(err) => (Vec::new(), Some(err)),
        };
        let provider = options
            .provider
            .clone()
            .or_else(|| {
                std::env::var("LAZAR_PROVIDER")
                    .ok()
                    .filter(|v| !v.is_empty())
            })
            .or_else(|| persisted.provider.clone())
            .or_else(|| {
                providers
                    .iter()
                    .find(|p| p.key_available)
                    .map(|p| p.id.clone())
            });
        let model = options
            .model
            .clone()
            .or_else(|| std::env::var("LAZAR_MODEL").ok().filter(|v| !v.is_empty()))
            .or_else(|| (provider == persisted.provider).then(|| persisted.model.clone()).flatten())
            .or_else(|| {
                provider.as_deref().and_then(|id| {
                    providers
                        .iter()
                        .find(|p| p.id == id)
                        .map(|p| p.default_model.clone())
                })
            });
        let session_id = options
            .session_id
            .clone()
            .or_else(|| persisted.session_id.clone());
        options.cwd = options.cwd.canonicalize().unwrap_or(options.cwd);
        let mut access = AccessState {
            policy: AccessPolicy::load(&policy_path(&options.lazar_home)),
            session_dirs: Vec::new(),
            launch_dirs: options.add_dirs.clone(),
        };
        access.launch_dirs.retain(|p| p.exists());
        let config = Self::make_config(
            &options,
            &kernel_binary,
            provider.clone(),
            model.clone(),
            session_id.clone(),
            access.additional_directories(),
        );
        let kernel = KernelHandle::spawn(config);
        let status = catalog_error.unwrap_or_else(|| "connecting".into());
        let thin = options
            .thin
            .or_else(font_weight_from_env)
            .unwrap_or(persisted.thin);
        let debug = debug_from_env() || persisted.debug;
        let icon_mode = IconMode::resolve(persisted.icons.as_deref());
        let base_theme = Theme::nerv_with_icons(icon_mode).with_thin(thin);
        let theme = std::env::var("LAZARTUI_THEME")
            .ok()
            .filter(|name| !name.trim().is_empty())
            .as_deref()
            .and_then(|name| Theme::from_name(name, base_theme.icon_mode))
            .or_else(|| {
                persisted
                    .theme
                    .as_deref()
                    .and_then(|name| Theme::from_name(name, base_theme.icon_mode))
            })
            .unwrap_or_else(|| base_theme.clone())
            .with_thin(thin);
        let (user, host) = current_user_host();
        let jobs_handle = Some(JobsHandle::spawn(kernel_binary.clone(), options.lazar_home.clone()));
        let exe_fingerprint = std::env::current_exe().ok().as_deref().and_then(fingerprint);
        let kernel_fingerprint = fingerprint(&kernel_binary);
        Self {
            options,
            kernel_binary,
            kernel,
            providers,
            skills,
            access,
            composer: Composer::new(persisted.composer_history.clone()),
            persisted,
            provider,
            model,
            session_id,
            transcript: {
                let mut t = Transcript::new();
                t.debug = debug;
                t
            },
            theme,
            highlighter: Highlighter::new(),
            render_cache: RenderCache::default(),
            overlay: None,
            overlay_scroll: 0,
            event_feed: Vec::new(),
            status,
            notice: String::new(),
            ready: false,
            effective_sandbox_off: false,
            turn_running: false,
            queued: None,
            scroll_from_bottom: 0,
            previous_visual_lines: 0,
            needs_scrollbar: false,
            spinner_frame: 0,
            input_tokens: 0,
            output_tokens: 0,
            last_theme_refresh: Instant::now(),
            turn_started: None,
            user,
            host,
            context: None,
            jobs: Vec::new(),
            jobs_handle,
            phase: None,
            last_text_delta_at: None,
            last_notice: String::new(),
            notice_set_at: Instant::now(),
            exe_fingerprint,
            kernel_fingerprint,
            last_binary_check: Instant::now(),
            stale_binary_notice: None,
            agents_screen: None,
            footer_agent_focus: None,
        }
    }

    fn make_config(
        options: &LaunchOptions,
        binary: &Path,
        provider: Option<String>,
        model: Option<String>,
        session_id: Option<String>,
        additional_dirs: Vec<PathBuf>,
    ) -> KernelConfig {
        KernelConfig {
            binary: binary.to_path_buf(),
            lazar_home: options.lazar_home.clone(),
            cwd: options.cwd.clone(),
            additional_dirs,
            provider,
            model,
            session_id,
            no_sandbox: options.no_sandbox,
        }
    }

    fn current_config(&self) -> KernelConfig {
        Self::make_config(
            &self.options,
            &self.kernel_binary,
            self.provider.clone(),
            self.model.clone(),
            self.session_id.clone(),
            self.access.additional_directories(),
        )
    }

    pub fn tick(&mut self) {
        self.spinner_frame = self.spinner_frame.wrapping_add(1);
        if self.last_theme_refresh.elapsed() >= Duration::from_secs(2) {
            let refreshed = self.theme.refresh_auto();
            if refreshed.cache_key() != self.theme.cache_key() {
                self.theme = refreshed;
            }
            self.last_theme_refresh = Instant::now();
        }
        while let Ok(event) = self.kernel.events.try_recv() {
            self.handle_kernel_event(event);
        }
        if let Some(handle) = &self.jobs_handle {
            if let Some(jobs) = handle.try_recv_latest() {
                self.jobs = jobs;
            }
        }
        self.tick_agents_screen();
        self.expire_notice();
        self.check_stale_binary();
    }

    /// While the Agents screen is open: drain its own (24h-window) jobs
    /// poll, and — while a job detail is open — drain its `jobs --show`
    /// fetch and, if the job is still running/starting, kick off a refresh
    /// every 2s (never while one is already in flight).
    fn tick_agents_screen(&mut self) {
        let Some(screen) = self.agents_screen.as_mut() else {
            return;
        };
        if let Some(jobs) = screen.jobs_handle.try_recv_latest() {
            screen.jobs = jobs;
            let total = jobs::selectable_jobs(&screen.jobs).len();
            screen.selected = if total == 0 { 0 } else { screen.selected.min(total - 1) };
        }
        let Some(detail) = screen.detail.as_mut() else {
            return;
        };
        if let Ok(result) = detail.fetch.try_recv() {
            detail.loading = false;
            match result {
                Ok(value) => {
                    if let Some(job) = value.get("job") {
                        if let Some(status) = job.get("status").and_then(Value::as_str) {
                            detail.status = status.to_string();
                        }
                        if let Some(elapsed) = job.get("elapsed_s").and_then(Value::as_u64) {
                            detail.elapsed_s = elapsed;
                        }
                        if let Some(task) = job.get("task").and_then(Value::as_str) {
                            detail.task = task.to_string();
                        }
                    }
                    detail.blocks = timeline_blocks(value.get("timeline").unwrap_or(&Value::Null));
                    detail.error = None;
                }
                Err(err) => detail.error = Some(err),
            }
        }
        let running = matches!(detail.status.as_str(), "running" | "starting");
        if running && !detail.loading && detail.last_poll.elapsed() >= Duration::from_secs(2) {
            detail.loading = true;
            detail.last_poll = Instant::now();
            detail.fetch =
                jobs::fetch_job_show(self.kernel_binary.clone(), self.options.lazar_home.clone(), detail.job_id.clone());
        }
    }

    /// The user keeps running stale builds: every ~5s, re-stat this
    /// process's own executable and the kernel binary it launched. If
    /// either now looks rebuilt (see `status::binary_replaced`), show a
    /// persistent restart notice once and never again this session.
    fn check_stale_binary(&mut self) {
        if self.stale_binary_notice.is_some() {
            return;
        }
        if self.last_binary_check.elapsed() < Duration::from_secs(5) {
            return;
        }
        self.last_binary_check = Instant::now();
        let exe_changed = status::binary_replaced(
            self.exe_fingerprint,
            std::env::current_exe().ok().as_deref().and_then(fingerprint),
        );
        let kernel_changed =
            status::binary_replaced(self.kernel_fingerprint, fingerprint(&self.kernel_binary));
        if exe_changed || kernel_changed {
            self.stale_binary_notice =
                Some("new lazar build installed — restart lazartui to use it".to_string());
        }
    }

    /// Notices are transient: ~3s after one is set (detected here, since
    /// there's no single setter — see `last_notice`), or on the next
    /// keypress (see `key`), whichever comes first.
    fn expire_notice(&mut self) {
        if self.notice != self.last_notice {
            self.last_notice = self.notice.clone();
            self.notice_set_at = Instant::now();
        } else if !self.notice.is_empty() && self.notice_set_at.elapsed() >= Duration::from_secs(3) {
            self.notice.clear();
            self.last_notice.clear();
        }
    }

    pub fn paste(&mut self, text: &str) {
        if self.overlay.is_none() {
            self.composer.insert_str(text);
        }
    }

    pub fn mouse(&mut self, event: MouseEvent) {
        let picker_event = if let Some(Overlay::Picker {
            kind,
            picker,
            payload,
        }) = self.overlay.as_mut()
        {
            if !picker.contains(event.column, event.row)
                && matches!(
                    event.kind,
                    MouseEventKind::ScrollUp | MouseEventKind::ScrollDown
                )
            {
                None
            } else {
                let outcome = picker.handle_mouse(event);
                Some((*kind, payload.clone(), outcome, picker.selected_id()))
            }
        } else {
            None
        };
        if let Some((kind, payload, outcome, selected)) = picker_event {
            if kind == PickerKind::Theme {
                if let Some(id) = selected {
                    self.preview_theme(&id);
                }
            }
            if let PickerOutcome::Chosen(id) = outcome {
                self.overlay = None;
                self.apply_pick(kind, id, payload);
                if matches!(
                    kind,
                    PickerKind::Theme
                        | PickerKind::Weight
                        | PickerKind::Icons
                        | PickerKind::Model
                        | PickerKind::Provider
                        | PickerKind::Session
                ) {
                    self.composer.clear();
                }
            }
            return;
        }
        match event.kind {
            MouseEventKind::ScrollUp => {
                self.scroll_from_bottom = self.scroll_from_bottom.saturating_add(3)
            }
            MouseEventKind::ScrollDown => {
                self.scroll_from_bottom = self.scroll_from_bottom.saturating_sub(3)
            }
            _ => {}
        }
    }

    /// Returns true when the application should quit.
    pub fn key(&mut self, key: KeyEvent) -> bool {
        // Notices are transient (see `tick`'s 3s expiry): any keypress also
        // dismisses one immediately, whichever comes first. A handler below
        // may set a fresh one in the same call, which is fine — it starts
        // its own 3s countdown next tick.
        if !self.notice.is_empty() {
            self.notice.clear();
            self.last_notice.clear();
        }
        if self.overlay.is_some() {
            return self.overlay_key(key);
        }
        if self.agents_screen.is_some() {
            return self.agents_key(key);
        }
        if self.footer_agent_focus.is_some() {
            return self.footer_focus_key(key);
        }
        if key.code == KeyCode::Left
            && key.modifiers.is_empty()
            && should_open_agents_screen(self.composer.is_empty(), self.composer.cursor_col())
        {
            self.open_agents_screen();
            return false;
        }
        if key.code == KeyCode::Down
            && key.modifiers.is_empty()
            && self.composer.is_empty()
            && !self.jobs.is_empty()
        {
            self.footer_agent_focus = Some(0);
            return false;
        }
        match action_for(&key, self.turn_running) {
            Action::Quit => return true,
            Action::CancelTurn => {
                let _ = self.kernel.cancel();
                self.status = "cancelling".into();
            }
            Action::Send => self.submit_composer(false),
            Action::Newline => self.composer.newline(),
            Action::Escape => self.composer.clear(),
            Action::OpenModelPicker => self.open_model_picker(),
            Action::OpenProviderPicker => self.open_provider_picker(),
            Action::ToggleToolFold => self.transcript.toggle_tool_fold(),
            Action::ToggleHooks => {
                self.transcript.show_hooks = !self.transcript.show_hooks;
                self.notice = if self.transcript.show_hooks {
                    "hook trace visible"
                } else {
                    "hook trace hidden"
                }
                .into();
            }
            Action::ToggleDebug => self.toggle_debug(),
            Action::ToggleEventFeed => self.open_text_overlay(Overlay::EventFeed),
            Action::ToggleSubagents => self.open_text_overlay(Overlay::Subagents),
            Action::Steer => self.submit_composer(true),
            Action::DropQueued => {
                self.queued = None;
                if let Err(err) = self.kernel.drop_queued() {
                    self.error(err);
                } else {
                    self.notice = "queued message dropped".into();
                }
            }
            Action::DropChip => self.notice = "no attachment chip to drop".into(),
            Action::Voice => self.notice = "voice input is not installed in this build".into(),
            Action::DumpTranscript => self.dump_transcript(),
            Action::ClearView => self.transcript.clear(),
            Action::Help => self.open_text_overlay(Overlay::Help),
            Action::Complete => self.complete_token(),
            Action::ScrollUp => self.scroll_from_bottom = self.scroll_from_bottom.saturating_add(1),
            Action::ScrollDown => {
                self.scroll_from_bottom = self.scroll_from_bottom.saturating_sub(1)
            }
            Action::PageUp => self.scroll_from_bottom = self.scroll_from_bottom.saturating_add(12),
            Action::PageDown => {
                self.scroll_from_bottom = self.scroll_from_bottom.saturating_sub(12)
            }
            Action::ScrollTop => self.scroll_from_bottom = usize::MAX / 4,
            Action::ScrollBottom => self.scroll_from_bottom = 0,
            Action::Compose => self.composer_key(key),
        }
        false
    }

    fn composer_key(&mut self, key: KeyEvent) {
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            return;
        }
        match key.code {
            KeyCode::Char(c) => {
                self.composer.insert_char(c);
                if c == '/' && self.composer.text().trim_start() == "/" {
                    self.open_slash_picker("");
                } else if c == '@' && self.composer.current_token() == "@" {
                    self.open_path_picker("");
                } else if c == '$' && self.composer.current_token() == "$" {
                    self.open_skill_picker("");
                }
            }
            KeyCode::Backspace => self.composer.backspace(),
            KeyCode::Delete => self.composer.delete(),
            KeyCode::Left => self.composer.left(),
            KeyCode::Right => self.composer.right(),
            KeyCode::Up => self.composer.up(),
            KeyCode::Down => self.composer.down(),
            KeyCode::Home => self.composer.home(),
            KeyCode::End => self.composer.end(),
            _ => {}
        }
    }

    fn overlay_key(&mut self, key: KeyEvent) -> bool {
        if matches!(key.code, KeyCode::Char('q')) && key.modifiers.contains(KeyModifiers::CONTROL) {
            return true;
        }
        let picker_event = if let Some(Overlay::Picker {
            kind,
            picker,
            payload,
        }) = self.overlay.as_mut()
        {
            let kind = *kind;
            let prefix = match kind {
                PickerKind::Slash => Some('/'),
                PickerKind::Path => Some('@'),
                PickerKind::Skill => Some('$'),
                _ => None,
            };
            let remove_trigger = prefix.is_some()
                && matches!(key.code, KeyCode::Backspace)
                && picker.query().is_empty();
            let finish_token = prefix.is_some() && matches!(key.code, KeyCode::Char(' '));
            let outcome = if remove_trigger || finish_token {
                PickerOutcome::Pending
            } else {
                picker.handle_key(&key)
            };
            let synced_token = prefix.map(|prefix| {
                format!(
                    "{prefix}{}{}",
                    picker.query(),
                    if finish_token { " " } else { "" }
                )
            });
            let selected = picker.selected_id();
            Some((
                kind,
                payload.clone(),
                outcome,
                selected,
                synced_token,
                remove_trigger,
                finish_token,
            ))
        } else {
            None
        };
        if let Some((
            kind,
            payload,
            outcome,
            selected,
            synced_token,
            remove_trigger,
            finish_token,
        )) = picker_event
        {
            if kind == PickerKind::Theme {
                if let Some(id) = selected.as_deref() {
                    self.preview_theme(id);
                }
            }
            if remove_trigger {
                self.composer.backspace();
                self.overlay = None;
                return false;
            }
            if let Some(token) = synced_token {
                self.composer.replace_current_token(&token);
            }
            if finish_token {
                self.overlay = None;
                if kind == PickerKind::Slash {
                    match selected.as_deref() {
                        Some("theme") => self.open_theme_picker(),
                        Some("weight") => self.open_weight_picker(),
                        Some("icons") => self.open_icons_picker(),
                        _ => {}
                    }
                }
                return false;
            }
            // These pickers act like commands (reached directly via a
            // hotkey, or via a "/theme "-style slash token that opened a
            // second, unprefixed picker): once one resolves, any leftover
            // command text in the composer is stale and would otherwise
            // linger (e.g. "/theme " staying after the theme was picked).
            let is_command_picker = matches!(
                kind,
                PickerKind::Theme
                    | PickerKind::Weight
                    | PickerKind::Icons
                    | PickerKind::Model
                    | PickerKind::Provider
                    | PickerKind::Session
            );
            match outcome {
                PickerOutcome::Pending => return false,
                PickerOutcome::Cancelled => {
                    self.overlay = None;
                    if kind == PickerKind::Theme {
                        if let Some(previous) = payload.as_deref() {
                            self.preview_theme(previous);
                        }
                    }
                    if is_command_picker {
                        self.composer.clear();
                    }
                    return false;
                }
                PickerOutcome::Chosen(id) => {
                    self.overlay = None;
                    self.apply_pick(kind, id, payload);
                    if is_command_picker {
                        self.composer.clear();
                    }
                    return false;
                }
            }
        }
        match key.code {
            KeyCode::Esc => self.overlay = None,
            KeyCode::Up => self.overlay_scroll = self.overlay_scroll.saturating_sub(1),
            KeyCode::Down => self.overlay_scroll = self.overlay_scroll.saturating_add(1),
            KeyCode::PageUp => self.overlay_scroll = self.overlay_scroll.saturating_sub(10),
            KeyCode::PageDown => self.overlay_scroll = self.overlay_scroll.saturating_add(10),
            _ => {}
        }
        false
    }

    /// Down from an empty composer, with the footer agents panel visible:
    /// Up/Down move the selection, Enter opens that job's detail (via the
    /// full Agents screen), Esc or Up past the top returns to the composer.
    /// Any other key drops focus and falls through to ordinary composing.
    fn footer_focus_key(&mut self, key: KeyEvent) -> bool {
        let visible = self.jobs.len().min(4);
        match key.code {
            KeyCode::Down => {
                if visible > 0 {
                    let next = self.footer_agent_focus.unwrap_or(0) + 1;
                    self.footer_agent_focus = Some(next.min(visible - 1));
                }
                false
            }
            KeyCode::Up => {
                match self.footer_agent_focus {
                    Some(0) | None => self.footer_agent_focus = None,
                    Some(i) => self.footer_agent_focus = Some(i - 1),
                }
                false
            }
            KeyCode::Enter => {
                if let Some(job_id) = self
                    .footer_agent_focus
                    .and_then(|i| self.jobs.get(i))
                    .map(|job| job.job_id.clone())
                {
                    self.footer_agent_focus = None;
                    self.open_agents_screen();
                    self.open_job_detail(job_id);
                }
                false
            }
            KeyCode::Esc => {
                self.footer_agent_focus = None;
                false
            }
            _ => {
                self.footer_agent_focus = None;
                self.key(key)
            }
        }
    }

    /// Key handling for the full-screen Agents view and its job-detail
    /// sub-screen (see `agents_state::AgentsScreenState`).
    fn agents_key(&mut self, key: KeyEvent) -> bool {
        let in_detail = matches!(&self.agents_screen, Some(s) if s.detail.is_some());
        match key.code {
            KeyCode::Esc if in_detail => {
                if let Some(screen) = self.agents_screen.as_mut() {
                    screen.detail = None;
                }
            }
            KeyCode::Esc | KeyCode::Right if !in_detail => {
                self.agents_screen = None;
            }
            KeyCode::Up if in_detail => self.detail_scroll(|s| s.saturating_sub(1)),
            KeyCode::Down if in_detail => self.detail_scroll(|s| s.saturating_add(1)),
            KeyCode::PageUp if in_detail => self.detail_scroll(|s| s.saturating_sub(10)),
            KeyCode::PageDown if in_detail => self.detail_scroll(|s| s.saturating_add(10)),
            KeyCode::Enter if in_detail => self.toggle_detail_tools(),
            KeyCode::Char('t')
                if in_detail && key.modifiers.contains(KeyModifiers::CONTROL) =>
            {
                self.toggle_detail_tools()
            }
            KeyCode::Up if !in_detail => {
                if let Some(screen) = self.agents_screen.as_mut() {
                    screen.selected = screen.selected.saturating_sub(1);
                }
            }
            KeyCode::Down if !in_detail => {
                if let Some(screen) = self.agents_screen.as_mut() {
                    let total = jobs::selectable_jobs(&screen.jobs).len();
                    if total > 0 {
                        screen.selected = (screen.selected + 1).min(total - 1);
                    }
                }
            }
            KeyCode::Enter if !in_detail => {
                let job_id = self.agents_screen.as_ref().and_then(|screen| {
                    jobs::selectable_jobs(&screen.jobs)
                        .get(screen.selected)
                        .map(|job| job.job_id.clone())
                });
                if let Some(job_id) = job_id {
                    self.open_job_detail(job_id);
                }
            }
            _ => {}
        }
        false
    }

    fn detail_scroll(&mut self, f: impl FnOnce(u16) -> u16) {
        if let Some(detail) = self.agents_screen.as_mut().and_then(|s| s.detail.as_mut()) {
            detail.scroll = f(detail.scroll);
        }
    }

    fn toggle_detail_tools(&mut self) {
        if let Some(detail) = self.agents_screen.as_mut().and_then(|s| s.detail.as_mut()) {
            detail.tools_unfolded = !detail.tools_unfolded;
        }
    }

    /// Opens the full-screen Agents view (Left from an empty/at-column-0
    /// composer, or Enter from the focused footer panel), starting its own
    /// 24h-window `jobs` poll. A no-op if it's already open.
    fn open_agents_screen(&mut self) {
        if self.agents_screen.is_some() {
            return;
        }
        let jobs_handle = JobsHandle::spawn_with_args(
            self.kernel_binary.clone(),
            self.options.lazar_home.clone(),
            Duration::from_secs(2),
            vec!["--since".into(), "86400".into()],
        );
        self.agents_screen = Some(AgentsScreenState {
            jobs_handle,
            jobs: self.jobs.clone(),
            selected: 0,
            detail: None,
        });
    }

    /// Enter on an Agents-screen row: fetches `jobs --show <id>` off the UI
    /// thread and opens the job-detail sub-screen. A no-op if the job isn't
    /// known to the (already open) Agents screen.
    fn open_job_detail(&mut self, job_id: String) {
        let Some(screen) = self.agents_screen.as_mut() else {
            return;
        };
        let Some(job) = jobs::selectable_jobs(&screen.jobs)
            .into_iter()
            .find(|job| job.job_id == job_id)
            .cloned()
        else {
            return;
        };
        let fetch = jobs::fetch_job_show(
            self.kernel_binary.clone(),
            self.options.lazar_home.clone(),
            job_id.clone(),
        );
        screen.detail = Some(JobDetailState {
            job_id,
            task: job.task,
            status: job.status,
            elapsed_s: job.elapsed_s,
            fetch,
            blocks: Vec::new(),
            loading: true,
            error: None,
            tools_unfolded: false,
            scroll: 0,
            last_poll: Instant::now(),
        });
    }

    fn apply_pick(&mut self, kind: PickerKind, id: String, payload: Option<String>) {
        match kind {
            PickerKind::Provider => self.switch_provider(id),
            PickerKind::Model => self.switch_model(id),
            PickerKind::Session => self.load_session(id),
            PickerKind::Theme => self.apply_theme(&id),
            PickerKind::Weight => self.apply_weight(&id),
            PickerKind::Icons => self.apply_icons(&id),
            PickerKind::Slash => {
                self.composer.replace_current_token(&format!("/{id} "));
                match id.as_str() {
                    "theme" => self.open_theme_picker(),
                    "weight" => self.open_weight_picker(),
                    "icons" => self.open_icons_picker(),
                    _ => {}
                }
            }
            PickerKind::Path => self.composer.replace_current_token(&format!("@{id}")),
            // A trailing space, unlike @-paths: `$name` is a complete
            // directive on its own, and the kernel resolves it as typed.
            PickerKind::Skill => self.composer.replace_current_token(&format!("${id} ")),
            PickerKind::AddDirScope => {
                if let Some(path) = payload {
                    let scope = if id == "always" {
                        Scope::Always
                    } else {
                        Scope::Session
                    };
                    self.grant_directory(PathBuf::from(path), scope);
                }
            }
        }
    }

    fn submit_composer(&mut self, steer: bool) {
        if self.composer.is_empty() {
            return;
        }
        let raw = self.composer.text();
        if self.handle_slash(&raw) {
            self.composer.take();
            return;
        }
        let text = self.expand_skill(raw);
        if let Some(notice) = prompt_limit_notice(&text) {
            self.notice = notice;
            return;
        }
        self.composer.take();
        if self.turn_running || !self.ready {
            if steer && self.turn_running {
                let _ = self.kernel.cancel();
                self.status = "steering after cancel".into();
            }
            self.queued = Some(text.clone());
            self.notice = "message queued".into();
            if let Err(err) = self.kernel.prompt(text) {
                self.queued = None;
                self.error(err);
            }
            return;
        }
        self.send_prompt(text);
    }

    fn send_prompt(&mut self, text: String) {
        if let Some(notice) = prompt_limit_notice(&text) {
            if self.composer.is_empty() {
                self.composer.set_text(&text);
            }
            self.notice = notice;
            return;
        }
        self.transcript.push_user(text.clone());
        self.transcript.finish_assistant();
        self.scroll_from_bottom = 0;
        match self.kernel.prompt(text) {
            Ok(()) => {
                self.turn_running = true;
                self.turn_started = Some(Instant::now());
                self.phase = None;
                self.last_text_delta_at = None;
                self.status = "thinking".into();
                self.notice.clear();
            }
            Err(err) => self.error(err),
        }
    }

    fn handle_slash(&mut self, text: &str) -> bool {
        let trimmed = text.trim();
        if !trimmed.starts_with('/') {
            return false;
        }
        let mut parts = trimmed[1..].splitn(2, char::is_whitespace);
        let command = parts.next().unwrap_or("");
        let arg = parts.next().unwrap_or("").trim();
        match command {
            "" => self.open_slash_picker(""),
            "provider" => {
                if arg.is_empty() {
                    self.open_provider_picker()
                } else {
                    self.switch_provider(arg.to_string())
                }
            }
            "model" => {
                if arg.is_empty() {
                    self.open_model_picker()
                } else {
                    self.switch_model(arg.to_string())
                }
            }
            "theme" => {
                if arg.is_empty() {
                    self.open_theme_picker()
                } else {
                    self.apply_theme(arg)
                }
            }
            "weight" => {
                if arg.is_empty() {
                    self.open_weight_picker()
                } else {
                    self.apply_weight(arg)
                }
            }
            "icons" => {
                if arg.is_empty() {
                    self.open_icons_picker()
                } else {
                    self.apply_icons(arg)
                }
            }
            "session" => match arg {
                "" | "list" => {
                    self.status = "loading sessions".into();
                    if let Err(err) = self.kernel.list_sessions() {
                        self.error(err);
                    }
                }
                "new" => self.new_session(),
                _ if arg.starts_with("resume ") => self.load_session(arg[7..].trim().to_string()),
                id => self.load_session(id.to_string()),
            },
            "add-dir" => {
                if arg.is_empty() {
                    self.notice = "usage: /add-dir /absolute/path".into();
                } else {
                    self.open_scope_picker(arg);
                }
            }
            "help" => self.open_text_overlay(Overlay::Help),
            "clear" => self.transcript.clear(),
            "events" => self.open_text_overlay(Overlay::EventFeed),
            "hooks" => self.transcript.show_hooks = !self.transcript.show_hooks,
            "debug" => self.toggle_debug(),
            skill if self.skills.iter().any(|s| s.name == skill) => return false,
            _ => self.notice = format!("unknown command /{command}; F1 shows commands"),
        }
        true
    }

    fn expand_skill(&self, text: String) -> String {
        let trimmed = text.trim_start();
        let Some(rest) = trimmed.strip_prefix('/') else {
            return text;
        };
        let mut parts = rest.splitn(2, char::is_whitespace);
        let name = parts.next().unwrap_or("");
        if self.skills.iter().any(|skill| skill.name == name) {
            expand_skill_prompt(name, parts.next().unwrap_or(""))
        } else {
            text
        }
    }

    /// Ctrl+A / `/debug`: master toggle for hooks, unfolded tool output, and
    /// internal kernel events (skill injection, compaction detail,
    /// continuation). Persisted like the other view preferences.
    fn toggle_debug(&mut self) {
        self.transcript.debug = !self.transcript.debug;
        self.persisted.debug = self.transcript.debug;
        self.notice = if self.transcript.debug {
            "debug view on (hooks, internal events, full detail)"
        } else {
            "debug view off"
        }
        .into();
    }

    fn open_text_overlay(&mut self, overlay: Overlay) {
        self.overlay_scroll = 0;
        self.overlay = Some(overlay);
    }

    fn open_provider_picker(&mut self) {
        if self.turn_running {
            self.notice = "cancel the running turn before switching provider".into();
            return;
        }
        let separator = self.theme.glyph(" · ", " | ");
        let items = self
            .providers
            .iter()
            .map(|p| {
                let mut item = PickItem::new(
                    p.id.clone(),
                    p.name.clone(),
                    format!(
                        "{}{separator}{}{}",
                        p.default_model,
                        p.key_env,
                        if p.key_available { "" } else { " missing" }
                    ),
                );
                item.disabled = !p.key_available;
                item
            })
            .collect();
        let mut picker = Picker::new("Provider", items);
        if let Some(id) = self.provider.as_deref() {
            picker.select_id(id);
        }
        self.overlay = Some(Overlay::Picker {
            kind: PickerKind::Provider,
            picker,
            payload: None,
        });
    }

    fn open_theme_picker(&mut self) {
        let current = self.theme.name().to_string();
        let items = ThemeKind::ALL
            .into_iter()
            .map(|kind| {
                let item = PickItem::new(kind.name(), kind.display_name(), kind.description());
                if kind.name() == current {
                    item.with_tag("active")
                } else {
                    item
                }
            })
            .collect();
        let mut picker = Picker::new("Theme", items);
        picker.select_id(&current);
        self.overlay = Some(Overlay::Picker {
            kind: PickerKind::Theme,
            picker,
            payload: Some(current),
        });
    }

    fn preview_theme(&mut self, name: &str) {
        if let Some(kind) = ThemeKind::from_name(name) {
            self.theme = self.theme.with_kind(kind);
        }
    }

    fn apply_theme(&mut self, name: &str) {
        let Some(kind) = ThemeKind::from_name(name) else {
            self.notice = format!("unknown theme {name}");
            return;
        };
        self.theme = self.theme.with_kind(kind);
        self.persisted.theme = Some(kind.name().to_string());
        self.notice = format!("theme: {}", kind.display_name());
        self.persist();
    }

    fn open_weight_picker(&mut self) {
        let current = if self.theme.thin { "thin" } else { "regular" };
        let items = [
            ("thin", "Thin", "use the terminal's configured light face"),
            (
                "regular",
                "Regular",
                "allow bold emphasis for headings and status",
            ),
        ]
        .into_iter()
        .map(|(id, label, detail)| {
            let item = PickItem::new(id, label, detail);
            if id == current {
                item.with_tag("active")
            } else {
                item
            }
        })
        .collect();
        let mut picker = Picker::new("Text weight", items);
        picker.select_id(current);
        self.overlay = Some(Overlay::Picker {
            kind: PickerKind::Weight,
            picker,
            payload: None,
        });
    }

    fn apply_weight(&mut self, value: &str) {
        let thin = match value.trim().to_ascii_lowercase().as_str() {
            "thin" | "light" => true,
            "regular" | "normal" => false,
            _ => {
                self.notice = format!("unknown text weight {value}; use thin or regular");
                return;
            }
        };
        self.theme = self.theme.clone().with_thin(thin);
        self.persisted.thin = thin;
        self.notice = format!("text weight: {}", if thin { "thin" } else { "regular" });
        self.persist();
    }

    fn open_icons_picker(&mut self) {
        let current = match self.theme.icon_mode {
            IconMode::Nerd => "nerd",
            IconMode::Unicode => "unicode",
            IconMode::Ascii => "ascii",
        };
        let items = [
            ("nerd", "Nerd", "Nerd Font glyphs (needs a patched font)"),
            ("unicode", "Unicode", "plain Unicode symbols"),
            ("ascii", "ASCII", "ASCII-only fallbacks"),
        ]
        .into_iter()
        .map(|(id, label, detail)| {
            let item = PickItem::new(id, label, detail);
            if id == current {
                item.with_tag("active")
            } else {
                item
            }
        })
        .collect();
        let mut picker = Picker::new("Icons", items);
        picker.select_id(current);
        self.overlay = Some(Overlay::Picker {
            kind: PickerKind::Icons,
            picker,
            payload: None,
        });
    }

    fn apply_icons(&mut self, value: &str) {
        let Some(icon_mode) = IconMode::from_name(value) else {
            self.notice = format!("unknown icon set {value}; use nerd, unicode, or ascii");
            return;
        };
        self.theme = self.theme.with_icon_mode(icon_mode);
        self.persisted.icons = Some(value.trim().to_ascii_lowercase());
        self.notice = format!("icons: {}", value.trim().to_ascii_lowercase());
        self.persist();
    }

    fn open_slash_picker(&mut self, query: &str) {
        let mut items: Vec<_> = SLASH_COMMANDS
            .iter()
            .map(|(name, summary)| PickItem::new(*name, *name, *summary).with_tag("built-in"))
            .collect();
        for skill in &self.skills {
            if !items.iter().any(|item| item.id == skill.name) {
                items.push(
                    PickItem::new(
                        skill.name.clone(),
                        skill.name.clone(),
                        skill.summary.clone(),
                    )
                    .with_tag("skill"),
                );
            }
        }
        let mut picker = Picker::new("Command / skill", items);
        picker.set_query(query);
        self.overlay = Some(Overlay::Picker {
            kind: PickerKind::Slash,
            picker,
            payload: None,
        });
    }

    fn complete_token(&mut self) {
        let token = self.composer.current_token();
        if token.starts_with('@') {
            self.open_path_picker(token.trim_start_matches('@'));
        } else if token.starts_with('/') {
            self.open_slash_picker(token.trim_start_matches('/'));
        } else if token.starts_with('$') {
            self.open_skill_picker(token.trim_start_matches('$'));
        } else {
            self.notice = "Tab completes an @-path, /skill, or $skill token".into();
        }
    }

    /// `$` picker: installed skills only (unlike `/`, which mixes in
    /// built-in commands). The kernel resolves a leading `$name` itself, so
    /// selecting one just inserts the literal text — nothing is expanded.
    fn open_skill_picker(&mut self, query: &str) {
        let items = self
            .skills
            .iter()
            .map(|skill| {
                PickItem::new(skill.name.clone(), skill.name.clone(), skill.summary.clone())
                    .with_tag("skill")
            })
            .collect();
        let mut picker = Picker::new("Skill", items);
        picker.set_query(query);
        self.overlay = Some(Overlay::Picker {
            kind: PickerKind::Skill,
            picker,
            payload: None,
        });
    }

    fn open_path_picker(&mut self, query: &str) {
        let mut paths = Vec::new();
        collect_paths(&self.options.cwd, &self.options.cwd, 0, &mut paths);
        let items = paths
            .into_iter()
            .map(|path| {
                let label = path.to_string_lossy().to_string();
                let is_dir = self.options.cwd.join(&path).is_dir();
                let icon = if is_dir {
                    self.theme.icon("󰉋", "▸", "[d]")
                } else {
                    self.theme.icon("󰈔", "·", "[f]")
                };
                PickItem::new(label.clone(), format!("{icon} {label}"), "workspace path")
                    .with_tag(if is_dir { "dir" } else { "file" })
            })
            .collect();
        let mut picker = Picker::new("Workspace path", items);
        picker.set_query(query);
        self.overlay = Some(Overlay::Picker {
            kind: PickerKind::Path,
            picker,
            payload: None,
        });
    }

    fn open_model_picker(&mut self) {
        if self.turn_running {
            self.notice = "cancel the running turn before switching model".into();
            return;
        }
        let mut models: Vec<String> = self
            .provider
            .as_deref()
            .and_then(|id| self.providers.iter().find(|p| p.id == id))
            .map(|p| p.models.clone())
            .unwrap_or_default();
        if let Some(model) = self.model.clone() {
            if !models.contains(&model) {
                models.insert(0, model);
            }
        }
        let items = models
            .into_iter()
            .map(|model| {
                PickItem::new(
                    model.clone(),
                    model,
                    self.provider
                        .clone()
                        .unwrap_or_else(|| "legacy provider".into()),
                )
            })
            .collect();
        let mut picker = Picker::new("Model", items).with_free_text();
        if let Some(model) = self.model.as_deref() {
            picker.select_id(model);
        }
        self.overlay = Some(Overlay::Picker {
            kind: PickerKind::Model,
            picker,
            payload: None,
        });
    }

    fn open_scope_picker(&mut self, path: &str) {
        let path = PathBuf::from(path);
        let path = if path.is_absolute() {
            path
        } else {
            self.options.cwd.join(path)
        };
        if !path.is_dir() {
            self.notice = format!("directory does not exist: {}", path.display());
            return;
        }
        let items = vec![
            PickItem::new("session", "This session", "forgotten when lazartui exits"),
            PickItem::new("always", "Always", "persist in access-policy.json"),
        ];
        self.overlay = Some(Overlay::Picker {
            kind: PickerKind::AddDirScope,
            picker: Picker::new(format!("Grant {}", path.display()), items),
            payload: Some(path.to_string_lossy().to_string()),
        });
    }

    fn switch_provider(&mut self, id: String) {
        if self.turn_running {
            self.notice = "cancel the running turn before switching provider".into();
            return;
        }
        let Some(provider) = self.providers.iter().find(|p| p.id == id).cloned() else {
            self.notice = format!("unknown provider {id}");
            return;
        };
        self.provider = Some(provider.id.clone());
        self.model = Some(provider.default_model.clone());
        self.restart_kernel(format!(
            "provider: {} / {}",
            provider.name, provider.default_model
        ));
    }

    fn switch_model(&mut self, model: String) {
        if self.turn_running {
            self.notice = "cancel the running turn before switching model".into();
            return;
        }
        if model.trim().is_empty() {
            return;
        }
        self.model = Some(model.clone());
        self.restart_kernel(format!("model: {model}"));
    }

    fn restart_kernel(&mut self, marker: String) {
        self.transcript.clear();
        self.transcript.push_marker(marker);
        self.ready = false;
        self.turn_running = false;
        self.turn_started = None;
        self.status = "restarting".into();
        if let Err(err) = self.kernel.restart(self.current_config()) {
            self.error(err);
        }
        self.persist();
    }

    fn new_session(&mut self) {
        if self.turn_running {
            self.notice = "cancel the running turn before starting a session".into();
            return;
        }
        self.transcript.clear();
        self.transcript.push_marker("new session");
        self.access.reset_session();
        self.session_id = None;
        self.ready = false;
        self.status = "starting session".into();
        if let Err(err) = self.kernel.restart(self.current_config()) {
            self.error(err);
        }
    }

    fn load_session(&mut self, id: String) {
        if self.turn_running {
            self.notice = "cancel the running turn before loading a session".into();
            return;
        }
        self.transcript.clear();
        self.transcript.push_marker(format!("resume {id}"));
        self.session_id = Some(id.clone());
        self.ready = false;
        self.status = "loading session".into();
        if let Err(err) = self.kernel.load_session(id) {
            self.error(err);
        }
    }

    fn grant_directory(&mut self, path: PathBuf, scope: Scope) {
        if self.turn_running {
            self.notice = "cancel the running turn before changing access".into();
            return;
        }
        self.access.grant(path.clone(), scope);
        if scope == Scope::Always {
            if let Err(err) = self
                .access
                .policy
                .save(&policy_path(&self.options.lazar_home))
            {
                self.error(format!("cannot save access policy: {err}"));
                return;
            }
        }
        self.restart_kernel(format!("write access: {} ({scope:?})", path.display()));
    }

    fn handle_kernel_event(&mut self, event: KernelEvent) {
        match event {
            KernelEvent::Status(status) => self.status = status,
            KernelEvent::Ready { session_id } => {
                self.session_id = Some(session_id);
                self.ready = true;
                self.turn_running = false;
                self.turn_started = None;
                self.phase = None;
                self.status = "ready".into();
                self.persist();
            }
            KernelEvent::HistoryUser(text) => {
                // The kernel injects operator-role nudges (e.g. "[kernel
                // verification] ..." after a failed VERIFY block) that
                // aren't something the user actually typed; don't replay
                // them into the transcript.
                if !text.is_empty() && !text.starts_with("[kernel") {
                    self.transcript.push_user(text);
                }
            }
            KernelEvent::HistoryAssistant(text) => {
                if !text.is_empty() {
                    self.transcript.push_assistant_complete(text);
                }
            }
            KernelEvent::TextDelta(text) => {
                self.transcript.append_assistant(&text);
                self.last_text_delta_at = Some(Instant::now());
            }
            KernelEvent::ToolStart { id, command } => self.transcript.start_tool(id, command),
            KernelEvent::ToolResult {
                id,
                output,
                exit_code,
            } => self.transcript.finish_tool(&id, output, exit_code),
            KernelEvent::Lazar(event) => self.handle_lazar_event(event),
            KernelEvent::TurnFinished { stop_reason } => {
                self.transcript.finish_assistant();
                self.turn_running = false;
                self.turn_started = None;
                self.phase = None;
                self.status = stop_reason;
                self.persist();
            }
            KernelEvent::PromptAccepted(text) => {
                self.transcript.push_user(text);
                self.transcript.finish_assistant();
                self.queued = None;
                self.turn_running = true;
                self.turn_started = Some(Instant::now());
                self.phase = None;
                self.last_text_delta_at = None;
                self.status = "thinking".into();
                self.notice.clear();
                self.scroll_from_bottom = 0;
            }
            KernelEvent::Sessions(rows) => self.open_session_picker(rows),
            KernelEvent::Notice(message) => {
                self.event_feed.push(message);
            }
            KernelEvent::Error(err) => {
                self.transcript.finish_assistant();
                self.turn_running = false;
                self.turn_started = None;
                self.phase = None;
                self.error(err);
            }
        }
    }

    fn handle_lazar_event(&mut self, event: Value) {
        let type_name = event
            .get("type")
            .or_else(|| event.get("kind"))
            .and_then(Value::as_str)
            .unwrap_or("event");
        self.event_feed.push(event.to_string());
        if self.event_feed.len() > 500 {
            self.event_feed.drain(0..self.event_feed.len() - 500);
        }
        match type_name {
            "access_mode" => {
                if let Some(value) = event["unsandboxed"].as_bool() { self.effective_sandbox_off = value; }
            }
            "text_done" => self.transcript.finish_assistant(),
            "skill_inject" => {
                // Internal kernel plumbing, not something the operator asked
                // about — only clutter the transcript with it in debug mode.
                if self.transcript.debug {
                    let name = event.get("name").and_then(Value::as_str).unwrap_or("?");
                    let bytes = event.get("bytes").and_then(Value::as_u64).unwrap_or(0);
                    self.transcript
                        .push_marker(format!("skill {name} injected by kernel ({bytes} bytes)"));
                }
            }
            "hook_start" | "hook_end" => self.transcript.push_hook(&event),
            "compaction" => {
                // The kernel elided/summarized/truncated context. The user
                // should know it happened either way; debug mode gets the
                // byte counts and strategy, default mode gets one short line.
                let text = if self.transcript.debug {
                    let stage = event.get("stage").and_then(Value::as_str).unwrap_or("?");
                    let before = event.get("before_bytes").and_then(Value::as_u64).unwrap_or(0);
                    let after = event.get("after_bytes").and_then(Value::as_u64).unwrap_or(0);
                    format!(
                        "context compacted {} → {} ({stage})",
                        human_bytes(before),
                        human_bytes(after)
                    )
                } else {
                    "context compacted".to_string()
                };
                self.transcript.push_marker(text);
            }
            "continuation" => {
                // A model hit max_tokens and the kernel auto-continued it.
                // Not user-actionable — hide entirely unless debugging.
                if self.transcript.debug {
                    let reason = event.get("reason").and_then(Value::as_str).unwrap_or("?");
                    self.transcript
                        .push_marker(format!("continuation ({reason})"));
                }
            }
            "verify" => {
                let status = match event.get("status").and_then(Value::as_str) {
                    Some("passed") => VerifyStatus::Passed,
                    _ => VerifyStatus::Failed,
                };
                let attempt = event.get("attempt").and_then(Value::as_u64).unwrap_or(1) as u32;
                let final_ = event.get("final").and_then(Value::as_bool).unwrap_or(true);
                let tools_run = event.get("tools_run").and_then(Value::as_u64).unwrap_or(0);
                let claims = event
                    .get("claims")
                    .and_then(Value::as_array)
                    .map(|arr| {
                        arr.iter()
                            .map(|c| VerifyClaim {
                                claim: c.get("claim").and_then(Value::as_str).unwrap_or("").to_string(),
                                ok: c.get("ok").and_then(Value::as_bool).unwrap_or(false),
                                detail: c.get("detail").and_then(Value::as_str).unwrap_or("").to_string(),
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                self.transcript
                    .push_verify(status, attempt, final_, tools_run, claims);
            }
            "answer" => {
                // Emitted after any "verify" event, once per turn. `reply`
                // duplicates the already-streamed text and `verify` the
                // already-rendered verify block, so only sources/speech are
                // new here.
                let speech = event.get("speech").and_then(Value::as_str).unwrap_or("").to_string();
                let sources = event
                    .get("sources")
                    .and_then(Value::as_array)
                    .map(|arr| {
                        arr.iter()
                            .map(|s| {
                                let raw = s.get("ref").and_then(Value::as_str).unwrap_or("").to_string();
                                let kind = match s.get("kind").and_then(Value::as_str) {
                                    Some("url") => SourceKind::Url,
                                    _ => SourceKind::File,
                                };
                                SourceEntry {
                                    raw,
                                    kind,
                                    path: s.get("path").and_then(Value::as_str).map(str::to_string),
                                    line: s.get("line").and_then(Value::as_u64),
                                    exists: s.get("exists").and_then(Value::as_bool),
                                    name: s.get("name").and_then(Value::as_str).map(str::to_string),
                                    title: s.get("title").and_then(Value::as_str).map(str::to_string),
                                }
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                self.transcript.push_sources(sources, speech);
            }
            "context" => {
                let model = event.get("model").and_then(Value::as_str).unwrap_or("").to_string();
                self.context = Some(ContextInfo {
                    input_tokens: event.get("input_tokens").and_then(Value::as_u64).unwrap_or(0),
                    output_tokens: event.get("output_tokens").and_then(Value::as_u64).unwrap_or(0),
                    window: event.get("window").and_then(Value::as_u64).unwrap_or(0),
                    served_model: event
                        .get("served_model")
                        .and_then(Value::as_str)
                        .map(str::to_string)
                        .unwrap_or_else(|| model.clone()),
                    model,
                });
            }
            "phase" => {
                if let Some(phase) = event.get("phase").and_then(Value::as_str) {
                    self.phase = Some(phase.to_string());
                }
            }
            "invoke_start" => {
                if let Some(model) = event.get("model").and_then(Value::as_str) {
                    self.model = Some(model.to_string());
                }
                if let Some(provider) = event.get("provider").and_then(Value::as_str) {
                    self.provider = Some(provider.to_string());
                }
            }
            "invoke_end" => {
                self.input_tokens = self.input_tokens.saturating_add(
                    event
                        .get("input_tokens")
                        .and_then(Value::as_u64)
                        .unwrap_or(0),
                );
                self.output_tokens = self.output_tokens.saturating_add(
                    event
                        .get("output_tokens")
                        .and_then(Value::as_u64)
                        .unwrap_or(0),
                );
                self.transcript.finish_assistant();
            }
            _ => {}
        }
    }

    fn open_session_picker(&mut self, rows: Vec<SessionRow>) {
        let separator = self.theme.glyph(" · ", " | ");
        let items = rows
            .into_iter()
            .map(|row| {
                let label = row.title.unwrap_or_else(|| row.id.clone());
                let detail = match row.updated_at {
                    Some(ts) => format!("{}{separator}{ts}", row.cwd),
                    None => row.cwd,
                };
                PickItem::new(row.id, label, detail)
            })
            .collect();
        let mut picker = Picker::new("Session", items);
        if let Some(id) = self.session_id.as_deref() {
            picker.select_id(id);
        }
        self.overlay = Some(Overlay::Picker {
            kind: PickerKind::Session,
            picker,
            payload: None,
        });
        self.status = "ready".into();
    }

    fn error(&mut self, message: String) {
        self.status = "error".into();
        self.notice = message.clone();
        self.transcript.push_banner(BannerKind::Error, message);
    }

    fn dump_transcript(&mut self) {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let path = self
            .options
            .lazar_home
            .join("user")
            .join("workspace")
            .join(format!("lazartui-dump-{timestamp}.md"));
        match fs::write(&path, self.transcript.to_plain_text()) {
            Ok(()) => self.notice = format!("saved {}", path.display()),
            Err(err) => self.error(format!("cannot save transcript: {err}")),
        }
    }

    fn persist(&mut self) {
        self.persisted.provider = self.provider.clone();
        self.persisted.model = self.model.clone();
        self.persisted.session_id = self.session_id.clone();
        self.persisted.composer_history = self.composer.history().to_vec();
        if let Err(err) = self.persisted.save(&state_path(&self.options.lazar_home)) {
            self.notice = format!("state was not saved: {err}");
        }
    }

    pub fn shutdown(&mut self) {
        self.persist();
    }

    fn spinner_glyph(&self) -> &'static str {
        if self.theme.has_unicode_borders() {
            SPINNER[self.spinner_frame % SPINNER.len()]
        } else {
            SPINNER_ASCII[self.spinner_frame % SPINNER_ASCII.len()]
        }
    }

    /// The command of a currently-running tool call (a `Tool` block with no
    /// output yet), if any — for both the activity indicator's "Running
    /// <program>" label and the status line's running-tool count.
    fn running_tool_commands(&self) -> Vec<&str> {
        self.transcript
            .blocks
            .iter()
            .filter_map(|b| match b {
                crate::transcript::Block::Tool { command, output: None, .. } => Some(command.as_str()),
                _ => None,
            })
            .collect()
    }

    /// The Claude-Code-style "✻ Thinking… (12s · …)" line shown just above
    /// the composer while a turn runs, reusing the layout's existing gap row
    /// so no extra row is ever reserved. `None` while idle.
    fn build_activity_line(&self) -> Option<Vec<crate::status::Segment>> {
        let running_tools = self.running_tool_commands();
        let input = status::ActivityInput {
            turn_running: self.turn_running,
            running_tool_command: running_tools.first().copied(),
            ms_since_last_text_delta: self
                .last_text_delta_at
                .map(|at| at.elapsed().as_millis() as u64),
            phase: self.phase.as_deref(),
            elapsed_s: self.turn_started.map(|s| s.elapsed().as_secs()).unwrap_or(0),
            input_tokens: self.context.as_ref().map(|c| c.input_tokens).unwrap_or(0),
            output_tokens: self.context.as_ref().map(|c| c.output_tokens).unwrap_or(0),
        };
        status::activity_line(&input, self.spinner_glyph())
    }

    /// The two-line status plus optional agent panel shown under the
    /// composer. Built once per frame, before layout (its row count sizes
    /// the status slot) and reused for the render itself.
    fn build_status_info(&self) -> StatusInfo {
        let scroll_label = if self.scroll_from_bottom == 0 {
            "bottom".into()
        } else {
            format!("{} up", self.scroll_from_bottom)
        };
        let cwd = compact_path(&self.options.cwd);
        let spinner = self.spinner_glyph();
        let running_tools = self.running_tool_commands().len();
        let sandbox_state = self.effective_sandbox_off.then_some("sandbox off");
        let (agent_rows, agents_more) = status::agent_rows(&self.jobs, spinner, 4);
        StatusInfo {
            line1: status::status_line1(&Line1Input {
                user: &self.user,
                host: &self.host,
                cwd: &cwd,
                model: self.model.as_deref().unwrap_or("kernel default"),
                context: self.context.as_ref(),
            }),
            line2: status::status_line2(&Line2Input {
                provider: self.provider.as_deref(),
                sandbox_state,
                running_tools,
                agent_count: self.jobs.len(),
                queued: self.queued.is_some(),
                input_tokens: self.input_tokens,
                output_tokens: self.output_tokens,
                turn_elapsed_s: self.turn_started.map(|started| started.elapsed().as_secs()),
                scroll_label: &scroll_label,
                notice: &self.notice,
                stale_binary: self.stale_binary_notice.as_deref(),
            }),
            agent_rows,
            agents_more,
            focused_agent_row: self.footer_agent_focus,
        }
    }

    pub fn draw(&mut self, frame: &mut Frame) {
        let area = frame.area();
        frame.render_widget(FrameBlock::default().style(self.theme.canvas), area);
        let gutter = match area.width {
            100.. => 3,
            60.. => 2,
            _ => 1,
        }
        .min(area.width / 4);
        let available = area.inner(Margin {
            horizontal: gutter,
            vertical: 0,
        });
        // Use the whole terminal width.
        let max_width = available.width;
        let [content] = Layout::horizontal([Constraint::Length(max_width)])
            .flex(Flex::Center)
            .areas(available);
        if self.agents_screen.is_some() {
            self.draw_agents_screen(frame, content);
            return;
        }
        let composer_height = self.composer.height(content.width.saturating_sub(6)) + 2;
        let gap = u16::from(area.height >= 12);
        // Built before the layout so its row count (line1 + line2 always,
        // plus one row per job and an optional "+N more") can size the
        // status slot before it's actually rendered, below.
        let status_info = self.build_status_info();
        let activity_line = self.build_activity_line();
        let [header, transcript_slot, activity_area, composer_area, footer] = Layout::vertical([
            // Header bar plus its underline row (see render_header).
            Constraint::Length(2),
            Constraint::Min(2),
            // Reused for the "✻ Thinking…" activity indicator while a turn
            // runs; otherwise the same blank gap row as before.
            Constraint::Length(gap),
            Constraint::Length(composer_height),
            Constraint::Length(status_info.height()),
        ])
        .areas(content);
        render_header(
            frame,
            header,
            &self.theme,
            &HeaderInfo {
                session_id: self.session_id.as_deref().unwrap_or("new"),
                status: &self.status,
                turn_running: self.turn_running,
                spinner_frame: self.spinner_frame,
                sandbox_off: self.effective_sandbox_off,
            },
        );
        let mut transcript_area = transcript_slot;
        let viewport = transcript_area.height as usize;
        let full_width = transcript_area.width;
        let narrow = full_width > 2 && self.needs_scrollbar;
        if narrow {
            transcript_area.width = full_width.saturating_sub(2);
        }
        let mut lines = self.render_cache.lines(
            &self.transcript,
            transcript_area.width,
            &self.theme,
            &self.highlighter,
        );
        let still_needs_scrollbar = lines.len() > viewport;
        if still_needs_scrollbar != narrow {
            // The decision from last frame no longer holds (transcript grew
            // past/shrank back under the viewport) — re-render once at the
            // now-correct width. This only happens on the transition frame;
            // steady state hits the cache above at a stable width.
            transcript_area.width = if still_needs_scrollbar && full_width > 2 {
                full_width.saturating_sub(2)
            } else {
                full_width
            };
            lines = self.render_cache.lines(
                &self.transcript,
                transcript_area.width,
                &self.theme,
                &self.highlighter,
            );
        }
        self.needs_scrollbar = transcript_area.width < full_width;
        let max_scroll = lines.len().saturating_sub(viewport);
        let total_lines = lines.len();
        preserve_parked_scroll(
            &mut self.scroll_from_bottom,
            &mut self.previous_visual_lines,
            total_lines,
        );
        self.scroll_from_bottom = self.scroll_from_bottom.min(max_scroll);
        let top = max_scroll.saturating_sub(self.scroll_from_bottom);
        frame.render_widget(
            Paragraph::new(lines).scroll((top.min(u16::MAX as usize) as u16, 0)),
            transcript_area,
        );
        render_scrollbar(
            frame,
            transcript_slot,
            total_lines,
            viewport,
            top,
            self.scroll_from_bottom == 0,
            &self.theme,
        );
        if let Some(segments) = &activity_line {
            render_activity(frame, activity_area, &self.theme, segments);
        }

        let composer_block = FrameBlock::default()
            .borders(Borders::ALL)
            .border_type(if self.theme.has_unicode_borders() {
                BorderType::Rounded
            } else {
                BorderType::Plain
            })
            .border_style(if self.turn_running {
                self.theme.accent
            } else if self.overlay.is_some() {
                self.theme.composer_border
            } else {
                self.theme.composer_border_active
            })
            .style(self.theme.composer_surface);
        let block_inner = composer_block.inner(composer_area);
        frame.render_widget(composer_block, composer_area);
        let editor_area = Rect {
            x: block_inner.x.saturating_add(1),
            y: block_inner.y,
            width: block_inner.width.saturating_sub(2),
            height: block_inner.height,
        };
        self.composer.render(
            frame,
            editor_area,
            self.theme.icon("󰅂 ", "› ", "> "),
            self.theme.glyph("│ ", "  "),
            "Message Lazar  (/ commands, @ paths)",
            self.theme.composer_prompt,
            self.theme.composer_text,
            self.theme.dim.patch(self.theme.composer_surface),
            composer_ghost(self.composer.text().as_str()),
        );
        render_status(frame, footer, &self.theme, &status_info);
        if let Some(Overlay::Picker { kind, picker, .. }) = self.overlay.as_mut() {
            match kind {
                PickerKind::Slash | PickerKind::Path | PickerKind::Skill => {
                    picker.render_completion(frame, composer_area, &self.theme)
                }
                _ => picker.render(frame, content, &self.theme),
            }
        } else if let Some(overlay) = &self.overlay {
            match overlay {
                Overlay::Picker { .. } => {
                    unreachable!("picker overlays render through the mutable branch")
                }
                Overlay::Help => render_text_panel(
                    frame,
                    content,
                    &self.theme,
                    "Help",
                    help_lines(&self.theme),
                    self.overlay_scroll,
                ),
                Overlay::EventFeed => render_text_panel(
                    frame,
                    content,
                    &self.theme,
                    "Kernel events",
                    self.event_lines(),
                    self.overlay_scroll,
                ),
                Overlay::Subagents => render_text_panel(
                    frame,
                    content,
                    &self.theme,
                    "Sub-agents",
                    self.subagent_lines(),
                    self.overlay_scroll,
                ),
            }
        }
    }

    /// The full-screen Agents view: either the job list, or — with a job
    /// selected — its detail sub-screen. Replaces the whole chat layout
    /// while open (see `draw`).
    fn draw_agents_screen(&mut self, frame: &mut Frame, area: Rect) {
        let Some(screen) = &self.agents_screen else {
            return;
        };
        if let Some(detail) = &screen.detail {
            render_job_detail(frame, area, &self.theme, &self.highlighter, detail, self.spinner_glyph());
        } else {
            let cwd = compact_path(&self.options.cwd);
            let spinner = self.spinner_glyph();
            render_agents_list(
                frame,
                area,
                &self.theme,
                &AgentsListInfo {
                    model: self.model.as_deref().unwrap_or("kernel default"),
                    cwd: &cwd,
                    jobs: &screen.jobs,
                    selected: screen.selected,
                    spinner,
                },
            );
        }
    }

    fn event_lines(&self) -> Vec<Line<'static>> {
        if self.event_feed.is_empty() {
            return vec![Line::from(Span::styled(
                "No Lazar extension events yet.",
                self.theme.dim,
            ))];
        }
        self.event_feed
            .iter()
            .enumerate()
            .map(|(i, event)| {
                Line::from(vec![
                    Span::styled(format!("{i:>3} "), self.theme.dim),
                    Span::raw(event.clone()),
                ])
            })
            .collect()
    }

    fn subagent_lines(&self) -> Vec<Line<'static>> {
        let dir = self
            .options
            .lazar_home
            .join("user")
            .join("workspace")
            .join("subagent");
        let mut files: Vec<_> = fs::read_dir(&dir)
            .ok()
            .into_iter()
            .flatten()
            .flatten()
            .filter(|entry| entry.file_name().to_string_lossy().starts_with("out-"))
            .collect();
        files.sort_by_key(|entry| entry.metadata().and_then(|m| m.modified()).ok());
        files.reverse();
        if files.is_empty() {
            return vec![Line::from(Span::styled(
                "No sub-agent logs.",
                self.theme.dim,
            ))];
        }
        let mut lines = Vec::new();
        for entry in files.into_iter().take(12) {
            let name = entry.file_name().to_string_lossy().to_string();
            lines.push(Line::from(Span::styled(name, self.theme.accent)));
            if let Ok(text) = fs::read_to_string(entry.path()) {
                for line in text
                    .lines()
                    .rev()
                    .take(3)
                    .collect::<Vec<_>>()
                    .into_iter()
                    .rev()
                {
                    lines.push(Line::from(format!("  {line}")));
                }
            }
        }
        lines
    }
}

/// The Left key opens the Agents screen only when the composer has nothing
/// typed, or the cursor sits at column 0 of whatever line it's on —
/// otherwise Left just moves the cursor, as usual.
fn should_open_agents_screen(composer_empty: bool, cursor_col: usize) -> bool {
    composer_empty || cursor_col == 0
}

fn prompt_limit_notice(text: &str) -> Option<String> {
    const MAX_PROMPT_BYTES: usize = 9_000;
    if text.len() > MAX_PROMPT_BYTES {
        Some(format!(
            "prompt is {} bytes; Lazar's per-item limit is {MAX_PROMPT_BYTES}",
            text.len()
        ))
    } else {
        None
    }
}

/// `user@host` for the status line. No crate carries this cross-platform, so
/// this reads the usual env vars and shells out to `hostname` once, at
/// startup — never per frame.
fn current_user_host() -> (String, String) {
    let user = std::env::var("USER")
        .or_else(|_| std::env::var("LOGNAME"))
        .unwrap_or_else(|_| "user".into());
    let host = std::env::var("HOSTNAME")
        .ok()
        .filter(|h| !h.trim().is_empty())
        .or_else(|| {
            std::process::Command::new("hostname")
                .output()
                .ok()
                .filter(|o| o.status.success())
                .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
                .filter(|h| !h.is_empty())
        })
        .unwrap_or_else(|| "host".into());
    (user, host)
}

/// Cheap `stat()` for the stale-binary check: unreadable/missing paths are
/// `None`, not an error — a transient stat failure should never itself read
/// as "the binary changed" (see `status::binary_replaced`).
fn fingerprint(path: &Path) -> Option<status::FileFingerprint> {
    use std::os::unix::fs::MetadataExt;
    let metadata = fs::metadata(path).ok()?;
    let modified = metadata.modified().ok()?;
    let mtime_secs = modified
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    Some(status::FileFingerprint {
        mtime_secs,
        inode: metadata.ino(),
    })
}

fn compact_path(path: &Path) -> String {
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
        return path.display().to_string();
    };
    match path.strip_prefix(&home) {
        Ok(relative) if relative.as_os_str().is_empty() => "~".into(),
        Ok(relative) => format!("~/{}", relative.display()),
        Err(_) => path.display().to_string(),
    }
}

fn font_weight_from_env() -> Option<bool> {
    if let Ok(value) = std::env::var("LAZARTUI_FONT_WEIGHT") {
        return match value.trim().to_ascii_lowercase().as_str() {
            "thin" | "light" => Some(true),
            "regular" | "normal" => Some(false),
            _ => None,
        };
    }
    std::env::var("LAZARTUI_THIN").ok().and_then(|value| {
        match value.trim().to_ascii_lowercase().as_str() {
            "1" | "true" | "yes" | "on" => Some(true),
            "0" | "false" | "no" | "off" => Some(false),
            _ => None,
        }
    })
}

/// `LAZARTUI_DEBUG=1` starts the session with hooks, internal kernel events,
/// and full detail already visible instead of having to press Ctrl+A.
fn debug_from_env() -> bool {
    std::env::var("LAZARTUI_DEBUG")
        .ok()
        .is_some_and(|value| matches!(value.trim(), "1" | "true" | "yes" | "on"))
}

/// Human-scaled byte count for the debug-mode compaction marker, e.g. `180KB`.
fn human_bytes(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes}{}", UNITS[unit])
    } else {
        format!("{value:.0}{}", UNITS[unit])
    }
}

fn composer_ghost(text: &str) -> Option<&'static str> {
    match text {
        "/theme " => Some("<theme>"),
        "/weight " => Some("<thin | regular>"),
        "/icons " => Some("<nerd | unicode | ascii>"),
        "/model " => Some("<model>"),
        "/provider " => Some("<provider>"),
        "/session " => Some("<new | list | session-id>"),
        "/add-dir " => Some("<path>"),
        _ => None,
    }
}

fn preserve_parked_scroll(
    scroll_from_bottom: &mut usize,
    previous_visual_lines: &mut usize,
    current_visual_lines: usize,
) {
    if *scroll_from_bottom > 0 && *previous_visual_lines > 0 {
        *scroll_from_bottom = scroll_from_bottom
            .saturating_add(current_visual_lines.saturating_sub(*previous_visual_lines));
    }
    *previous_visual_lines = current_visual_lines;
}

fn collect_paths(root: &Path, dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    if depth > 4 || out.len() >= 2_000 {
        return;
    }
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        if out.len() >= 2_000 {
            return;
        }
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with('.') || matches!(name.as_ref(), "target" | "node_modules" | "vendor") {
            continue;
        }
        let path = entry.path();
        if let Ok(relative) = path.strip_prefix(root) {
            out.push(relative.to_path_buf());
        }
        if path.is_dir() {
            collect_paths(root, &path, depth + 1, out);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn char_key(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
    }

    fn plain_key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn new_test_app(root: &Path) -> App {
        App::new(LaunchOptions {
            lazar_home: root.to_path_buf(),
            cwd: root.to_path_buf(),
            add_dirs: vec![],
            provider: None,
            model: None,
            session_id: None,
            no_sandbox: false,
            kernel: Some(PathBuf::from("/usr/bin/false")),
            thin: None,
        })
    }

    #[test]
    fn left_key_opens_the_agents_screen_only_at_column_zero_or_an_empty_composer() {
        assert!(should_open_agents_screen(true, 5), "empty composer: any column");
        assert!(should_open_agents_screen(false, 0), "non-empty composer at column 0");
        assert!(!should_open_agents_screen(false, 1), "mid-line: Left just moves the cursor");
    }

    #[test]
    fn down_from_an_empty_composer_focuses_the_footer_panel_and_up_past_the_top_returns_focus() {
        let root = std::env::temp_dir().join(format!(
            "lazartui-footer-focus-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let mut app = new_test_app(&root);

        // No jobs yet: Down is ordinary composer navigation, not a focus.
        assert!(!app.key(plain_key(KeyCode::Down)));
        assert!(app.footer_agent_focus.is_none());

        app.jobs = vec![Job { job_id: "1".into(), task: "a".into(), status: "running".into(), ..Job::default() }];
        assert!(!app.key(plain_key(KeyCode::Down)));
        assert_eq!(app.footer_agent_focus, Some(0));

        // Up at the top returns focus to the composer.
        assert!(!app.key(plain_key(KeyCode::Up)));
        assert!(app.footer_agent_focus.is_none());

        // Refocus, then an ordinary keypress drops focus and reaches the
        // composer instead of being swallowed.
        assert!(!app.key(plain_key(KeyCode::Down)));
        assert!(!app.key(char_key('x')));
        assert!(app.footer_agent_focus.is_none());
        assert_eq!(app.composer.text(), "x");

        drop(app);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn esc_navigates_back_through_job_detail_then_the_agents_list_then_to_chat() {
        let root = std::env::temp_dir().join(format!(
            "lazartui-agents-esc-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let mut app = new_test_app(&root);

        app.open_agents_screen();
        assert!(app.agents_screen.is_some());
        {
            let screen = app.agents_screen.as_mut().unwrap();
            screen.jobs = vec![Job { job_id: "1".into(), task: "a".into(), status: "running".into(), ..Job::default() }];
        }
        app.open_job_detail("1".into());
        assert!(app.agents_screen.as_ref().unwrap().detail.is_some());

        // Esc from detail: back to the list, screen stays open.
        assert!(!app.key(plain_key(KeyCode::Esc)));
        assert!(app.agents_screen.is_some());
        assert!(app.agents_screen.as_ref().unwrap().detail.is_none());

        // Esc from the list: back to chat.
        assert!(!app.key(plain_key(KeyCode::Esc)));
        assert!(app.agents_screen.is_none());

        drop(app);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn parked_scroll_keeps_its_absolute_top_when_streaming_adds_lines() {
        let mut from_bottom = 7;
        let mut previous = 100;
        preserve_parked_scroll(&mut from_bottom, &mut previous, 104);
        assert_eq!(from_bottom, 11);
        assert_eq!(previous, 104);

        from_bottom = 0;
        preserve_parked_scroll(&mut from_bottom, &mut previous, 108);
        assert_eq!(from_bottom, 0, "follow mode must remain at the bottom");
    }

    #[test]
    fn direct_provider_and_model_switches_cannot_restart_a_running_turn() {
        let root = std::env::temp_dir().join(format!(
            "lazartui-switch-guard-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let mut app = App::new(LaunchOptions {
            lazar_home: root.clone(),
            cwd: root.clone(),
            add_dirs: vec![],
            provider: None,
            model: Some("original-model".into()),
            session_id: None,
            no_sandbox: false,
            kernel: Some(PathBuf::from("/usr/bin/false")),
            thin: None,
        });
        app.providers = vec![ProviderInfo {
            id: "alternate".into(),
            name: "Alternate".into(),
            base_url: "https://example.invalid".into(),
            key_env: "EXAMPLE_API_KEY".into(),
            key_available: false,
            default_model: "alternate-model".into(),
            models: vec!["alternate-model".into()],
        }];
        app.turn_running = true;

        app.switch_model("new-model".into());
        assert_eq!(app.model.as_deref(), Some("original-model"));
        assert!(app.notice.contains("running turn"));

        app.grant_directory(root.clone(), Scope::Always);
        assert!(app.access.session_dirs.is_empty());
        assert!(!policy_path(&root).exists());
        assert!(app.turn_running);
        app.handle_kernel_event(KernelEvent::Notice("ordinary stderr warning".into()));
        assert!(app.turn_running);
        app.handle_lazar_event(serde_json::json!({"type":"access_mode","unsandboxed":true}));
        assert!(app.effective_sandbox_off);
        app.handle_lazar_event(serde_json::json!({"type":"access_mode","unsandboxed":false}));
        assert!(!app.effective_sandbox_off);

        app.switch_provider("alternate".into());
        assert_eq!(app.provider, None);
        assert_eq!(app.model.as_deref(), Some("original-model"));
        assert!(app.notice.contains("running turn"));

        drop(app);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn debug_off_by_default_hides_internal_events_and_toggle_reveals_them() {
        let root = std::env::temp_dir().join(format!(
            "lazartui-debug-toggle-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let mut app = App::new(LaunchOptions {
            lazar_home: root.clone(),
            cwd: root.clone(),
            add_dirs: vec![],
            provider: None,
            model: None,
            session_id: None,
            no_sandbox: false,
            kernel: Some(PathBuf::from("/usr/bin/false")),
            thin: None,
        });
        assert!(!app.transcript.debug, "debug view must be off by default");

        app.handle_lazar_event(
            serde_json::json!({"type":"skill_inject","name":"foo","bytes":12}),
        );
        assert!(
            app.transcript.blocks.is_empty(),
            "skill_inject is internal plumbing, hidden by default"
        );

        app.handle_lazar_event(serde_json::json!({"type":"continuation","reason":"max_tokens"}));
        assert!(
            app.transcript.blocks.is_empty(),
            "continuation is not user-actionable, hidden by default"
        );

        app.handle_lazar_event(serde_json::json!({
            "type":"compaction","stage":"summary","before_bytes":612_000,"after_bytes":180_000,
        }));
        assert_eq!(app.transcript.blocks.len(), 1, "compaction is always surfaced");
        assert!(matches!(
            &app.transcript.blocks[0],
            crate::transcript::Block::Marker { text } if text == "context compacted"
        ));

        app.handle_lazar_event(
            serde_json::json!({"type":"hook_end","event":"pre-tool","script":"x.sh","action":"continue"}),
        );
        assert!(!app.transcript.hooks_visible());

        app.toggle_debug();
        assert!(app.transcript.debug);
        assert!(app.transcript.hooks_visible());

        app.handle_lazar_event(
            serde_json::json!({"type":"skill_inject","name":"foo","bytes":12}),
        );
        app.handle_lazar_event(serde_json::json!({"type":"continuation","reason":"max_tokens"}));
        app.handle_lazar_event(serde_json::json!({
            "type":"compaction","stage":"summary","before_bytes":612_000,"after_bytes":180_000,
        }));
        assert!(app
            .transcript
            .blocks
            .iter()
            .any(|b| matches!(b, crate::transcript::Block::Marker { text } if text.contains("skill foo injected"))));
        assert!(app
            .transcript
            .blocks
            .iter()
            .any(|b| matches!(b, crate::transcript::Block::Marker { text } if text.contains("continuation"))));
        assert!(app
            .transcript
            .blocks
            .iter()
            .any(|b| matches!(b, crate::transcript::Block::Marker { text } if text.contains("KB"))));

        drop(app);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn activity_line_is_none_while_idle_and_reports_a_running_tool_when_busy() {
        let root = std::env::temp_dir().join(format!(
            "lazartui-activity-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let mut app = App::new(LaunchOptions {
            lazar_home: root.clone(),
            cwd: root.clone(),
            add_dirs: vec![],
            provider: None,
            model: None,
            session_id: None,
            no_sandbox: false,
            kernel: Some(PathBuf::from("/usr/bin/false")),
            thin: None,
        });

        assert!(app.build_activity_line().is_none(), "idle: no row reserved");

        app.turn_running = true;
        app.turn_started = Some(Instant::now());
        app.transcript.start_tool("t1", "cargo build --release");
        let segments = app.build_activity_line().expect("turn running");
        assert!(segments[0].text.contains("Running cargo"), "{segments:?}");

        app.transcript.finish_tool("t1", "ok".into(), Some(0));
        app.phase = Some("verifying".into());
        let segments = app.build_activity_line().expect("turn running");
        assert!(segments[0].text.contains("Verifying"), "{segments:?}");

        app.handle_kernel_event(KernelEvent::TurnFinished { stop_reason: "end_turn".into() });
        assert!(app.build_activity_line().is_none(), "phase resets at turn end");
        assert!(app.phase.is_none());

        drop(app);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn answer_events_become_a_sources_block_and_store_speech() {
        let root = std::env::temp_dir().join(format!(
            "lazartui-answer-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let mut app = App::new(LaunchOptions {
            lazar_home: root.clone(),
            cwd: root.clone(),
            add_dirs: vec![],
            provider: None,
            model: None,
            session_id: None,
            no_sandbox: false,
            kernel: Some(PathBuf::from("/usr/bin/false")),
            thin: None,
        });

        app.handle_lazar_event(serde_json::json!({
            "type": "answer",
            "reply": "the fix is in x.rs",
            "speech": "It's fixed.",
            "sources": [
                {"ref": "/abs/x.rs:42", "kind": "file", "path": "/abs/x.rs", "line": 42, "exists": true},
                {"ref": "/abs/gone.rs:7", "kind": "file", "path": "/abs/gone.rs", "line": 7, "exists": false},
                {"ref": "https://example.com/page", "kind": "url"},
            ],
            "verify": {"status": "none", "claims": 0},
        }));

        assert_eq!(app.transcript.last_speech.as_deref(), Some("It's fixed."));
        match app.transcript.blocks.last() {
            Some(crate::transcript::Block::Sources { entries, speech }) => {
                assert_eq!(speech, "It's fixed.");
                assert_eq!(entries.len(), 3);
                assert_eq!(entries[0].path.as_deref(), Some("/abs/x.rs"));
                assert_eq!(entries[0].line, Some(42));
                assert_eq!(entries[0].exists, Some(true));
                assert_eq!(entries[1].exists, Some(false));
                assert!(matches!(entries[2].kind, crate::transcript::SourceKind::Url));
            }
            other => panic!("{other:?}"),
        }

        drop(app);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn verify_events_become_transcript_blocks_and_kernel_nudges_are_hidden_from_replay() {
        let root = std::env::temp_dir().join(format!(
            "lazartui-verify-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let mut app = App::new(LaunchOptions {
            lazar_home: root.clone(),
            cwd: root.clone(),
            add_dirs: vec![],
            provider: None,
            model: None,
            session_id: None,
            no_sandbox: false,
            kernel: Some(PathBuf::from("/usr/bin/false")),
            thin: None,
        });

        app.handle_lazar_event(serde_json::json!({
            "type":"verify","status":"passed","attempt":1,"final":true,"tools_run":1,
            "claims":[{"claim":"exists=/tmp/x","ok":true,"detail":"exists"}],
        }));
        assert!(matches!(
            app.transcript.blocks.last(),
            Some(crate::transcript::Block::Verify {
                status: crate::transcript::VerifyStatus::Passed,
                ..
            })
        ));

        app.handle_lazar_event(serde_json::json!({
            "type":"verify","status":"failed","attempt":1,"final":false,"tools_run":2,
            "claims":[{"claim":"exists=/tmp/x","ok":false,"detail":"missing"}],
        }));
        assert!(matches!(
            app.transcript.blocks.last(),
            Some(crate::transcript::Block::Verify {
                status: crate::transcript::VerifyStatus::Failed,
                final_: false,
                ..
            })
        ));

        // A replayed operator-role kernel nudge is not something the user
        // typed; it must not show up as a user message in the transcript.
        app.handle_kernel_event(KernelEvent::HistoryUser(
            "[kernel verification] your previous reply failed a check".into(),
        ));
        assert!(!app
            .transcript
            .blocks
            .iter()
            .any(|b| matches!(b, crate::transcript::Block::User { text } if text.contains("kernel verification"))));

        // An ordinary replayed user message still comes through.
        app.handle_kernel_event(KernelEvent::HistoryUser("hello there".into()));
        assert!(app
            .transcript
            .blocks
            .iter()
            .any(|b| matches!(b, crate::transcript::Block::User { text } if text == "hello there")));

        drop(app);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn oversized_prompt_is_rejected_before_transport() {
        let root = std::env::temp_dir().join(format!(
            "lazartui-prompt-limit-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let mut app = App::new(LaunchOptions {
            lazar_home: root.clone(),
            cwd: root.clone(),
            add_dirs: vec![],
            provider: None,
            model: None,
            session_id: None,
            no_sandbox: false,
            kernel: Some(PathBuf::from("/usr/bin/false")),
            thin: None,
        });
        app.ready = true;
        let oversized = "x".repeat(9_001);
        app.composer.set_text(&oversized);
        app.submit_composer(false);
        assert!(!app.turn_running);
        assert!(app.transcript.blocks.is_empty());
        assert!(app.notice.contains("per-item limit"));
        assert_eq!(app.composer.text(), oversized, "a rejected prompt stays in the composer");

        app.composer.clear();
        app.send_prompt(oversized.clone());
        assert!(!app.turn_running);
        assert!(app.transcript.blocks.is_empty());
        assert_eq!(app.composer.text(), oversized);
        drop(app);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn slash_and_at_open_live_completion_and_sync_the_composer() {
        let root = std::env::temp_dir().join(format!(
            "lazartui-completion-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("user/skills")).unwrap();
        fs::write(root.join("user/skills/INDEX.md"), "- todo — task list\n").unwrap();

        let mut app = App::new(LaunchOptions {
            lazar_home: root.clone(),
            cwd: root.clone(),
            add_dirs: vec![],
            provider: None,
            model: None,
            session_id: None,
            no_sandbox: false,
            kernel: Some(PathBuf::from("/usr/bin/false")),
            thin: None,
        });

        app.key(char_key('/'));
        let Overlay::Picker { kind, picker, .. } = app.overlay.as_ref().unwrap() else {
            panic!("slash did not open a picker");
        };
        assert_eq!(*kind, PickerKind::Slash);
        assert!(picker.filtered().iter().any(|(_, item)| item.id == "model"));
        assert!(picker.filtered().iter().any(|(_, item)| item.id == "todo"));

        app.key(char_key('m'));
        assert_eq!(app.composer.text(), "/m");
        app.key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(app.overlay.is_none());
        assert_eq!(app.composer.text(), "/m");

        app.composer.clear();
        app.key(char_key('@'));
        assert!(matches!(
            app.overlay,
            Some(Overlay::Picker {
                kind: PickerKind::Path,
                ..
            })
        ));
        app.key(char_key('s'));
        assert_eq!(app.composer.text(), "@s");

        app.mouse(MouseEvent {
            kind: MouseEventKind::ScrollUp,
            column: 0,
            row: 0,
            modifiers: KeyModifiers::NONE,
        });
        assert_eq!(app.scroll_from_bottom, 3);
        app.handle_kernel_event(KernelEvent::TextDelta("streaming".into()));
        assert_eq!(
            app.scroll_from_bottom, 3,
            "streaming output must not steal the scrolled viewport"
        );
        app.mouse(MouseEvent {
            kind: MouseEventKind::ScrollDown,
            column: 0,
            row: 0,
            modifiers: KeyModifiers::NONE,
        });
        assert_eq!(app.scroll_from_bottom, 0);

        drop(app);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn dollar_opens_a_skills_only_picker_and_sends_the_prompt_verbatim() {
        let root = std::env::temp_dir().join(format!(
            "lazartui-dollar-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("user/skills")).unwrap();
        fs::write(root.join("user/skills/INDEX.md"), "- todo — task list\n").unwrap();

        let mut app = App::new(LaunchOptions {
            lazar_home: root.clone(),
            cwd: root.clone(),
            add_dirs: vec![],
            provider: None,
            model: None,
            session_id: None,
            no_sandbox: false,
            kernel: Some(PathBuf::from("/usr/bin/false")),
            thin: None,
        });
        app.ready = true;

        app.key(char_key('$'));
        let Overlay::Picker { kind, picker, .. } = app.overlay.as_ref().unwrap() else {
            panic!("$ did not open a picker");
        };
        assert_eq!(*kind, PickerKind::Skill);
        // Only skills, no built-in commands like "model".
        assert!(picker.filtered().iter().any(|(_, item)| item.id == "todo"));
        assert!(!picker.filtered().iter().any(|(_, item)| item.id == "model"));

        app.apply_pick(PickerKind::Skill, "todo".into(), None);
        assert_eq!(app.composer.text(), "$todo ");

        // The kernel resolves a leading `$name` itself; the TUI must not
        // expand or otherwise rewrite it before sending.
        app.composer.insert_str("do the thing");
        app.submit_composer(false);
        // submit_composer sends through send_prompt -> kernel.prompt(text),
        // which also records the text verbatim in the transcript.
        assert!(app
            .transcript
            .blocks
            .iter()
            .any(|b| matches!(b, crate::transcript::Block::User { text } if text == "$todo do the thing")));

        drop(app);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn theme_picker_previews_reverts_and_persists() {
        let root = std::env::temp_dir().join(format!(
            "lazartui-theme-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let mut app = App::new(LaunchOptions {
            lazar_home: root.clone(),
            cwd: root.clone(),
            add_dirs: vec![],
            provider: None,
            model: None,
            session_id: None,
            no_sandbox: false,
            kernel: Some(PathBuf::from("/usr/bin/false")),
            thin: None,
        });

        app.key(char_key('/'));
        for c in "theme".chars() {
            app.key(char_key(c));
        }
        app.key(char_key(' '));
        assert!(matches!(
            app.overlay,
            Some(Overlay::Picker {
                kind: PickerKind::Theme,
                ..
            })
        ));
        app.key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        app.composer.clear();

        app.open_theme_picker();
        app.key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        assert_eq!(app.theme.name(), "groknight");
        app.key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert_eq!(app.theme.name(), "nerv");

        app.open_theme_picker();
        app.key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        app.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(app.theme.name(), "groknight");
        assert_eq!(
            PersistedState::load(&state_path(&root)).theme.as_deref(),
            Some("groknight")
        );

        app.composer.clear();
        app.key(char_key('/'));
        for c in "weight".chars() {
            app.key(char_key(c));
        }
        app.key(char_key(' '));
        assert!(matches!(
            app.overlay,
            Some(Overlay::Picker {
                kind: PickerKind::Weight,
                ..
            })
        ));
        app.key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
        app.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(app.theme.thin);
        assert!(PersistedState::load(&state_path(&root)).thin);

        drop(app);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn command_pickers_clear_the_composer_on_pick_and_on_cancel() {
        let root = std::env::temp_dir().join(format!(
            "lazartui-picker-clears-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let mut app = App::new(LaunchOptions {
            lazar_home: root.clone(),
            cwd: root.clone(),
            add_dirs: vec![],
            provider: None,
            model: None,
            session_id: None,
            no_sandbox: false,
            kernel: Some(PathBuf::from("/usr/bin/false")),
            thin: None,
        });

        // Reached via the slash live-completion flow: the composer holds
        // "/theme " when the (unprefixed) Theme picker opens over it.
        app.key(char_key('/'));
        for c in "theme".chars() {
            app.key(char_key(c));
        }
        app.key(char_key(' '));
        assert_eq!(app.composer.text(), "/theme ");
        app.key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        app.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(app.composer.text(), "", "picking a theme clears the stale /theme token");

        app.key(char_key('/'));
        for c in "weight".chars() {
            app.key(char_key(c));
        }
        app.key(char_key(' '));
        assert_eq!(app.composer.text(), "/weight ");
        app.key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert_eq!(app.composer.text(), "", "cancelling also clears the command token");

        drop(app);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn notices_are_transient_expiring_after_3s_or_on_the_next_keypress() {
        let root = std::env::temp_dir().join(format!(
            "lazartui-transient-notice-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let mut app = App::new(LaunchOptions {
            lazar_home: root.clone(),
            cwd: root.clone(),
            add_dirs: vec![],
            provider: None,
            model: None,
            session_id: None,
            no_sandbox: false,
            kernel: Some(PathBuf::from("/usr/bin/false")),
            thin: None,
        });

        // Exercise `expire_notice` directly rather than the full `tick`: the
        // fake `/usr/bin/false` kernel exits immediately and its background
        // "connection closed" error would otherwise race this notice.
        app.notice = "hook trace visible".into();
        app.expire_notice();
        assert_eq!(app.notice, "hook trace visible", "not expired yet");
        app.notice_set_at = Instant::now() - Duration::from_secs(4);
        app.expire_notice();
        assert!(app.notice.is_empty(), "expires ~3s after it was set");

        app.notice = "another notice".into();
        app.expire_notice();
        assert!(!app.notice.is_empty());
        app.key(char_key('x'));
        assert!(app.notice.is_empty(), "a keypress dismisses it immediately");

        drop(app);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn stale_binary_notice_appears_once_after_a_rebuild_and_never_clears_itself() {
        use std::os::unix::fs::PermissionsExt;

        let root = std::env::temp_dir().join(format!(
            "lazartui-stale-binary-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let script = root.join("fake-lazar");
        fs::write(&script, "#!/bin/sh\nexit 0\n").unwrap();
        let mut permissions = fs::metadata(&script).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&script, permissions).unwrap();

        let mut app = App::new(LaunchOptions {
            lazar_home: root.clone(),
            cwd: root.clone(),
            add_dirs: vec![],
            provider: None,
            model: None,
            session_id: None,
            no_sandbox: false,
            kernel: Some(script.clone()),
            thin: None,
        });
        assert!(app.stale_binary_notice.is_none());

        // Not due for a recheck yet: unchanged even if the file were newer.
        let file = fs::File::open(&script).unwrap();
        file.set_modified(SystemTime::now() + Duration::from_secs(60)).unwrap();
        app.check_stale_binary();
        assert!(app.stale_binary_notice.is_none(), "checked too soon");

        // Due for a recheck, and the kernel binary is now newer: notice.
        app.last_binary_check = Instant::now() - Duration::from_secs(6);
        app.check_stale_binary();
        let notice = app.stale_binary_notice.clone().expect("stale build detected");
        assert!(notice.contains("restart lazartui"), "{notice:?}");
        assert!(
            status::status_line2(&Line2Input {
                provider: None,
                sandbox_state: None,
                running_tools: 0,
                agent_count: 0,
                queued: false,
                input_tokens: 0,
                output_tokens: 0,
                turn_elapsed_s: None,
                scroll_label: "bottom",
                notice: "",
                stale_binary: app.stale_binary_notice.as_deref(),
            })
            .iter()
            .any(|s| s.text == notice && s.tone == status::Tone::Warn)
        );

        // Doesn't nag: it stays exactly as it was after further checks, even
        // once the recheck interval has passed again.
        app.last_binary_check = Instant::now() - Duration::from_secs(6);
        app.check_stale_binary();
        assert_eq!(app.stale_binary_notice, Some(notice));

        drop(app);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn icons_command_switches_and_persists_the_icon_mode() {
        let root = std::env::temp_dir().join(format!(
            "lazartui-icons-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let mut app = App::new(LaunchOptions {
            lazar_home: root.clone(),
            cwd: root.clone(),
            add_dirs: vec![],
            provider: None,
            model: None,
            session_id: None,
            no_sandbox: false,
            kernel: Some(PathBuf::from("/usr/bin/false")),
            thin: None,
        });

        app.composer.set_text("/icons ascii");
        assert!(app.handle_slash(&app.composer.text()));
        assert_eq!(app.theme.icon_mode, crate::render::theme::IconMode::Ascii);
        assert_eq!(
            PersistedState::load(&state_path(&root)).icons.as_deref(),
            Some("ascii")
        );

        app.composer.clear();
        app.key(char_key('/'));
        for c in "icons".chars() {
            app.key(char_key(c));
        }
        app.key(char_key(' '));
        assert!(matches!(
            app.overlay,
            Some(Overlay::Picker {
                kind: PickerKind::Icons,
                ..
            })
        ));
        app.key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
        app.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(app.theme.icon_mode, crate::render::theme::IconMode::Unicode);
        assert_eq!(
            PersistedState::load(&state_path(&root)).icons.as_deref(),
            Some("unicode")
        );

        drop(app);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn launch_only_thin_override_does_not_replace_saved_preference() {
        let root = std::env::temp_dir().join(format!(
            "lazartui-weight-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let mut app = App::new(LaunchOptions {
            lazar_home: root.clone(),
            cwd: root.clone(),
            add_dirs: vec![],
            provider: None,
            model: None,
            session_id: None,
            no_sandbox: false,
            kernel: Some(PathBuf::from("/usr/bin/false")),
            thin: Some(true),
        });
        assert!(app.theme.thin);
        app.persist();
        assert!(!PersistedState::load(&state_path(&root)).thin);
        drop(app);
        fs::remove_dir_all(root).unwrap();
    }
}
