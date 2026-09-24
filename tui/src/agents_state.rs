//! State for the full-screen Agents view (Left key from the composer) and
//! its job-detail sub-screen (Enter on a row). Kept ratatui-free so both
//! `app.rs` (key handling, polling) and `render::agents` (drawing) can use
//! it without a rendering dependency here.

use crate::jobs::{Job, JobsHandle};
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
    /// while the job is still running/starting.
    pub fetch: Receiver<Result<Value, String>>,
    /// Empty while `loading` is true and no fetch has landed yet.
    pub blocks: Vec<Block>,
    pub loading: bool,
    pub error: Option<String>,
    /// Enter or Ctrl+T: unfold every `Block::Tool` in `blocks` at once.
    pub tools_unfolded: bool,
    /// Lines skipped from the top of the rendered timeline.
    pub scroll: u16,
    pub last_poll: Instant,
}

pub struct AgentsScreenState {
    pub jobs_handle: JobsHandle,
    pub jobs: Vec<Job>,
    /// Index into `jobs::selectable_jobs(&jobs)` — Working, then Needs
    /// attention, then Completed, with no slot for a section header.
    pub selected: usize,
    pub detail: Option<JobDetailState>,
}
