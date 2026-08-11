# M24 Conversation-First Layout (direction)

Riel's direction (2026-08-11): make the agent conversation the center of
focus, the way T3 Code and Cursor recentered coding from the file buffer to
the session. This document records the design before implementation.

## The principle

T3 Code's essence is not a bigger chat panel — it is that the session is the
spine of the product. User messages, agent narration, and the artifacts of
work flow inline in one column; the composer carries the controls; the
artifact (code) flanks the conversation and opens on demand.

The video translation keeps one non-negotiable difference: an editor must
show picture. The monitor and timeline therefore do not become on-demand
panes — they become the material flanking the conversation.

## Layout

- Center column: the Session. Chat stream as the primary surface — user
  messages, agent narration, and inline operation cards ("Cut 14 silences ·
  -22.4s") with playhead-seeking timecode links and undo affordances.
  Destructive-plan confirmations render as cards in the stream. The composer
  is pinned at the column's bottom, full width, with the harness picker and
  Send in the composer row (no header chrome).
- Right: the program monitor, docked and generous; inspector collapses
  beneath it.
- Bottom: timeline + transcript, full width, resizable and collapsible.
  Manual editing loses no capability; it is visually reframed, not demoted.
- Left: slim media rail (import + assets), collapsible.
- Empty state: the composer front and center — "Import footage and describe
  your edit."

## Sequence

1. Skeleton: rearrange the egui panel geometry (center session column,
   right monitor+inspector, bottom timeline+transcript, left rail).
2. Session stream: operation cards from the existing Core event flow,
   confirmations inline, richer agent narration entries.
3. Composer row: harness picker + send integrated, T3-style.
4. Design pass with screenshots until it earns pride.

Claude-implemented (design-critical surface).
