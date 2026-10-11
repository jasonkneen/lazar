//! Transcript rendering timing; run with `cargo test --release bench_timing -- --nocapture`.
//! Measurements are diagnostic, never sleep- or elapsed-time pass thresholds.
use std::time::Instant;
use crate::render::markdown::Highlighter;
use crate::render::theme::{IconMode, Theme};
use crate::render::transcript::RenderCache;
use crate::transcript::Transcript;

fn long_transcript(turns: usize) -> Transcript {
    let mut t = Transcript::new();
    for i in 0..turns {
        t.push_user(format!("Question {i}: explain the module and show code"));
        t.append_assistant(&format!(
            "# Answer {i}\n\nSome **bold** prose with `inline` code and a list:\n\n- one\n- two\n\n\
             ```rust\nfn f{i}(x: u32) -> u32 {{\n    let y = x * 2;\n    y + {i}\n}}\n```\n\n\
             ```python\ndef g{i}(a):\n    return [v * 2 for v in a]\n```\n"
        ));
        t.finish_assistant();
    }
    t
}

#[test]
fn bench_timing() {
    let theme = Theme::nerv_with_icons(IconMode::Unicode);
    let t = long_transcript(300);
    let start = Instant::now();
    let _ = Highlighter::new();
    println!("BENCH highlighter_new_ms={:.2}", start.elapsed().as_secs_f64() * 1e3);
    let hl = Highlighter::new();
    let mut cache = RenderCache::default();
    let start = Instant::now();
    let lines = cache.lines(&t, 100, &theme, &hl, 0);
    assert!(!lines.is_empty(), "benchmark must render a real transcript");
    println!("BENCH first_paint_ms={:.2} lines={}", start.elapsed().as_secs_f64() * 1e3, lines.len());
    let frames = 200;
    let start = Instant::now();
    for f in 0..frames {
        assert!(!cache.lines(&t, 100, &theme, &hl, f % 8).is_empty());
    }
    println!("BENCH cached_frame_ms_avg={:.3} frames={}", start.elapsed().as_secs_f64() * 1e3 / frames as f64, frames);
    let frames = 5;
    let start = Instant::now();
    for _ in 0..frames {
        let mut fresh = RenderCache::default();
        assert!(!fresh.lines(&t, 100, &theme, &hl, 0).is_empty());
    }
    println!("BENCH uncached_frame_ms_avg={:.2} frames={}", start.elapsed().as_secs_f64() * 1e3 / frames as f64, frames);
}