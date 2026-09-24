//! Images the operator attached (`[Image #N: /path]`, written by lazartui for
//! dropped/pasted files) are sent to the model as real Anthropic `image`
//! blocks, not just paths. Formats the API does not take (HEIC, TIFF, …) and
//! oversized files are converted/shrunk with macOS `sips`.

use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Largest image sent as is; bigger ones are shrunk to `MAX_EDGE` pixels.
const MAX_BYTES: u64 = 3_500_000;
const MAX_EDGE: &str = "1568";

/// What one image counts for when sizing a request: roughly its token cost
/// (~1.6k tokens at 3 bytes each), not its base64 length, which would make
/// a single screenshot trigger compaction every round.
const IMAGE_WEIGHT: usize = 6_000;

/// Serialized size of `value` with every base64 image counted as `IMAGE_WEIGHT`.
pub(crate) fn weighted_len(value: &Value) -> usize {
    let raw = value.to_string().len();
    let mut payload = 0;
    let mut images = 0;
    visit(value, &mut |v| {
        if v["type"] == "image" {
            if let Some(data) = v["source"]["data"].as_str() {
                payload += data.len();
                images += 1;
            }
        }
    });
    raw.saturating_sub(payload) + images * IMAGE_WEIGHT
}

fn visit(value: &Value, f: &mut dyn FnMut(&Value)) {
    f(value);
    match value {
        Value::Array(items) => items.iter().for_each(|v| visit(v, f)),
        Value::Object(map) => map.values().for_each(|v| visit(v, f)),
        _ => {}
    }
}

/// Paths of `[Image #N: <path>]` tokens in `text`, in order.
pub(crate) fn attached_paths(text: &str) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find("[Image #") {
        rest = &rest[start + "[Image #".len()..];
        let Some(colon) = rest.find(": ") else { break };
        let digits = &rest[..colon];
        let Some(end) = rest[colon..].find(']') else { break };
        if !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit()) {
            paths.push(PathBuf::from(&rest[colon + 2..colon + end]));
        }
        rest = &rest[colon + end..];
    }
    paths
}

fn media_type(path: &Path) -> Option<&'static str> {
    match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "gif" => Some("image/gif"),
        "webp" => Some("image/webp"),
        _ => None,
    }
}

/// Bytes + media type the API accepts, converting with `sips` when needed.
fn load(path: &Path) -> Result<(Vec<u8>, &'static str), String> {
    let size = std::fs::metadata(path).map_err(|e| format!("{}: {e}", path.display()))?.len();
    if let Some(kind) = media_type(path) {
        if size <= MAX_BYTES {
            return std::fs::read(path).map(|b| (b, kind)).map_err(|e| format!("{}: {e}", path.display()));
        }
    }
    let out = std::env::temp_dir().join(format!("lazar-image-{}-{}.png", std::process::id(), size));
    let status = Command::new("sips")
        .args(["-s", "format", "png", "-Z", MAX_EDGE])
        .arg(path)
        .arg("--out")
        .arg(&out)
        .output()
        .map_err(|e| format!("cannot convert {} (sips): {e}", path.display()))?;
    if !status.status.success() {
        return Err(format!("cannot convert {}: {}", path.display(), String::from_utf8_lossy(&status.stderr).trim()));
    }
    let bytes = std::fs::read(&out).map_err(|e| format!("{}: {e}", out.display()));
    let _ = std::fs::remove_file(&out);
    bytes.map(|b| (b, "image/png"))
}

fn base64(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(TABLE[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// Content for a user message: one `image` block per attached image, then
/// the text. `None` when the text attaches no images. Unreadable images are
/// reported in `errors` and left as their path in the text.
pub(crate) fn user_content(text: &str, errors: &mut Vec<String>) -> Option<Value> {
    let paths = attached_paths(text);
    if paths.is_empty() {
        return None;
    }
    let mut blocks: Vec<Value> = Vec::new();
    for path in paths {
        match load(&path) {
            Ok((bytes, kind)) => blocks.push(json!({
                "type": "image",
                "source": {"type": "base64", "media_type": kind, "data": base64(&bytes)},
            })),
            Err(err) => errors.push(err),
        }
    }
    if blocks.is_empty() {
        return None;
    }
    blocks.push(json!({"type": "text", "text": text}));
    Some(Value::Array(blocks))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_image_tokens_but_not_files_or_plain_text() {
        let text = "see [Image #1: /tmp/a b.png] and [File #2: /tmp/n.md] then [Image #3: /x/y.heic] [Image #x: nope]";
        assert_eq!(attached_paths(text), vec![PathBuf::from("/tmp/a b.png"), PathBuf::from("/x/y.heic")]);
        assert!(attached_paths("no images").is_empty());
    }

    #[test]
    fn images_weigh_their_token_cost_not_their_base64() {
        let big = json!([{"role":"user","content":[{"type":"image","source":{"type":"base64","media_type":"image/png","data":"A".repeat(5_000_000)}},{"type":"text","text":"hi"}]}]);
        assert!(weighted_len(&big) < 10_000, "{}", weighted_len(&big));
    }

    #[test]
    fn base64_matches_the_standard_alphabet_and_padding() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn builds_image_blocks_before_the_text() {
        let path = std::env::temp_dir().join(format!("lazar-img-{}.png", std::process::id()));
        std::fs::write(&path, b"\x89PNG fake").unwrap();
        let text = format!("what is this [Image #1: {}]", path.display());
        let mut errors = Vec::new();
        let content = user_content(&text, &mut errors).unwrap();
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(content[0]["type"], "image");
        assert_eq!(content[0]["source"]["media_type"], "image/png");
        assert_eq!(content[0]["source"]["data"], base64(b"\x89PNG fake"));
        assert_eq!(content[1]["text"], text);
        let _ = std::fs::remove_file(&path);
    }
}
