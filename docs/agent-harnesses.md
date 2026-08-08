# Agent harnesses

OpenReel owns one `rmcp` Streamable HTTP server inside the app process. It binds only to
`127.0.0.1` on an operating-system-assigned port and exposes `/mcp`. Every handler talks to the
same live Core actor used by the UI. Mutators therefore enter the normal snapshot undo stack and
emit the normal `DocumentChanged` broadcast. The workspace pins `rmcp` at the root.

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

Status: detected and protocol-fixture-tested, but intentionally unavailable as a runnable driver
with Codex CLI 0.142.5.

Transport, once safe: native Streamable HTTP configured through `codex exec --json -c` overrides.
Its JSONL parser already maps agent messages, MCP calls/results, usage, failures, and completion to
`AgentEvent`.

Blocker: `codex exec` currently has no supported switch that disables its built-in shell and file
tools while retaining configured MCP tools. Non-interactive MCP approval behavior also cannot give
OpenReel the same fail-closed guarantee as the Claude invocation. Launching it would give a video
editing prompt unrelated machine capabilities, so `start_session` returns a precise unavailable
error instead. Enable the driver only after the installed CLI can enforce an MCP-only allowlist.

## Test gate

The live subscription test is ignored by behavior unless `OPENREEL_AGENT_TEST=1` is set. It creates
generated media and a two-clip project, launches the real installed Claude CLI once, asks it to split
clip 1 at frame 30 and delete clip 2, verifies the live document, then sends two undo commands and
asserts that the original document is restored.
