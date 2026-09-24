//! The conversation as a list of typed blocks. Rendering lives in
//! `render::transcript`; this module only models what happened.

use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BannerKind {
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Block {
    User {
        text: String,
    },
    Assistant {
        markdown: String,
        streaming: bool,
    },
    Tool {
        id: String,
        command: String,
        output: Option<String>,
        exit_code: Option<i32>,
        folded: bool,
    },
    Banner {
        kind: BannerKind,
        text: String,
    },
    Hook {
        event: String,
        script: String,
        action: Option<String>,
        exit_code: Option<i64>,
        duration_ms: Option<u64>,
        timed_out: bool,
    },
    /// Session boundaries, provider switches, resumed-history markers.
    Marker {
        text: String,
    },
    /// Result of the kernel's post-turn grounding check on a `[VERIFY]`
    /// block the model emitted (see `App::handle_lazar_event`'s `"verify"`
    /// case).
    Verify {
        status: VerifyStatus,
        attempt: u32,
        final_: bool,
        tools_run: u64,
        claims: Vec<VerifyClaim>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerifyStatus {
    Passed,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifyClaim {
    pub claim: String,
    pub ok: bool,
    pub detail: String,
}

#[derive(Debug, Default)]
pub struct Transcript {
    pub blocks: Vec<Block>,
    /// One counter per `blocks` entry, bumped whenever that block mutates in
    /// place. Lets the render cache (`render::transcript::RenderCache`) key
    /// on a cheap integer instead of hashing `format!("{block:?}")` — which
    /// re-allocates and re-walks the whole block, output text included — on
    /// every frame for every block.
    revisions: Vec<u64>,
    /// Newly started tool chips start folded when the output is long.
    pub fold_long_tools: bool,
    /// Show hook trace blocks (Ctrl+H).
    pub show_hooks: bool,
    /// Master debug/verbose toggle (Ctrl+A / `/debug`): reveals hooks and
    /// other internal kernel events regardless of `show_hooks`. Kept
    /// separate from `show_hooks` so the finer Ctrl+H toggle still works on
    /// its own.
    pub debug: bool,
}

/// Output longer than this many lines starts folded.
pub const FOLD_THRESHOLD_LINES: usize = 12;

impl Transcript {
    pub fn new() -> Self {
        Self {
            blocks: Vec::new(),
            revisions: Vec::new(),
            fold_long_tools: true,
            show_hooks: false,
            debug: false,
        }
    }

    /// Hooks are visible when either the coarse debug toggle or the finer
    /// Ctrl+H toggle is on.
    pub fn hooks_visible(&self) -> bool {
        self.debug || self.show_hooks
    }

    /// Revision counter for `blocks[i]`, for the render cache. Bumped by
    /// every mutation below; a fresh push starts at 0.
    pub fn revision(&self, i: usize) -> u64 {
        self.revisions.get(i).copied().unwrap_or(0)
    }

    fn push_block(&mut self, block: Block) {
        self.blocks.push(block);
        self.revisions.push(0);
    }

    fn bump(&mut self, i: usize) {
        if let Some(r) = self.revisions.get_mut(i) {
            *r += 1;
        }
    }

    pub fn push_user(&mut self, text: impl Into<String>) {
        self.push_block(Block::User { text: text.into() });
    }

    pub fn push_marker(&mut self, text: impl Into<String>) {
        self.push_block(Block::Marker { text: text.into() });
    }

    #[allow(clippy::too_many_arguments)]
    pub fn push_verify(
        &mut self,
        status: VerifyStatus,
        attempt: u32,
        final_: bool,
        tools_run: u64,
        claims: Vec<VerifyClaim>,
    ) {
        self.push_block(Block::Verify {
            status,
            attempt,
            final_,
            tools_run,
            claims,
        });
    }

    pub fn push_banner(&mut self, kind: BannerKind, text: impl Into<String>) {
        self.push_block(Block::Banner {
            kind,
            text: text.into(),
        });
    }

    /// Append streamed assistant text to the open assistant block, opening
    /// one if the last block is not a streaming assistant block.
    pub fn append_assistant(&mut self, delta: &str) {
        if let Some(Block::Assistant {
            markdown,
            streaming: true,
        }) = self.blocks.last_mut()
        {
            markdown.push_str(delta);
            self.bump(self.blocks.len() - 1);
            return;
        }
        self.push_block(Block::Assistant {
            markdown: delta.to_string(),
            streaming: true,
        });
    }

    /// Close the open assistant block (text_done / end of turn).
    pub fn finish_assistant(&mut self) {
        if let Some(Block::Assistant { streaming, .. }) = self.blocks.last_mut() {
            *streaming = false;
            self.bump(self.blocks.len() - 1);
        }
    }

    /// A complete assistant message (history replay).
    pub fn push_assistant_complete(&mut self, markdown: impl Into<String>) {
        self.finish_assistant();
        self.push_block(Block::Assistant {
            markdown: markdown.into(),
            streaming: true,
        });
        self.finish_assistant();
    }

    pub fn start_tool(&mut self, id: impl Into<String>, command: impl Into<String>) {
        self.finish_assistant();
        self.push_block(Block::Tool {
            id: id.into(),
            command: command.into(),
            output: None,
            exit_code: None,
            folded: false,
        });
    }

    pub fn finish_tool(&mut self, id: &str, output: String, exit_code: Option<i32>) {
        let fold_long = self.fold_long_tools;
        let target = self
            .blocks
            .iter_mut()
            .enumerate()
            .rev()
            .find(|(_, b)| matches!(b, Block::Tool { id: tid, .. } if tid == id));
        match target {
            Some((
                i,
                Block::Tool {
                    output: slot,
                    exit_code: code,
                    folded,
                    ..
                },
            )) => {
                *folded = fold_long && output.lines().count() > FOLD_THRESHOLD_LINES;
                *slot = Some(output);
                *code = exit_code;
                self.bump(i);
            }
            _ => {
                // Result for a call we never saw start (history replay): show
                // it anyway.
                self.push_block(Block::Tool {
                    id: id.to_string(),
                    command: String::new(),
                    output: Some(output),
                    exit_code,
                    folded: false,
                });
            }
        }
    }

    /// Ctrl+T: fold every finished tool if any is open, else unfold all.
    pub fn toggle_tool_fold(&mut self) {
        let any_open = self.blocks.iter().any(|b| {
            matches!(
                b,
                Block::Tool {
                    folded: false,
                    output: Some(_),
                    ..
                }
            )
        });
        for (i, block) in self.blocks.iter_mut().enumerate() {
            if let Block::Tool {
                folded,
                output: Some(_),
                ..
            } = block
            {
                if *folded != any_open {
                    *folded = any_open;
                    self.revisions[i] += 1;
                }
            }
        }
    }

    pub fn push_hook(&mut self, event: &Value) {
        let kind = event["kind"]
            .as_str()
            .or_else(|| event["type"].as_str())
            .unwrap_or("");
        let script = event["script"].as_str().unwrap_or("?").to_string();
        let hook_event = event["event"].as_str().unwrap_or("?").to_string();
        if kind == "hook_start" {
            // Collapse start+end into one block: only record the end unless
            // the start never finishes.
            return;
        }
        self.push_block(Block::Hook {
            event: hook_event,
            script,
            action: event["action"].as_str().map(str::to_string),
            exit_code: event["exit_code"].as_i64(),
            duration_ms: event["duration_ms"].as_u64(),
            timed_out: event["timed_out"].as_bool().unwrap_or(false),
        });
    }

    pub fn clear(&mut self) {
        self.blocks.clear();
        self.revisions.clear();
    }

    /// Plain-text dump for Ctrl+D.
    pub fn to_plain_text(&self) -> String {
        let mut out = String::new();
        for block in &self.blocks {
            match block {
                Block::User { text } => out.push_str(&format!("\n▶ {text}\n")),
                Block::Assistant { markdown, .. } => out.push_str(&format!("\n{markdown}\n")),
                Block::Tool {
                    command,
                    output,
                    exit_code,
                    ..
                } => {
                    out.push_str(&format!("\n$ {command}\n"));
                    if let Some(o) = output {
                        out.push_str(o);
                        out.push('\n');
                    }
                    if let Some(c) = exit_code {
                        out.push_str(&format!("[exit {c}]\n"));
                    }
                }
                Block::Banner { kind, text } => out.push_str(&format!("\n[{kind:?}] {text}\n")),
                Block::Hook {
                    event,
                    script,
                    action,
                    ..
                } => out.push_str(&format!(
                    "\n[hook {event}] {script} → {}\n",
                    action.as_deref().unwrap_or("-")
                )),
                Block::Marker { text } => out.push_str(&format!("\n── {text} ──\n")),
                Block::Verify {
                    status,
                    attempt,
                    final_,
                    tools_run,
                    claims,
                } => {
                    out.push_str(&format!(
                        "\n[verify {status:?} attempt={attempt} final={final_} tools_run={tools_run}]\n"
                    ));
                    for claim in claims {
                        out.push_str(&format!(
                            "  {} {} — {}\n",
                            if claim.ok { "✓" } else { "✗" },
                            claim.claim,
                            claim.detail
                        ));
                    }
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn deltas_accumulate_into_one_streaming_block_until_finished() {
        let mut t = Transcript::new();
        t.append_assistant("Hel");
        t.append_assistant("lo");
        t.finish_assistant();
        t.append_assistant("Again");
        assert_eq!(t.blocks.len(), 2);
        assert!(
            matches!(&t.blocks[0], Block::Assistant { markdown, streaming: false } if markdown == "Hello")
        );
        assert!(matches!(
            &t.blocks[1],
            Block::Assistant {
                streaming: true,
                ..
            }
        ));
    }

    #[test]
    fn tool_results_pair_by_id_and_fold_long_output() {
        let mut t = Transcript::new();
        t.start_tool("t1", "ls");
        t.start_tool("t2", "cat big");
        let long: String = (0..20).map(|i| format!("line {i}\n")).collect();
        t.finish_tool("t2", long, Some(0));
        t.finish_tool("t1", "a\nb".into(), Some(1));
        match &t.blocks[0] {
            Block::Tool {
                exit_code, folded, ..
            } => {
                assert_eq!(*exit_code, Some(1));
                assert!(!folded);
            }
            other => panic!("{other:?}"),
        }
        assert!(matches!(&t.blocks[1], Block::Tool { folded: true, .. }));
        t.toggle_tool_fold();
        assert!(t
            .blocks
            .iter()
            .all(|b| matches!(b, Block::Tool { folded: true, .. })));
        t.toggle_tool_fold();
        assert!(t
            .blocks
            .iter()
            .all(|b| matches!(b, Block::Tool { folded: false, .. })));
    }

    #[test]
    fn hooks_become_blocks() {
        let mut t = Transcript::new();
        t.push_assistant_complete("Done.");
        assert!(matches!(
            &t.blocks[0],
            Block::Assistant { markdown, streaming: false } if markdown == "Done."
        ));

        t.push_hook(&json!({"kind": "hook_start", "event": "pre-tool", "script": "10-x.sh"}));
        t.push_hook(&json!({"kind": "hook_end", "event": "pre-tool", "script": "10-x.sh", "action": "veto", "exit_code": 0, "duration_ms": 12, "timed_out": false}));
        assert_eq!(t.blocks.len(), 2, "hook_start is folded into hook_end");
        assert!(matches!(&t.blocks[1], Block::Hook { action: Some(a), .. } if a == "veto"));
        assert!(t.to_plain_text().contains("[hook pre-tool] 10-x.sh → veto"));
    }

    #[test]
    fn assistant_markdown_is_never_reinterpreted() {
        // Whatever the model wrote is what the operator sees. The TUI does not
        // mine the prose for protocol markers or promote them into blocks.
        let mut t = Transcript::new();
        t.append_assistant("Done.\n[VERIFY] exists=/tmp/x [/VERIFY]\nmore text");
        t.finish_assistant();
        assert_eq!(t.blocks.len(), 1);
        assert!(matches!(
            &t.blocks[0],
            Block::Assistant { markdown, streaming: false }
                if markdown == "Done.\n[VERIFY] exists=/tmp/x [/VERIFY]\nmore text"
        ));
    }
}
