//! State for the full-screen Agents view (Left key from the composer) and
//! its job-detail sub-screen (Enter on a row). Kept ratatui-free so both
//! `app.rs` (key handling, polling) and `render::agents` (drawing) can use
//! it without a rendering dependency here.

use crate::composer::Composer;
use crate::jobs::{Job, JobsHandle, SendOutcome};
use crate::transcript::Block;
use serde_json::Value;
use std::sync::mpsc::Receiver;
use std::time::Instant;

pub struct JobDetailState {
    pub job_id: String,
    pub task: String,
    pub status: String,
    pub elapsed_s: u64,
    /// The in-flight `jobs --show` fetch: the initial one, or a refresh
    /// while the job is still running/starting (or right after a send).
    pub fetch: Receiver<Result<Value, String>>,
    /// Empty while `loading` is true and no fetch has landed yet.
    pub blocks: Vec<Block>,
    pub loading: bool,
    pub error: Option<String>,
    /// Enter or Ctrl+T: unfold every `Block::Tool` in `blocks` at once.
    pub tools_unfolded: bool,
    /// Parked-scroll bookkeeping, same convention as the main transcript
    /// (`app::preserve_parked_scroll`): 0 means "follow the bottom".
    pub scroll_from_bottom: usize,
    pub previous_visual_lines: usize,
    pub last_poll: Instant,
    /// The steer/reply composer.
    pub composer: Composer,
    /// Whether this job has a session to continue (`jobs --send` works).
    /// Old jobs with no session leave the composer disabled.
    pub can_continue: bool,
    /// Text just sent, shown as an optimistic bubble until the next
    /// `jobs --show` timeline contains it (see `timeline::timeline_contains_user_text`).
    pub pending_send: Option<String>,
    /// The in-flight `jobs --send` call, if one hasn't resolved yet.
    pub sending: Option<Receiver<Result<SendOutcome, String>>>,
}

/// While the job runs: "steer this agent…"; once it's finished (and still
/// continuable): "reply to continue…". Pulled out as a pure function so the
/// placeholder choice is testable without a terminal.
pub fn composer_placeholder(status: &str) -> &'static str {
    if matches!(status, "running" | "starting") {
        "steer this agent…"
    } else {
        "reply to continue…"
    }
}

pub struct AgentsScreenState {
    pub jobs_handle: JobsHandle,
    pub jobs: Vec<Job>,
    /// Index into `jobs::selectable_jobs(&jobs)` — Working, then Needs
    /// attention, then Completed, with no slot for a section header.
    pub selected: usize,
    pub detail: Option<JobDetailState>,
    /// The new-task composer at the bottom of the list.
    pub composer: Composer,
    /// The in-flight `jobs --start` call, if a new task was just submitted.
    pub starting: Option<Receiver<Result<Job, String>>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn composer_placeholder_follows_job_status() {
        assert_eq!(composer_placeholder("running"), "steer this agent…");
        assert_eq!(composer_placeholder("starting"), "steer this agent…");
        assert_eq!(composer_placeholder("ok"), "reply to continue…");
        assert_eq!(composer_placeholder("fail"), "reply to continue…");
    }
}
