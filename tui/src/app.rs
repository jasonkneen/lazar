use crate::access::{policy_path, AccessPolicy, AccessState, Scope};
use crate::composer::Composer;
use crate::kernel::{KernelConfig, KernelEvent, KernelHandle, SessionRow};
use crate::keys::{action_for, Action};
use crate::overlays::picker::{PickItem, Picker, PickerOutcome};
use crate::overlays::{help_lines, render_text_panel, Overlay, PickerKind};
use crate::providers::{kernel_binary, load_catalog, ProviderInfo};
use crate::render::chrome::{render_footer, render_header, FooterInfo, HeaderInfo};
use crate::render::markdown::Highlighter;
use crate::render::scrollbar::render_scrollbar;
use crate::render::theme::{Theme, ThemeKind};
use crate::render::transcript::RenderCache;
use crate::skills::{expand_skill_prompt, load_index, SkillEntry};
use crate::state::{state_path, PersistedState};
use crate::transcript::{BannerKind, Transcript, VerifyClaim, VerifyStatus};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseEvent, MouseEventKind};
use ratatui::layout::{Alignment, Constraint, Flex, Layout, Margin, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block as FrameBlock, BorderType, Borders, Paragraph};
use ratatui::Frame;
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use unicode_width::UnicodeWidthStr;

const SLASH_COMMANDS: &[(&str, &str)] = &[
    ("provider", "list or switch provider"),
    ("model", "list or switch model"),
    ("theme", "preview or switch the color theme"),
    ("weight", "switch between thin and regular text"),
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
}

impl App {
    pub fn new(mut options: LaunchOptions) -> Self {
        let state_file = state_path(&options.lazar_home);
        let skills = load_index(&options.lazar_home);
        let persisted = PersistedState::load(&state_file);
        let kernel_binary = options
            .kernel
            .clone()
            .unwrap_or_else(|| kernel_binary(&options.lazar_home));
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
        let base_theme = Theme::nerv().with_thin(thin);
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
        if self.overlay.is_some() {
            return self.overlay_key(key);
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
                        _ => {}
                    }
                }
                return false;
            }
            match outcome {
                PickerOutcome::Pending => return false,
                PickerOutcome::Cancelled => {
                    self.overlay = None;
                    if kind == PickerKind::Theme {
                        if let Some(previous) = payload.as_deref() {
                            self.preview_theme(previous);
                        }
                    }
                    return false;
                }
                PickerOutcome::Chosen(id) => {
                    self.overlay = None;
                    self.apply_pick(kind, id, payload);
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

    fn apply_pick(&mut self, kind: PickerKind, id: String, payload: Option<String>) {
        match kind {
            PickerKind::Provider => self.switch_provider(id),
            PickerKind::Model => self.switch_model(id),
            PickerKind::Session => self.load_session(id),
            PickerKind::Theme => self.apply_theme(&id),
            PickerKind::Weight => self.apply_weight(&id),
            PickerKind::Slash => {
                self.composer.replace_current_token(&format!("/{id} "));
                match id.as_str() {
                    "theme" => self.open_theme_picker(),
                    "weight" => self.open_weight_picker(),
                    _ => {}
                }
            }
            PickerKind::Path => self.composer.replace_current_token(&format!("@{id}")),
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
        } else {
            self.notice = "Tab completes an @-path or /skill token".into();
        }
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
                self.status = stop_reason;
                self.persist();
            }
            KernelEvent::PromptAccepted(text) => {
                self.transcript.push_user(text);
                self.transcript.finish_assistant();
                self.queued = None;
                self.turn_running = true;
                self.turn_started = Some(Instant::now());
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
        let max_width = available.width.min(132);
        let [content] = Layout::horizontal([Constraint::Length(max_width)])
            .flex(Flex::Center)
            .areas(available);
        let composer_height = self.composer.height(content.width.saturating_sub(6)) + 2;
        let gap = u16::from(area.height >= 12);
        let [header, transcript_slot, _, composer_area, footer] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Min(2),
            Constraint::Length(gap),
            Constraint::Length(composer_height),
            Constraint::Length(1),
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

        let composer_meta = composer_metadata(
            self.model.as_deref().unwrap_or("kernel default"),
            self.provider.as_deref().unwrap_or("legacy"),
            self.theme.name(),
            self.theme.glyph(" · ", " | "),
            self.theme.glyph("…", "..."),
            composer_area.width.saturating_sub(4) as usize,
        );
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
            .style(self.theme.composer_surface)
            .title_bottom(Span::styled(
                format!(" {composer_meta} "),
                self.theme.footer.patch(self.theme.composer_surface),
            ))
            .title_alignment(Alignment::Right);
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
        let scroll_label = if self.scroll_from_bottom == 0 {
            "bottom".into()
        } else {
            format!("{} up", self.scroll_from_bottom)
        };
        let cwd = compact_path(&self.options.cwd);
        render_footer(
            frame,
            footer,
            &self.theme,
            &FooterInfo {
                cwd: &cwd,
                input_tokens: self.input_tokens,
                output_tokens: self.output_tokens,
                scroll: &scroll_label,
                queued: self.queued.is_some(),
                notice: &self.notice,
                turn_elapsed: self.turn_started.map(|started| started.elapsed().as_secs()),
            },
        );
        if let Some(Overlay::Picker { kind, picker, .. }) = self.overlay.as_mut() {
            match kind {
                PickerKind::Slash | PickerKind::Path => {
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

fn composer_metadata(
    model: &str,
    provider: &str,
    theme: &str,
    separator: &str,
    ellipsis: &str,
    width: usize,
) -> String {
    let choices = [
        format!("{model}{separator}{provider}{separator}{theme}"),
        format!("{model}{separator}{theme}"),
        model.to_string(),
    ];
    choices
        .into_iter()
        .find(|value| value.width() <= width)
        .unwrap_or_else(|| truncate_columns(model, width, ellipsis))
}

fn composer_ghost(text: &str) -> Option<&'static str> {
    match text {
        "/theme " => Some("<theme>"),
        "/weight " => Some("<thin | regular>"),
        "/model " => Some("<model>"),
        "/provider " => Some("<provider>"),
        "/session " => Some("<new | list | session-id>"),
        "/add-dir " => Some("<path>"),
        _ => None,
    }
}

fn truncate_columns(text: &str, width: usize, ellipsis: &str) -> String {
    if text.width() <= width {
        return text.to_string();
    }
    if width == 0 {
        return String::new();
    }
    let marker = fit_prefix(ellipsis, width);
    let budget = width.saturating_sub(marker.width());
    let mut out = String::new();
    for c in text.chars() {
        let candidate = format!("{out}{c}");
        if candidate.width() > budget {
            break;
        }
        out.push(c);
    }
    out.push_str(&marker);
    out
}

fn fit_prefix(text: &str, width: usize) -> String {
    let mut out = String::new();
    for c in text.chars() {
        let candidate = format!("{out}{c}");
        if candidate.width() > width {
            break;
        }
        out.push(c);
    }
    out
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
