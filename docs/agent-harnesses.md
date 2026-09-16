# Agent harnesses

Kinewright owns one `rmcp` Streamable HTTP server inside the app process. It binds only to
`127.0.0.1` on an operating-system-assigned port and exposes `/mcp`. Every handler talks to the
same live Core actor used by the UI. Mutators therefore enter the normal snapshot undo stack and
emit the normal `DocumentChanged` broadcast. The workspace pins `rmcp` at the root.

Kinewright-owned sessions advertise a compact seven-tool runtime. Models inspect the current
revision, discover capabilities by name and kind, load only the selected schema, invoke non-edit
capabilities through one dispatcher, and prepare then commit timeline edits atomically. The
generated capability registry is internal and direct calls to its names are rejected. The exact
contract and measured schema overhead are documented in [M36 - Agent runtime efficiency](M36-AGENT-RUNTIME-EFFICIENCY.md).

Two of the ten harnesses enforce that list on the harness side: Claude Code takes an exact
`--allowedTools` allowlist and Codex takes per-server `enabled_tools`, both built from
`SessionConfig.tool_names`. The other eight drop it. ACP's `session/new` has no per-tool
allowlist at all, Muse's MSP `session/start` attaches servers rather than tools, and Copilot's
`--available-tools` names a tool filter Kinewright is not currently using correctly (see Copilot below), so for those
harnesses the surface the model sees is whatever the Kinewright MCP server itself serves. Kinewright therefore treats the
server as the authoritative tool boundary: a session that must run a restricted surface has to be
served that surface, and the driver-side allowlist is a second net where the CLI offers one.

Opening another project interrupts the current agent session, shuts down the old endpoint, and
starts a new endpoint against the replacement Core actor. No filesystem or network handle is put
in `kinewright-core`.

## Claude Code

Status: supported and shown in the app when `claude` is found on `PATH`.

Transport: native Streamable HTTP. Kinewright passes a strict, process-local MCP configuration with
the ephemeral URL. No proxy or persistent change to the user's Claude MCP configuration is needed.

Protocol: one long-lived process using `claude -p --input-format stream-json --output-format
stream-json --verbose`. User turns are JSONL on stdin. Assistant text, MCP tool calls, tool results,
usage, cost, and completion are parsed from stdout JSONL behind `AgentSession`.

Safety: `--strict-mcp-config`, `--tools ""`, and an exact `--allowedTools` list containing only
`mcp__kinewright__*` tools. The process uses `--permission-mode dontAsk` only after that restriction
is applied. The driver enforces the configured internal turn cap and `interrupt` kills the child.

## Codex

Status: supported with Codex CLI 0.147.0 or newer and shown in the app when `codex` is found on
`PATH`. A detected older version fails closed at session start.

Capability assessment (2026-08-08): Codex still does not have one switch for an MCP-only tool
surface. [openai/codex#6049](https://github.com/openai/codex/issues/6049) remains open. The 0.147.0
configuration surface does, however, support the policy-neutralized option:

- `--sandbox read-only`, `approval_policy='never'`, `web_search='disabled'`, `--ephemeral`,
  `--ignore-user-config`, and `--ignore-rules`;
- stable feature switches for `shell_tool`, `unified_exec`, `view_image`, browser/computer use,
  image generation, apps, plugins, multi-agent features, skills, and related optional tools;
- `tools.update_plan.enabled=false`, added before 0.147.0; and
- per-server `enabled_tools`, `required`, and `default_tools_approval_mode` MCP settings.

See the official [Codex configuration reference](https://developers.openai.com/codex/config-reference),
[CLI reference](https://developers.openai.com/codex/cli/reference), and
[MCP guide](https://developers.openai.com/codex/mcp). `codex app-server` uses the same policy and
tool configuration rather than offering a stronger MCP-only boundary. Codex's MCP-server mode is
the opposite direction: it exposes Codex as a tool to another client.

The empirical probe used the exact 0.147.0 executable, strict config, an empty scratch working
directory, a read-only sandbox, and one allowlisted scratch MCP tool. The captured model request
contained the scratch MCP tool plus only resource-discovery, plan, and user-input helpers. It did
not contain shell, file mutation, web, browser, computer, image, app, plugin, or agent tools. The
model called the scratch MCP tool successfully, could not create the requested marker file, and the
working directory remained empty. The shipped command additionally disables the plan helper.
Resource discovery and user-input helpers remain, which is why this is the policy-neutralized path
rather than a claim that all built-ins are gone.

Safety: every invocation ignores user config and rules, runs in a newly created empty directory,
sets project-instruction loading to zero bytes, uses the read-only sandbox and no inherited shell
environment, disables shell/file/web and other unrelated feature tools, and registers only the
exact Kinewright MCP tool names. The MCP server still routes every mutation through Core, and its
`ConfirmationBroker` still gates destructive edits. The chat panel states: "Codex sessions use a
read-only empty scratch sandbox; shell, file-write, and web tools are disabled."

Transport: native Streamable HTTP. `codex exec` receives the process-local `/mcp` URL and exact
tool allowlist through command-line config overrides, so no stdio proxy and no persistent change to
the user's Codex configuration are needed.

Protocol: each user turn starts one ephemeral `codex exec --json` process. The session wrapper
replays earlier user requests as bounded chat context while requiring a fresh live-timeline read,
enforces the configured number of user turns, and `interrupt` kills the active child. Recorded
0.147.0 JSONL includes `thread.started`, `turn.started`, `item.started` and `item.completed` for
`mcp_tool_call`, `item.completed` for `agent_message` or `error`, and `turn.completed` with token
usage. These map to `ToolCall`, `ToolResult`, `Text`/`Error`, `Cost`, and `Done`. Codex subscription
JSONL reports total, cached, output, and reasoning token categories but not a dollar price, so
`cost_usd` is normally absent. Categories omitted by a provider stay unavailable instead of being
reported as zero. Claude cache-read and cache-creation usage is normalized into the same event
contract, and eval output records both those categories and the advertised tool-surface bytes.

## Cursor

Status: supported and shown in the app when `agent` or `cursor-agent` is found on `PATH`. Detection
uses `agent status --format json`; `agent about --format json` supplies the installed CLI version
and subscription tier without exposing account identity in the UI.

Transport: one long-lived `agent acp` child over ACP v1 NDJSON on the same shared ACP session
runtime as OpenCode, Qwen, Kimi, Kiro and Devin. Kinewright advertises no client filesystem or
terminal capability, creates the ACP session in a new empty scratch directory, and hands
`session/new` the project-local Streamable HTTP MCP endpoint inline. The driver requires the
agent's advertised HTTP MCP capability. It never writes Cursor's MCP configuration.

Protocol: a dedicated stdout reader routes JSON-RPC replies through a pending-request map and sends
`session/update` notifications plus agent-to-client requests through one channel. `session/prompt`
streams message and tool updates into the existing `AgentEvent` UI, and is waited for on a turn
thread rather than the UI frame. Permission requests receive ACP `allow_once`; destructive
Kinewright operations still stop at the same `ConfirmationBroker` used by the other harnesses.
`session/cancel` is sent before the child is killed on interrupt.

Models: the picker comes from Cursor's live `cursor/list_available_models` extension. Its
`effort`/`reasoning` options feed Kinewright's Effort picker, and its `fast` option feeds the existing
Speed picker. Cursor currently persists `session/set_config_option` values as CLI-wide defaults,
even when invoked through ACP — it is the only harness that does. Kinewright therefore snapshots
the complete Cursor configuration, leases configuration changes to one active Cursor turn, and
restores the snapshot on completion, failure, interrupt, and drop. That snapshot-lease-restore is
a field on the shared runtime's per-harness spec rather than a separate driver, and it is off for
every other harness. A machine-level crash during the active request can still strand the
temporary choice; this is an upstream limitation until Cursor offers session-scoped config.

Safety: the model sees the Kinewright MCP endpoint and runs from an empty scratch directory. ACP can
still surface Cursor-owned tools and carries no per-tool allowlist, so this boundary is weaker
than Codex's explicit feature-off configuration and Claude's exact tool allowlist, and the
settings card says so. Kinewright's transactional operation validation,
undo snapshots, edit-plan atomicity, and destructive confirmation remain the authoritative edit
boundary.

## Muse

Status: supported and shown in the app when `muse` is found on `PATH`. Authenticated when
`META_API_KEY` is set or the Meta account login in `~/.config/muse/auth.json` holds a key.

Transport: one long-lived `muse serve` child speaking MSP (Muse Session Protocol, JSON-RPC 2.0
over stdio) with `--disable-shell --disable-write`. Memory-only sessions are deliberately not
used: turns submitted to a `--no-session-log` host are accepted but never execute.

Protocol: `initialize` requests the `sessionMcp` capability (fail-closed when ungranted), then
`session/start` opens one session in a new empty scratch directory with the ephemeral endpoint
attached per-session as a required Streamable HTTP server. No approval mode is named: the host
seals a startup ceiling that rejects explicit modes, so tool approvals are answered over the wire
instead. Each user turn is one `turn/start`, whose reply is an admission ack (`status`, `turnId`,
`disposition`) rather than the finished turn — confirmed against the wire schema this host
exports with `muse schema generate-json-schema`. Kinewright waits for that ack on a worker
thread, never on the UI frame, and treats the 30 s budget as the deadline for the ack alone.
Transcript items map to `Text`/`ToolCall`/`ToolResult`, per-call `session/tokenUsage` records
map to `Cost`, and `turn/completed` ends the turn.
`turn/cancel` stops a turn without killing the host, so the session stays usable after an interrupt.

Approvals: the host announces them with `approval/requested` (the must-answer `approval/request`
may never follow). Kinewright decides immediately: a one-shot `approved` choice first, then a
session-scoped one. Policy amendments are never selected, and anything unapprovable is denied so
the tool call fails fast instead of hanging the headless turn. User-input prompts are
auto-cancelled.

Models: the picker comes from the live MSP `model/list` catalog (Muse Spark 1.3/1.2, newest
first), falling back to the last verified snapshot when the host cannot be queried. All eight
reasoning efforts (`none`–`ultra`) are offered per turn.

## OpenCode

Status: supported and shown in the app when `opencode` is found on `PATH`. Authenticated when
`auth.json` in the OpenCode data directory holds any provider credential.

Transport: one long-lived `opencode acp` child over ACP v1 NDJSON, sharing the generic ACP
session runtime: no client filesystem or terminal capability, a new empty scratch directory, and
the endpoint handed to `session/new` inline. Permission requests receive ACP `allow_once`.

Protocol: each user turn is one `session/prompt`; standard `session/update` notifications stream
message and tool traffic into the existing `AgentEvent` UI. `session/cancel` precedes the kill on
interrupt.

Models: the picker comes from the session's live `model` select options — the providers the user
authenticated in `opencode auth`, which is also how DeepSeek, Z.ai (GLM), Moonshot (Kimi), and
MiniMax models are reachable today. OpenCode has no model spawn flag, so the requested model is
applied with `session/set_config_option`. Hand-run 2026-09-16 against the installed `opencode`
CLI, not covered by a test: the value is session-scoped and the user's `opencode.json` is
untouched. Cursor is the one harness where the same call persists CLI-wide. No effort or tier axis is advertised.

## Qwen Code

Status: supported and shown in the app when `qwen` is found on `PATH`. Authenticated when
`~/.qwen/oauth_creds.json` exists or `DASHSCOPE_API_KEY` is set.

Transport: one long-lived `qwen --model <id> --acp` child over ACP v1 NDJSON on the shared
runtime: empty scratch directory, inline endpoint, `allow_once` permissions. Turns are
`session/prompt` with the standard update mapping; interrupt cancels then kills.

Models: the picker comes from the session's live `model` select options. No effort or tier axis
is advertised.

## Kimi

Status: supported and shown in the app when `kimi` is found on `PATH`. Authenticated when
`~/.kimi/credentials/` holds any entry or the Kimi config sets an API key.

Transport: one long-lived `kimi --model <id> acp` child over ACP v1 NDJSON on the shared
runtime: empty scratch directory, inline endpoint, `allow_once` permissions. Turns are
`session/prompt` with the standard update mapping; interrupt cancels then kills.

Models: the picker comes from the session's live `model` select options when logged in, falling
back to the `[models.*]` tables in the user's Kimi config (listing only; running still needs
authentication). No effort or tier axis is advertised.

## Kiro

Status: supported and shown in the app when `kiro-cli` is found on `PATH`. Detection uses
`kiro-cli whoami`, which answers `Not logged in` distinctly from any authenticated identity.

Transport: one long-lived `kiro-cli acp` child over ACP v1 NDJSON on the shared runtime: empty
scratch directory, inline endpoint, requested `--model` and `--effort`. Kiro cannot answer tool
approvals headlessly over ACP, so the child runs with `-a/--trust-all-tools`; the settings panel
states this weaker boundary explicitly. Turns are `session/prompt` with the standard update
mapping; interrupt cancels then kills.

Models: the picker comes from the session's live `model` select options, and every model carries
Kiro's documented `--effort` levels (`low`–`max`). No tier axis is advertised.

## Devin

Status: supported and shown in the app when `devin` is found on `PATH`. Detection uses
`devin auth status`, which reports the login and the subscription tier shown in the UI.

Transport: one long-lived `devin acp` child over ACP v1 NDJSON on the shared runtime, with one
exception: Devin advertises no HTTP MCP capability and reads `mcp_config.json` from its config
directory instead. The session therefore gets an isolated config root holding only the Kinewright
endpoint; `XDG_CONFIG_HOME` and `%APPDATA%` are both redirected, because Windows Devin reads the
latter and setting only the former failed closed on every Windows start. Hand-run 2026-09-16
against the installed `devin` CLI, not covered by a test: with the redirect Devin reports an
`_meta.mcpConfigPath` inside Kinewright's isolated directory, and without it the user's own
`~/.config/devin/mcp_config.json`. Credentials live under `XDG_DATA_HOME` and are untouched, and
the driver fails closed when Devin reports a config path outside the isolated directory, or one
whose file does not declare the Kinewright endpoint. Turns are `session/prompt` with the standard update mapping
(Devin's narrated tool titles unwrap to bare tool names); interrupt cancels then kills.

Models: the picker comes from the session's live `model` select options. Effort and speed are
part of Devin's model ids, so no effort or tier axis is offered.

## Copilot

Status: supported and shown in the app when `copilot` is found on `PATH`. Copilot keeps its
token in the system credential store, so detection reports authenticated only for an explicit
`COPILOT_GITHUB_TOKEN` and unknown otherwise.

Transport: native headless JSONL. Each user turn starts one ephemeral `copilot -p` process in a
new empty scratch directory with `--output-format json`, `--silent` (Copilot's interactive
banner and progress chrome stay off stdout, so the stream is JSONL only), `--disable-builtin-mcps`,
and the endpoint passed per-session through `--additional-mcp-config`. No Copilot configuration
file is touched.

Protocol: the session wrapper replays earlier user requests as bounded chat context while
requiring a fresh live-timeline read, enforces the configured number of user turns, and
`interrupt` kills the active child. `assistant.message` lines carry final text plus `toolRequests`
(deduplicated against `tool.execution_start`), `tool.execution_complete` carries results, and the
terminal `result` line ends the turn. These map to `Text`/`ToolCall`/`ToolResult`/`Error` and
`Done`. Copilot reports premium requests and AI units rather than token counts, so no `Cost`
event is emitted.

Models: the picker is curated from GitHub's supported-models documentation (Copilot ships no
models-list command) and every model carries the documented `--reasoning-effort` levels. No tier
axis is advertised.

Safety: **Copilot is the weakest boundary of the ten, and Kinewright cannot currently narrow
it.** `--disable-builtin-mcps` does stop other MCP servers loading, so Kinewright's is the only
MCP server in the run. Copilot's own built-in tools are another matter. The run passes
`--available-tools kinewright`, but `copilot --help` documents that flag as
`--available-tools [<tools>...]`, "Only these tools will be available to the model" — a list of
**tool** names, beside `--allow-tool`/`--deny-tool`/`--excluded-tools`, not a server name. A bare
`kinewright` therefore names no tool Copilot knows, and the recorded run in
`tests/fixtures/copilot-stream.jsonl` shows Copilot's built-in `bash` being offered and executed.
The run also passes `--allow-all-tools`, which auto-approves every tool call rather than
prompting, because Copilot offers no headless approval channel and a prompted run would simply
hang. Taken together: a Copilot session can read and write files and run shell commands in its
scratch directory without being asked, and the settings card says so. What holds for every
harness still holds here — the timeline is reachable only through Kinewright's MCP server, every
mutation routes through Core, and the `ConfirmationBroker` gates destructive edits. Narrowing
this properly means passing the qualified tool names Copilot actually uses (it exposes MCP tools
as `kinewright-<tool>`, which is why `display_tool_name` strips that prefix) and checking the
result against a real run; that is recorded as a follow-up rather than shipped on an untested
guess. Each `-p` run also persists a session in Copilot's own session store; that is Copilot's
normal behavior.

## Test gate

The live Claude and Codex subscription tests are ignored by behavior unless
`KINEWRIGHT_AGENT_TEST=1` is set. The Cursor acceptance test has its own
`KINEWRIGHT_CURSOR_AGENT_TEST=1` gate so it can be run alone. The second-wave harnesses (Muse,
OpenCode, Qwen, Kimi, Kiro, Devin, Copilot) are `#[ignore]`d as well as gated on
`KINEWRIGHT_NEW_AGENT_TEST=1`, so the default lane reports them as ignored rather than as seven
passing no-ops; run them with

```text
KINEWRIGHT_NEW_AGENT_TEST=1 cargo test -p kinewright-agent --test new_harness_e2e -- --ignored
```

A driver whose CLI is missing or unauthenticated is skipped with a message. Each creates a two-clip
project, launches the real installed CLI once, asks it to split clip 1 at frame 30 and delete clip
2, verifies the live document, then sends one undo command and asserts that the atomic plan is
restored. CLI-independent workspace tests never launch a subscription harness.
