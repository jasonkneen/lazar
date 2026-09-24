//! Pure composition for the Claude-Code-style status area under the
//! composer: a two-line status (identity/model/context, then mode/activity)
//! plus, when sub-agents exist, an agent panel. Kept free of ratatui so the
//! composition rules (what shows, what's omitted, which colour) are testable
//! without a terminal; `render::chrome` turns `Tone` into a `Theme` style.

use crate::jobs::Job;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Normal,
    Dim,
    Accent,
    Ok,
    Warn,
    Fail,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Segment {
    pub text: String,
    pub tone: Tone,
}

impl Segment {
    pub fn new(text: impl Into<String>, tone: Tone) -> Self {
        Self { text: text.into(), tone }
    }
    pub fn plain(text: impl Into<String>) -> Self {
        Self::new(text, Tone::Normal)
    }
    pub fn dim(text: impl Into<String>) -> Self {
        Self::new(text, Tone::Dim)
    }
}

/// The kernel's `context` event: token usage and window size for the model's
/// most recent request. The TUI only displays this — the kernel computes it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ContextInfo {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub window: u64,
    /// The configured model id (e.g. a router alias like "auto").
    pub model: String,
    /// The model that actually answered. Equal to `model` when the kernel
    /// doesn't know better (e.g. no router involved).
    pub served_model: String,
}

impl ContextInfo {
    /// `input_tokens` as a percentage of `window`, clamped to 100.
    pub fn percent(&self) -> u8 {
        if self.window == 0 {
            return 0;
        }
        (self.input_tokens.saturating_mul(100) / self.window).min(100) as u8
    }

    /// theme.ok below 50%, theme.warn 50-80%, theme.fail above 80%.
    pub fn tone(&self) -> Tone {
        let pct = self.percent();
        if pct > 80 {
            Tone::Fail
        } else if pct >= 50 {
            Tone::Warn
        } else {
            Tone::Ok
        }
    }
}

pub struct Line1Input<'a> {
    pub user: &'a str,
    pub host: &'a str,
    pub cwd: &'a str,
    pub model: &'a str,
    pub context: Option<&'a ContextInfo>,
}

/// `user@host  ~/cwd  <model> (<window/1000>k ctx)  ctx:<pct>%`
///
/// Before any `context` event, `<model>` is the configured model
/// (`Line1Input::model`). Once one has arrived, it's the model that actually
/// answered (`served_model`) — and if a router alias was configured and
/// served differs from it, both show as `served (configured)`, e.g.
/// `glm-4.6 (auto)`.
pub fn status_line1(input: &Line1Input) -> Vec<Segment> {
    let mut segments = vec![
        Segment::plain(format!("{}@{}", input.user, input.host)),
        Segment::plain(input.cwd.to_string()),
    ];
    match input.context {
        Some(ctx) => {
            let served = if ctx.served_model.is_empty() {
                ctx.model.as_str()
            } else {
                ctx.served_model.as_str()
            };
            let model = if !ctx.model.is_empty() && ctx.model != served {
                format!("{served} ({})", ctx.model)
            } else {
                served.to_string()
            };
            segments.push(Segment::plain(format!("{model} ({}k ctx)", ctx.window / 1000)));
            segments.push(Segment::new(format!("ctx:{}%", ctx.percent()), ctx.tone()));
        }
        None => segments.push(Segment::plain(input.model.to_string())),
    }
    segments
}

pub struct Line2Input<'a> {
    /// The active provider id, e.g. "freellmapi". No theme name here — the
    /// footer isn't the place for that.
    pub provider: Option<&'a str>,
    /// e.g. "sandbox off" / "sandboxed" — `None` when the TUI doesn't know.
    pub sandbox_state: Option<&'a str>,
    pub running_tools: usize,
    pub agent_count: usize,
    pub queued: bool,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub turn_elapsed_s: Option<u64>,
    pub scroll_label: &'a str,
    pub notice: &'a str,
    /// A persistent warning (e.g. "new lazar build installed — restart
    /// lazartui to use it") — unlike `notice`, never expires or gets
    /// dismissed by a keypress; it stays until the TUI restarts.
    pub stale_binary: Option<&'a str>,
}

/// Mode + activity, dot-separated by the caller; only non-empty parts are
/// returned, so an idle single-agent session with nothing queued is quiet.
pub fn status_line2(input: &Line2Input) -> Vec<Segment> {
    let mut segments = Vec::new();
    if !input.notice.is_empty() {
        segments.push(Segment::new(input.notice.to_string(), Tone::Warn));
    }
    if let Some(message) = input.stale_binary.filter(|m| !m.is_empty()) {
        segments.push(Segment::new(message.to_string(), Tone::Warn));
    }
    if let Some(provider) = input.provider.filter(|p| !p.is_empty()) {
        segments.push(Segment::dim(provider.to_string()));
    }
    if let Some(state) = input.sandbox_state.filter(|s| !s.is_empty()) {
        let tone = if state.contains("off") { Tone::Warn } else { Tone::Dim };
        segments.push(Segment::new(state.to_string(), tone));
    }
    if input.running_tools > 0 {
        segments.push(Segment::plain(format!("{} running", input.running_tools)));
    }
    if input.agent_count > 0 {
        segments.push(Segment::new(format!("{} agents", input.agent_count), Tone::Accent));
    }
    if input.queued {
        segments.push(Segment::new("queued (^X drop)".to_string(), Tone::Warn));
    }
    segments.push(Segment::dim(format!("↓{} ↑{}", input.input_tokens, input.output_tokens)));
    if let Some(seconds) = input.turn_elapsed_s.filter(|s| *s > 0) {
        let text = if seconds < 60 {
            format!("{seconds}s")
        } else {
            format!("{}m{:02}s", seconds / 60, seconds % 60)
        };
        segments.push(Segment::dim(text));
    }
    segments.push(Segment::dim(input.scroll_label.to_string()));
    segments
}

/// What the transcript+status area shows just above the composer while a
/// turn is running, à la Claude Code's "✻ Thinking… (12s · …)". Empty
/// (`None`) whenever no turn is running — no row is reserved for it then.
pub struct ActivityInput<'a> {
    pub turn_running: bool,
    /// The full command of the currently-running tool call, if any (a `Tool`
    /// block with no output yet). Only its first word is shown.
    pub running_tool_command: Option<&'a str>,
    /// Milliseconds since the last streamed assistant text delta, if any has
    /// arrived this turn.
    pub ms_since_last_text_delta: Option<u64>,
    /// The kernel's latest `phase` event value for this turn, if any:
    /// "thinking" | "reasoning" | "compacting" | "verifying".
    pub phase: Option<&'a str>,
    pub elapsed_s: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
}

/// A streamed text delta counts as "still writing" for this long after it
/// arrives, before falling back to the phase/idle label.
const WRITING_WINDOW_MS: u64 = 700;

/// The activity label only (e.g. "Running cargo", "Writing", "Thinking",
/// "Compacting context", "Working"), in priority order: a running tool call,
/// then recently-streamed text, then the kernel's phase event, then a plain
/// "Working" fallback. `None` when no turn is running.
pub fn activity_label(input: &ActivityInput) -> Option<String> {
    if !input.turn_running {
        return None;
    }
    if let Some(command) = input.running_tool_command.filter(|c| !c.trim().is_empty()) {
        let program = command.split_whitespace().next().unwrap_or(command.trim());
        return Some(format!("Running {program}"));
    }
    if input.ms_since_last_text_delta.is_some_and(|ms| ms < WRITING_WINDOW_MS) {
        return Some("Writing".to_string());
    }
    let phase_label = input.phase.and_then(|phase| match phase.to_ascii_lowercase().as_str() {
        "thinking" => Some("Thinking"),
        "reasoning" => Some("Reasoning"),
        "compacting" => Some("Compacting context"),
        "verifying" => Some("Verifying"),
        _ => None,
    });
    Some(phase_label.unwrap_or("Working").to_string())
}

fn format_k(tokens: u64) -> String {
    if tokens >= 1000 {
        format!("{:.1}k", tokens as f64 / 1000.0)
    } else {
        tokens.to_string()
    }
}

/// The full activity line: `<spinner> <Label>… (<elapsed>s · ↓in ↑out
/// tokens · ctrl+c to interrupt)` — spinner+label in one (accent) segment,
/// the rest dim. `None` when idle, so the caller reserves no row.
pub fn activity_line(input: &ActivityInput, spinner: &str) -> Option<Vec<Segment>> {
    let label = activity_label(input)?;
    let head = format!("{spinner} {label}…");
    let tail = format!(
        "({}s · ↓{} ↑{} tokens · ctrl+c to interrupt)",
        input.elapsed_s,
        format_k(input.input_tokens),
        format_k(input.output_tokens)
    );
    Some(vec![Segment::new(head, Tone::Accent), Segment::dim(tail)])
}

/// An mtime + inode snapshot of a file on disk, cheap to `stat()` and compare
/// without re-reading the file itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileFingerprint {
    pub mtime_secs: i64,
    pub inode: u64,
}

/// True when `current` looks like a rebuilt/replaced file relative to
/// `baseline`: a strictly newer mtime, or the same-or-older mtime but a
/// different inode (a reproducible or clock-skewed build that replaced the
/// file in place). Missing either fingerprint (the path couldn't be stat'd,
/// at startup or now) never reports a change — a transient stat failure
/// shouldn't nag the user with a false "new build" notice.
pub fn binary_replaced(baseline: Option<FileFingerprint>, current: Option<FileFingerprint>) -> bool {
    match (baseline, current) {
        (Some(base), Some(cur)) => cur.mtime_secs > base.mtime_secs || cur.inode != base.inode,
        _ => false,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentRow {
    pub glyph: String,
    pub tone: Tone,
    pub text: String,
}

/// Up to `max` rows for the agents panel, plus how many were left out.
/// `spinner` is the current animation frame for running/starting jobs.
pub fn agent_rows(jobs: &[Job], spinner: &str, max: usize) -> (Vec<AgentRow>, usize) {
    let rows = jobs
        .iter()
        .take(max)
        .map(|job| {
            let (glyph, tone) = if job.is_running() {
                (spinner.to_string(), Tone::Accent)
            } else if job.is_ok() {
                ("✓".to_string(), Tone::Ok)
            } else {
                ("✗".to_string(), Tone::Fail)
            };
            let text = format!(
                "{}  {}  {}s · {} tools · ↓{}k tokens",
                job.task,
                job.status,
                job.elapsed_s,
                job.tool_calls,
                job.input_tokens / 1000
            );
            AgentRow { glyph, tone, text }
        })
        .collect();
    (rows, jobs.len().saturating_sub(max))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn job(status: &str) -> Job {
        Job { status: status.into(), ..Job::default() }
    }

    #[test]
    fn context_tone_follows_the_documented_thresholds() {
        let low = ContextInfo { input_tokens: 10_000, output_tokens: 0, window: 100_000, ..Default::default() };
        assert_eq!(low.percent(), 10);
        assert_eq!(low.tone(), Tone::Ok);

        let mid = ContextInfo { input_tokens: 50_000, output_tokens: 0, window: 100_000, ..Default::default() };
        assert_eq!(mid.percent(), 50);
        assert_eq!(mid.tone(), Tone::Warn);

        let high = ContextInfo { input_tokens: 81_000, output_tokens: 0, window: 100_000, ..Default::default() };
        assert_eq!(high.percent(), 81);
        assert_eq!(high.tone(), Tone::Fail);

        // Window unknown: no divide-by-zero, reads as empty/ok.
        let unknown = ContextInfo { input_tokens: 10, output_tokens: 0, window: 0, ..Default::default() };
        assert_eq!(unknown.percent(), 0);
    }

    #[test]
    fn line1_includes_context_only_when_known() {
        let ctx = ContextInfo {
            input_tokens: 50_000,
            output_tokens: 0,
            window: 100_000,
            model: "sonnet-5".into(),
            served_model: "sonnet-5".into(),
        };
        let with_ctx = status_line1(&Line1Input {
            user: "jason", host: "box", cwd: "~/lazar", model: "sonnet-5", context: Some(&ctx),
        });
        assert!(with_ctx.iter().any(|s| s.text.contains("100k ctx")));
        let ctx_seg = with_ctx.iter().find(|s| s.text.starts_with("ctx:")).unwrap();
        assert_eq!(ctx_seg.text, "ctx:50%");
        assert_eq!(ctx_seg.tone, Tone::Warn);

        let without_ctx = status_line1(&Line1Input {
            user: "jason", host: "box", cwd: "~/lazar", model: "sonnet-5", context: None,
        });
        assert!(!without_ctx.iter().any(|s| s.text.contains("ctx")));
        assert!(without_ctx.iter().any(|s| s.text == "sonnet-5"));
    }

    #[test]
    fn line1_shows_served_model_and_flags_a_router_alias() {
        // No router involved: configured == served, shown plainly.
        let direct = ContextInfo {
            window: 100_000,
            model: "sonnet-5".into(),
            served_model: "sonnet-5".into(),
            ..Default::default()
        };
        let direct_line = status_line1(&Line1Input {
            user: "j", host: "h", cwd: "~", model: "sonnet-5", context: Some(&direct),
        });
        assert!(direct_line.iter().any(|s| s.text.starts_with("sonnet-5 (")));

        // Router alias configured, a concrete model served: show both.
        let routed = ContextInfo {
            window: 100_000,
            model: "auto".into(),
            served_model: "glm-4.6".into(),
            ..Default::default()
        };
        let routed_line = status_line1(&Line1Input {
            user: "j", host: "h", cwd: "~", model: "auto", context: Some(&routed),
        });
        assert!(
            routed_line.iter().any(|s| s.text.starts_with("glm-4.6 (auto) (")),
            "{routed_line:?}"
        );
    }

    #[test]
    fn line2_omits_empty_segments() {
        let quiet = status_line2(&Line2Input {
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
            stale_binary: None,
        });
        // Only the always-present tokens + scroll segments.
        assert_eq!(quiet.len(), 2, "{quiet:?}");
        assert!(!quiet.iter().any(|s| s.tone == Tone::Warn || s.tone == Tone::Accent));

        let busy = status_line2(&Line2Input {
            provider: Some("freellmapi"),
            sandbox_state: Some("sandbox off"),
            running_tools: 2,
            agent_count: 3,
            queued: true,
            input_tokens: 10,
            output_tokens: 20,
            turn_elapsed_s: Some(75),
            scroll_label: "bottom",
            notice: "",
            stale_binary: Some("new lazar build installed — restart lazartui to use it"),
        });
        let texts: Vec<&str> = busy.iter().map(|s| s.text.as_str()).collect();
        assert!(texts.contains(&"freellmapi"));
        assert!(texts.contains(&"sandbox off"));
        assert!(texts.contains(&"2 running"));
        assert!(texts.contains(&"3 agents"));
        assert!(texts.contains(&"queued (^X drop)"));
        assert!(texts.contains(&"1m15s"));
        assert!(texts.contains(&"new lazar build installed — restart lazartui to use it"));
        let agents_tone = busy.iter().find(|s| s.text == "3 agents").unwrap().tone;
        assert_eq!(agents_tone, Tone::Accent);
        let sandbox_tone = busy.iter().find(|s| s.text == "sandbox off").unwrap().tone;
        assert_eq!(sandbox_tone, Tone::Warn);
        let stale_tone = busy
            .iter()
            .find(|s| s.text == "new lazar build installed — restart lazartui to use it")
            .unwrap()
            .tone;
        assert_eq!(stale_tone, Tone::Warn);
    }

    fn activity(turn_running: bool) -> ActivityInput<'static> {
        ActivityInput {
            turn_running,
            running_tool_command: None,
            ms_since_last_text_delta: None,
            phase: None,
            elapsed_s: 12,
            input_tokens: 1_200,
            output_tokens: 300,
        }
    }

    #[test]
    fn activity_label_is_none_while_idle_so_no_row_is_reserved() {
        assert_eq!(activity_label(&activity(false)), None);
        assert_eq!(activity_line(&activity(false), "⠋"), None);
    }

    #[test]
    fn activity_label_follows_priority_order() {
        // 1. A running tool call wins over everything else.
        let mut input = activity(true);
        input.running_tool_command = Some("cargo build --release");
        input.ms_since_last_text_delta = Some(10);
        input.phase = Some("thinking");
        assert_eq!(activity_label(&input).as_deref(), Some("Running cargo"));

        // 2. No tool running: recent text delta wins over phase.
        input.running_tool_command = None;
        assert_eq!(activity_label(&input).as_deref(), Some("Writing"));

        // 3. No tool, no recent text: the phase event.
        input.ms_since_last_text_delta = Some(5_000);
        assert_eq!(activity_label(&input).as_deref(), Some("Thinking"));

        // 4. Nothing at all: the plain fallback.
        input.phase = None;
        assert_eq!(activity_label(&input).as_deref(), Some("Working"));
    }

    #[test]
    fn activity_label_maps_every_known_phase_case_insensitively() {
        let mut input = activity(true);
        for (raw, label) in [
            ("thinking", "Thinking"),
            ("REASONING", "Reasoning"),
            ("compacting", "Compacting context"),
            ("Verifying", "Verifying"),
            ("something-unknown", "Working"),
        ] {
            input.phase = Some(raw);
            assert_eq!(activity_label(&input).as_deref(), Some(label), "phase={raw}");
        }
    }

    #[test]
    fn activity_label_writing_window_expires() {
        let mut input = activity(true);
        input.ms_since_last_text_delta = Some(699);
        assert_eq!(activity_label(&input).as_deref(), Some("Writing"));
        input.ms_since_last_text_delta = Some(700);
        assert_eq!(activity_label(&input).as_deref(), Some("Working"), "the window has closed");
    }

    #[test]
    fn activity_line_formats_elapsed_and_k_tokens_with_the_real_interrupt_key() {
        let mut input = activity(true);
        input.phase = Some("thinking");
        let segments = activity_line(&input, "⠋").unwrap();
        assert_eq!(segments[0].text, "⠋ Thinking…");
        assert_eq!(segments[0].tone, Tone::Accent);
        assert_eq!(segments[1].text, "(12s · ↓1.2k ↑300 tokens · ctrl+c to interrupt)");
        assert_eq!(segments[1].tone, Tone::Dim);
    }

    #[test]
    fn binary_replaced_only_flags_a_strictly_newer_mtime_or_a_changed_inode() {
        let base = FileFingerprint { mtime_secs: 1_000, inode: 7 };
        let older = FileFingerprint { mtime_secs: 900, inode: 7 };
        let same = base;
        let newer = FileFingerprint { mtime_secs: 1_100, inode: 7 };
        let same_time_different_inode = FileFingerprint { mtime_secs: 1_000, inode: 8 };

        assert!(!binary_replaced(Some(base), Some(older)), "older mtime: not a rebuild");
        assert!(!binary_replaced(Some(base), Some(same)), "unchanged: not a rebuild");
        assert!(binary_replaced(Some(base), Some(newer)), "newer mtime: rebuilt");
        assert!(
            binary_replaced(Some(base), Some(same_time_different_inode)),
            "same mtime but a different inode: replaced in place"
        );
        assert!(!binary_replaced(None, Some(newer)), "missing baseline: no false positive");
        assert!(!binary_replaced(Some(base), None), "missing file now: no false positive");
        assert!(!binary_replaced(None, None), "missing both: no false positive");
    }

    #[test]
    fn agent_rows_cap_at_max_and_report_the_remainder() {
        let jobs = vec![job("running"), job("ok"), job("fail"), job("lost"), job("starting")];
        let (rows, more) = agent_rows(&jobs, "⠋", 4);
        assert_eq!(rows.len(), 4);
        assert_eq!(more, 1);
        assert_eq!(rows[0].tone, Tone::Accent);
        assert_eq!(rows[0].glyph, "⠋");
        assert_eq!(rows[1].tone, Tone::Ok);
        assert_eq!(rows[1].glyph, "✓");
        assert_eq!(rows[2].tone, Tone::Fail);
        assert_eq!(rows[2].glyph, "✗");
        assert_eq!(rows[3].tone, Tone::Fail, "lost is not ok, so it fails closed");

        let (rows, more) = agent_rows(&[], "⠋", 4);
        assert!(rows.is_empty());
        assert_eq!(more, 0);
    }
}
