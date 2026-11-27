# yrem harness plugins

Five minimal plugins. Each one only calls the `yrem` CLI (`yrem hook <event> --harness <h>`);
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
| Antigravity | `agy plugin install ./plugins/antigravity` (from a clone of this repo) | `SessionStart` | `PreInvocation`, first model call of a turn | `Stop`, every turn (`--detach`) | none |

Claude Code and Codex pipe their own hook JSON to `yrem`, which understands it as is. OpenCode
and pi build the JSON themselves (`session_id`, `cwd`, `prompt`, `turns`) and use `--harness plain`,
where `yrem` prints the raw context text and the plugin appends it as background data. Antigravity
pipes its own camelCase hook JSON to `yrem` with `--harness antigravity`, which reads it as is and
answers with `injectSteps` (one system message) or `{}`.

Settings are `YREM_HOOK_*` environment variables, documented in
[the main README](../README.md#agent-hooks).

## Things to know

- A session started in the home directory itself is the project `home` (`YREM_HOME_NAMESPACE`
  renames it) and is briefed like one; see the main README. Every write carries
  `metadata.harness`; the OpenCode and pi plugins set it through `YREM_HARNESS`.
- The project comes from the event's `cwd`: the directory name of the git repository's main
  worktree, so `fluidd-wt-306` and `.claude/worktrees/agent-*` are `fluidd`. Claude Code and
  Codex send `cwd` themselves; the OpenCode and pi plugins put it in the JSON they build. The
  project scopes the briefing (live decisions of the project plus a couple of recent ones),
  ranks and gates recall (hits of the project pass at a lower bar than the rest) and names the
  namespace of captured lines and of the tracked session. `YREM_HOOK_NAMESPACE_ALIASES` maps a
  renamed repository (`bw-lite-desktop=vaultkeeper`); an explicit `YREM_HOOK_NAMESPACE` wins.
- `yrem` injects the digest once per session even when two hooks fire it (say, the Claude plugin
  plus a hand-written `yrem hook session-start` in `~/.claude/settings.json`), and again only
  after `compact` or `clear`. Recall never repeats a memory already shown in the session, nor
  one shown in the same project within the last 24 hours (unless it matches strongly).
- Sessions run by a machine (`CLAUDE_CODE_ENTRYPOINT` starting with `sdk`, `codex exec`, Codex
  subagents) get no briefing, no recall, no tracked session and no `think`. The query sent for
  a prompt has IDE blocks cut out (a prompt whose IDE block cannot be told from its text sends
  nothing) and secrets replaced by `<redacted>`; harness text (subagent hand-backs, task
  notifications) sends none. Memories stored with `--sensitive` are never injected, nor are
  merges made from them (the hooks read a merge's sources first). The briefing asks the server
  for the digest and two listings at once and waits 8 seconds at most.
- Capture (`YREM_HOOK_CAPTURE=1`) sends each user line at most once per session and skips lines
  already in memory (semantic similarity ≥ 0.92), so `PreCompact` and `SessionEnd` never store
  the same fact twice. It skips lines that only ask for work (`commit and push`, `go on`,
  `task 387`) and reads messages typed mid-turn without Claude Code's wrapper text. It writes to
  the project namespace. Turning it on does not make recall inject captured rows, nor merges
  made from them; that is `YREM_HOOK_INJECT_CAPTURED`.
- The briefing shows only the project's rows (`YREM_HOOK_OTHER_PROJECTS=N` adds recent rows of
  other projects) and is skipped outside a git repository (`YREM_HOOK_BRIEF_WITHOUT_PROJECT=1`
  brings it back). Session start also sends writes queued while the server was unreachable.
- Do not run these next to the `yantrikdb-*` plugins; every prompt would be recalled twice.
- The plugins do not bundle the MCP server, so the tools are never registered twice. Add it
  yourself:
  - Claude Code: `claude mcp add --scope user yrem -- yrem mcp`
  - Codex: a `[mcp_servers.yrem]` table in `~/.codex/config.toml` with `command = "yrem"` and `args = ["mcp"]`
  - OpenCode: an `mcp` entry in `opencode.json` with `"type": "local"` and `"command": ["yrem", "mcp"]`
  - pi: `pi mcp add yrem -- yrem mcp`
  - Antigravity: `agy mcp add yrem yrem mcp`, or a `yrem` entry with `"command": "yrem"` and
    `"args": ["mcp"]` in `~/.gemini/config/mcp_config.json`

## Antigravity permissions and upgrades

`agy` asks for approval before every MCP tool call that `permissions.allow` in
`~/.gemini/antigravity-cli/settings.json` does not list, and headless runs (`agy -p`) deny it.
Allow yrem's tools (the server is called `yrem`, the entry is `mcp(<server>/<tool>)`):

```json
"permissions": {
  "allow": [
    "mcp(yrem/recall)", "mcp(yrem/memory_get)", "mcp(yrem/memory_current)",
    "mcp(yrem/session_digest)", "mcp(yrem/conflict_list)",
    "mcp(yrem/remember)", "mcp(yrem/correct)", "mcp(yrem/session_capture)",
    "mcp(yrem/think)", "mcp(yrem/graph_relate)", "mcp(yrem/conflict_resolve)"
  ]
}
```

`mcp(yrem/*)` allows all twelve tools, `forget` included. Leave `mcp(yrem/forget)` out to keep
deletions behind a prompt. The plugin copy lives in `~/.gemini/config/plugins/yrem`, imported by
`agy plugin install`; nothing updates it when `yrem` or this repo does. To upgrade, from a clone
at the release you want:

```bash
brew upgrade yrem                          # the binary the hooks and the MCP server call
agy plugin uninstall yrem
agy plugin install ./plugins/antigravity   # hooks, skill and manifest, copied again
agy plugin list                            # yrem should show the new version
```

Restart `agy` afterwards. The hooks only call `yrem`, so a new `yrem` binary is enough when
`hooks.json` did not change; reinstall only to refresh the manifest version or the skill.

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
- Antigravity (`agy` 1.5.16; the hook contract is from its docs and from probes of
  2026-11-24, an end-to-end run of this plugin is still to do): there is no session-end or
  pre-compact event. `Stop` fires once per user turn, so capture runs there, detached; its
  per-session watermark keeps it from storing a line twice. No tracked server session is opened
  (nothing would close it) and `think` never runs from this plugin. `PreInvocation` fires before
  every model call of the tool loop, and `yrem` recalls only when `invocationNum` is 0, the
  first call of a user turn. Injected system messages stay in the conversation, which is why
  each memory is shown once. On `--conversation` resume `SessionStart` does not fire again, so
  a resumed conversation gets no new briefing. Hooks run in the plugin's own directory; the
  project comes from the first of `workspacePaths`. `agy -p` runs are not detected as headless
  and are briefed like interactive ones. Headless `agy -p` denies MCP tools unless
  `settings.json` has `permissions.allow` with `mcp(yrem/*)`. The App started from the Dock may
  not have `/opt/homebrew/bin` on `PATH`; then every hook answers `{}`.
- The Claude Code, Codex, pi and Antigravity plugins bundle the `yrem` skill (`skills/yrem/SKILL.md`). For
  OpenCode, link it into `~/.config/opencode/skills/yrem`.
