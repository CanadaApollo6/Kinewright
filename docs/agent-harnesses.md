# Agent harnesses

OpenReel owns one `rmcp` Streamable HTTP server inside the app process. It binds only to
`127.0.0.1` on an operating-system-assigned port and exposes `/mcp`. Every handler talks to the
same live Core actor used by the UI. Mutators therefore enter the normal snapshot undo stack and
emit the normal `DocumentChanged` broadcast. The workspace pins `rmcp` at the root.

OpenReel-owned sessions advertise a compact seven-tool runtime. Models inspect the current
revision, discover capabilities by name and kind, load only the selected schema, invoke non-edit
capabilities through one dispatcher, and prepare then commit timeline edits atomically. The
generated capability registry is internal and direct calls to its names are rejected. The exact
contract and measured schema overhead are documented in [M36 - Agent runtime efficiency](M36-AGENT-RUNTIME-EFFICIENCY.md).

Opening another project interrupts the current agent session, shuts down the old endpoint, and
starts a new endpoint against the replacement Core actor. No filesystem or network handle is put
in `openreel-core`.

## Claude Code

Status: supported and shown in the app when `claude` is found on `PATH`.

Transport: native Streamable HTTP. OpenReel passes a strict, process-local MCP configuration with
the ephemeral URL. No proxy or persistent change to the user's Claude MCP configuration is needed.

Protocol: one long-lived process using `claude -p --input-format stream-json --output-format
stream-json --verbose`. User turns are JSONL on stdin. Assistant text, MCP tool calls, tool results,
usage, cost, and completion are parsed from stdout JSONL behind `AgentSession`.

Safety: `--strict-mcp-config`, `--tools ""`, and an exact `--allowedTools` list containing only
`mcp__openreel__*` tools. The process uses `--permission-mode dontAsk` only after that restriction
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
exact OpenReel MCP tool names. The MCP server still routes every mutation through Core, and its
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

Transport: one long-lived `agent acp` child over ACP v1 NDJSON. OpenReel advertises no client
filesystem or terminal capability, creates the ACP session in a new empty scratch directory, and
hands `session/new` the project-local Streamable HTTP MCP endpoint inline. The driver requires the
agent's advertised HTTP MCP capability. It never writes Cursor's MCP configuration.

Protocol: a dedicated stdout reader routes JSON-RPC replies through a pending-request map and sends
`session/update` notifications plus agent-to-client requests through one channel. `session/prompt`
streams message and tool updates into the existing `AgentEvent` UI. Permission requests receive
ACP `allow_once`; destructive OpenReel operations still stop at the same `ConfirmationBroker` used
by the other harnesses. `session/cancel` is sent before the child is killed on interrupt.

Models: the picker comes from Cursor's live `cursor/list_available_models` extension. Its
`effort`/`reasoning` options feed OpenReel's Effort picker, and its `fast` option feeds the existing
Speed picker. Cursor currently persists `session/set_config_option` values as CLI-wide defaults,
even when invoked through ACP. OpenReel therefore snapshots the complete Cursor configuration,
leases configuration changes to one active Cursor turn, and restores the snapshot on completion,
failure, interrupt, and drop. A machine-level crash during the active request can still strand the
temporary choice; this is an upstream limitation until Cursor offers session-scoped config.

Safety: the model sees the OpenReel MCP endpoint and runs from an empty scratch directory. ACP can
still surface Cursor-owned tools, so this boundary is weaker than Codex's explicit feature-off
configuration and Claude's exact tool allowlist. OpenReel's transactional operation validation,
undo snapshots, edit-plan atomicity, and destructive confirmation remain the authoritative edit
boundary.

## Test gate

The live Claude and Codex subscription tests are ignored by behavior unless
`OPENREEL_AGENT_TEST=1` is set. The Cursor acceptance test has its own
`OPENREEL_CURSOR_AGENT_TEST=1` gate so it can be run alone. Each creates a two-clip project,
launches the real installed CLI once, asks it to split clip 1 at frame 30 and delete clip 2, verifies
the live document, then sends one undo command and asserts that the atomic plan is restored.
CLI-independent workspace tests never launch a subscription harness.
