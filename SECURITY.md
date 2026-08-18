# Security Policy

## Reporting a vulnerability

Please report security issues privately to **riel.stamand@gmail.com** (or via
GitHub's private vulnerability reporting on this repository, if enabled) rather
than opening a public issue. You should receive a response within a few days.
Please include reproduction steps and your assessment of impact.

## Scope notes: the agent sandbox model

Kinewright's most security-relevant surface is the agent harness. The intended
guarantees, which security reports are especially welcome against:

- Agent CLI sessions are launched with their built-in shell, file-write, and
  web tools **disabled** (Claude Code) or **neutralized by a read-only sandbox
  over an empty scratch directory** (Codex; see
  [docs/agent-harnesses.md](docs/agent-harnesses.md) for why the difference
  exists). The agent's only intended capability is Kinewright's own MCP tool
  surface: timeline operations and read-only inspectors.
- The MCP server binds to `127.0.0.1` on an ephemeral port, for the lifetime of
  the session only.
- Destructive operations require interactive user approval in the chat panel.
- Kinewright never handles provider credentials; authentication lives entirely in
  the user's installed CLI.

A demonstrated escape from any of these — an agent session writing to the real
filesystem, reaching the network through Kinewright, executing shell commands, or
bypassing the confirmation broker — is a vulnerability we want to hear about.

## Supported versions

Kinewright is pre-1.0; only the latest release receives fixes.
