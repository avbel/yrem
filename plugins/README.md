# yrem harness plugins

Four minimal plugins. Each one only calls the `yrem` CLI (`yrem hook <event> --harness <h>`);
digest rendering, the relevance gate, dedup, session state, capture and `think` all live in the
binary. The plugins carry no server config: `yrem` reads `~/.config/yrem/config.toml` or
`YREM_URL` / `YREM_TOKEN` itself, and `yrem` 0.2.1 or newer must be on `PATH`. With an older
`yrem` (no `hook` command) every hook is a no-op: commands end in `|| true`, and the TypeScript
plugins treat any failure as no context.

| Harness | Install | session-start | prompt | pre-compact | session-end |
|---|---|---|---|---|---|
| Claude Code | `claude plugin marketplace add avbel/yrem` then `claude plugin install yrem@yrem` | `SessionStart` | `UserPromptSubmit` | `PreCompact` | `SessionEnd` (`--detach`) |
| Codex | `codex plugin marketplace add avbel/yrem` then `codex plugin add yrem@yrem` | `SessionStart` (startup, resume, clear) | `UserPromptSubmit` | `PreCompact` | `SessionEnd` (`--detach`) |
| OpenCode V2 | `opencode plugin add 'github:avbel/yrem#main::path:plugins/opencode'` | `context` hook, fresh user turn | `context` hook, fresh user turn | `compaction` hook | `session.deleted` event (`--detach`) |
| pi | `pi install ./plugins/pi` (from a clone of this repo) | `session_start` | `context` | none | `session_shutdown` (`--detach`) |

Claude Code and Codex pipe their own hook JSON to `yrem`, which understands it as is. OpenCode
and pi build the JSON themselves (`session_id`, `cwd`, `prompt`, `turns`) and use `--harness plain`,
where `yrem` prints the raw context text and the plugin appends it as background data.

Settings are `YREM_HOOK_*` environment variables, documented in
[the main README](../README.md#agent-hooks).

## Things to know

- `yrem` injects the digest once per session even when two hooks fire it (say, the Claude plugin
  plus a hand-written `yrem hook session-start` in `~/.claude/settings.json`), and again only
  after `compact` or `clear`. Recall never repeats a memory already shown in the session.
- Capture (`YREM_HOOK_CAPTURE=1`) sends each user line at most once per session and skips lines
  already in memory (semantic similarity ≥ 0.92), so `PreCompact` and `SessionEnd` never store
  the same fact twice.
- Do not run these next to the `yantrikdb-*` plugins; every prompt would be recalled twice.
- The plugins do not bundle the MCP server, so the tools are never registered twice. Add it
  yourself:
  - Claude Code: `claude mcp add --scope user yrem -- yrem mcp`
  - Codex: a `[mcp_servers.yrem]` table in `~/.codex/config.toml` with `command = "yrem"` and `args = ["mcp"]`
  - OpenCode: an `mcp` entry in `opencode.json` with `"type": "local"` and `"command": ["yrem", "mcp"]`
  - pi: `pi mcp add yrem -- yrem mcp`

## Host caveats

- OpenCode (verified with `opencode run`, 2026-10-31: the plugin loads, the briefing reaches the
  system prompt): the host rebuilds `system` for every model call, so the plugin keeps the last
  briefing `yrem` returned for each session and re-appends it on every fresh user turn. The
  helper kills `yrem` after 10 s and treats any failure as no context. Capture runs on
  `compaction` only: `session.deleted` carries no messages.
- pi (verified 2026-10-31: the extension spawns `yrem`, the child gets `HOME` and `PATH`, so
  `yrem` finds its own config, and recalled memories reach the model): the helper kills `yrem`
  after 1.7 s on the prompt path and 10 s at session start. If `spawn` throws, the plugin
  silently does nothing. pi has no pre-compact event, so capture runs only at
  `session_shutdown`.
- Codex: its `SessionStart` matcher has no `compact` source, so the briefing is not re-injected
  after a Codex compaction.
- The Claude Code, Codex and pi plugins bundle the `yrem` skill (`skills/yrem/SKILL.md`). For
  OpenCode, link it into `~/.config/opencode/skills/yrem`.
