//! Shared limits for text that enters a model request.
//!
//! There is no fixed request ceiling: the real ceiling is the model's context
//! window, and `compact` keeps a turn under it by eliding and summarising old
//! rounds. The per-item limits below are sanity bounds against a runaway item
//! (a 50 MB `cat`, a looping stream), set well above any useful message.

use crate::text::head_bytes;

pub(crate) const MAX_CONTEXT_ITEM_BYTES: usize = 256_000;
pub(crate) const MAX_SKILL_BODY_BYTES: usize = 48_000;
pub(crate) const MAX_HOOK_CONTEXT_BYTES: usize = 16_000;
/// Default tool output kept per call; `LAZAR_TOOL_OUTPUT_MAX_BYTES` may raise
/// it up to one context item.
pub(crate) const MAX_TOOL_OUTPUT_BYTES: usize = 32_000;
pub(crate) const MAX_MODEL_CONTENT_BLOCKS: usize = 512;
/// Default output tokens per model call; `LAZAR_MAX_TOKENS` overrides it. A
/// reply that hits the limit is continued, not cut.
pub(crate) const MAX_OUTPUT_TOKENS: u32 = 16_384;
/// Context window assumed when the provider catalog and `LAZAR_CONTEXT_WINDOW`
/// are both silent.
pub(crate) const DEFAULT_CONTEXT_WINDOW_TOKENS: u64 = 200_000;
/// Conservative bytes-per-token used to turn a token window into a byte
/// budget. Code and JSON tokenise densely, so this errs towards compacting
/// early rather than overflowing.
pub(crate) const BYTES_PER_TOKEN: u64 = 3;

/// Byte budgets derived from one model's context window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Window {
    pub tokens: u64,
    /// Request bytes above which the loop compacts before sending.
    pub compact_at: usize,
    /// Request bytes compaction aims for, leaving room for the next rounds.
    pub compact_to: usize,
    /// Prior-session history replayed into a new turn.
    pub history: usize,
}

impl Window {
    pub(crate) fn new(tokens: u64, max_output_tokens: u32) -> Self {
        let tokens = tokens.max(8_000);
        let input_tokens = tokens.saturating_sub(max_output_tokens as u64).max(4_000);
        let budget = input_tokens.saturating_mul(BYTES_PER_TOKEN) as usize;
        Self {
            tokens,
            compact_at: budget / 10 * 8,
            compact_to: budget / 2,
            history: budget / 2,
        }
    }

    /// Window for this invocation: `LAZAR_CONTEXT_WINDOW`, then the catalog
    /// value, then the default.
    pub(crate) fn resolve(catalog_tokens: Option<u64>, max_output_tokens: u32) -> Self {
        let tokens = std::env::var("LAZAR_CONTEXT_WINDOW")
            .ok()
            .and_then(|s| s.trim().parse::<u64>().ok())
            .filter(|n| *n > 0)
            .or(catalog_tokens)
            .unwrap_or(DEFAULT_CONTEXT_WINDOW_TOKENS);
        Self::new(tokens, max_output_tokens)
    }
}

/// Model response bytes allowed for one call: the output-token budget at a
/// generous bytes-per-token, so only a runaway stream trips it.
pub(crate) fn max_response_bytes(max_output_tokens: u32) -> usize {
    (max_output_tokens as usize).saturating_mul(16).max(256_000)
}

pub(crate) trait ContextualUserFragment {
    fn source(&self) -> &'static str;
    fn text(&self) -> &str;
}

pub(crate) struct OperatorPrompt<'a>(pub(crate) &'a str);

impl ContextualUserFragment for OperatorPrompt<'_> {
    fn source(&self) -> &'static str {
        "operator prompt"
    }

    fn text(&self) -> &str {
        self.0
    }
}

pub(crate) struct ExpandedSkillPrompt<'a>(pub(crate) &'a str);

impl ContextualUserFragment for ExpandedSkillPrompt<'_> {
    fn source(&self) -> &'static str {
        "skill-expanded prompt"
    }

    fn text(&self) -> &str {
        self.0
    }
}

pub(crate) fn validate(fragment: &impl ContextualUserFragment) -> Result<(), String> {
    validate_text(fragment.source(), fragment.text())
}

pub(crate) fn validate_text(source: &str, text: &str) -> Result<(), String> {
    let bytes = text.len();
    if bytes <= MAX_CONTEXT_ITEM_BYTES {
        return Ok(());
    }
    Err(format!(
        "{} is {bytes} bytes; the per-item context limit is {MAX_CONTEXT_ITEM_BYTES} bytes",
        source
    ))
}

pub(crate) fn push_bounded(
    buffer: &mut String,
    addition: &str,
    source: &str,
) -> Result<(), String> {
    let next = buffer.len().saturating_add(addition.len());
    if next > MAX_CONTEXT_ITEM_BYTES {
        return Err(format!(
            "{source} exceeded the per-item context limit ({next} > {MAX_CONTEXT_ITEM_BYTES} bytes)"
        ));
    }
    buffer.push_str(addition);
    Ok(())
}

pub(crate) fn truncate_item(text: &str) -> String {
    if text.len() <= MAX_CONTEXT_ITEM_BYTES {
        return text.to_string();
    }
    let head = head_bytes(text, MAX_CONTEXT_ITEM_BYTES.saturating_sub(80));
    let omitted = text.len().saturating_sub(head.len());
    format!("{head}\n[… {omitted} bytes omitted from replayed context]")
}

pub(crate) fn sanitize_metadata(value: &str, max_bytes: usize) -> String {
    let normalized = value
        .chars()
        .map(|ch| if ch.is_control() { ' ' } else { ch })
        .collect::<String>();
    head_bytes(normalized.trim(), max_bytes).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fragment_limit_rejects_one_oversized_item() {
        let text = "x".repeat(MAX_CONTEXT_ITEM_BYTES + 1);
        let error = validate(&OperatorPrompt(&text)).unwrap_err();
        assert!(error.contains("operator prompt"));
        assert!(error.contains(&MAX_CONTEXT_ITEM_BYTES.to_string()));
    }

    #[test]
    fn window_budgets_scale_with_the_model() {
        let small = Window::new(32_000, 8_192);
        let large = Window::new(1_000_000, 16_384);
        assert!(small.compact_to < small.compact_at);
        assert!(small.compact_at < (32_000 * BYTES_PER_TOKEN) as usize);
        assert!(large.compact_at > 1_000_000);
        assert_eq!(small.history, small.compact_to);
    }

    #[test]
    fn truncation_and_metadata_stay_utf8_safe() {
        let text = "é".repeat(MAX_CONTEXT_ITEM_BYTES);
        let bounded = truncate_item(&text);
        assert!(bounded.len() <= MAX_CONTEXT_ITEM_BYTES);
        assert!(bounded.contains("bytes omitted"));
        assert_eq!(sanitize_metadata(" bad\nclient\0 ", 40), "bad client");
    }
}
