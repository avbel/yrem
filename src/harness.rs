//! Which agent a write came from, stored as `metadata.harness`.

/// Overrides every other source.
pub const ENV: &str = "YREM_HARNESS";
/// Label of commands typed in a terminal or run from a script.
pub const CLI: &str = "cli";

fn from_env(env: &impl Fn(&str) -> Option<String>) -> Option<String> {
    env(ENV)
        .map(|v| v.trim().to_ascii_lowercase())
        .filter(|v| !v.is_empty())
}

/// The CLI's label: `YREM_HARNESS`, else `cli`.
#[must_use]
pub fn for_cli(env: &impl Fn(&str) -> Option<String>) -> String {
    from_env(env).unwrap_or_else(|| CLI.to_owned())
}

/// The hook's label: `YREM_HARNESS`, else the harness the hook was started for. `None` for
/// `plain`, which says nothing about the agent behind it.
#[must_use]
pub fn for_hook(env: &impl Fn(&str) -> Option<String>, named: Option<&str>) -> Option<String> {
    from_env(env).or_else(|| named.map(str::to_owned))
}

/// The MCP server's label: `YREM_HARNESS`, else the client's `clientInfo.name`.
#[must_use]
pub fn for_mcp(env: &impl Fn(&str) -> Option<String>, client: Option<&str>) -> Option<String> {
    from_env(env).or_else(|| client.and_then(from_client_name))
}

/// Maps a client's self-reported name to the label of a known harness; any other name is
/// kept, lowercased.
#[must_use]
pub fn from_client_name(name: &str) -> Option<String> {
    let lower = name.trim().to_ascii_lowercase();
    if lower.is_empty() {
        return None;
    }
    let mut tokens = lower.split(|c: char| !c.is_ascii_alphanumeric());
    let known = tokens.find_map(|t| match t {
        "claude" => Some("claude"),
        "codex" => Some("codex"),
        "antigravity" | "gemini" | "agy" => Some("antigravity"),
        "opencode" => Some("opencode"),
        "pi" => Some("pi"),
        _ => None,
    });
    Some(known.map_or(lower, str::to_owned))
}
