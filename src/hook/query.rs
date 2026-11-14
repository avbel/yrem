//! The recall query built from a prompt.
//!
//! The server stores every query verbatim and counts it as demand, so the query must carry
//! only what the user typed: no editor blocks, no harness text, no secrets.

use serde_json::Value;

use super::render::clip;
use super::transcript::strip_harness;
use crate::secrets;

const MIN_PROMPT_CHARS: usize = 24;
pub const QUERY_CHARS: usize = 400;
const REDACTED: &str = "<redacted>";

/// How much of a prompt is scanned for secrets, so a pasted file costs no more than a sentence.
const SCAN_CHARS: usize = 4 * QUERY_CHARS;

/// The query for `prompt`, or `None` when there is nothing worth recalling for: harness text,
/// a slash command, a fragment, or an editor block that cannot be told from the prompt.
#[must_use]
pub fn build(prompt: &str) -> Option<String> {
    let text = strip_harness(prompt)?;
    if text.chars().count() < MIN_PROMPT_CHARS || text.starts_with('/') {
        return None;
    }
    // Redact before clipping: a secret cut in half would no longer be recognised.
    Some(clip(&redact(scanned(&text)), QUERY_CHARS))
}

/// Whether text stored by someone else (a recall query kept on the server) may be shown: no
/// secret the guard knows, no editor block.
#[must_use]
pub fn is_safe(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    !lower.contains("<ide_")
        && !lower.contains("</ide_")
        && secrets::scan(&Value::String(text.to_owned())).is_empty()
}

/// The start of `text` that is scanned and redacted: [`SCAN_CHARS`], extended to the end of the
/// token it ends in so a secret is never cut at the boundary.
fn scanned(text: &str) -> &str {
    let Some((cut, _)) = text.char_indices().nth(SCAN_CHARS) else {
        return text;
    };
    let end = text[cut..]
        .find(char::is_whitespace)
        .map_or(text.len(), |i| cut + i);
    &text[..end]
}

/// `text` with every span the secret guard flags replaced by `<redacted>`.
fn redact(text: &str) -> String {
    let mut spans: Vec<(usize, usize)> = secrets::scan(&Value::String(text.to_owned()))
        .iter()
        .map(|f| (f.start, f.end))
        .collect();
    spans.sort_unstable();
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut at = 0;
    for (start, end) in spans {
        if start >= at {
            out.extend(&chars[at..start]);
            out.push_str(REDACTED);
        }
        at = at.max(end);
    }
    out.extend(&chars[at.min(chars.len())..]);
    out
}
