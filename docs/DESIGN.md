# OpenReel design system

## Direction: Cut Room

OpenReel is a professional editing instrument. It should feel dense, quiet, and
confident: a dark cut room where media, timing, and edit state carry the visual
weight. The reference territory is DaVinci Resolve, not consumer creator tools.
Surfaces are nearly flat, borders establish hierarchy, type is compact, and
color is reserved for state.

The timeline is the signature. Video texture and audio shape remain visible at
editing scale, the ruler is always legible, and the cyan playhead cuts through
the stack as the strongest line in the application. Other panels stay visually
subordinate.

Design rules:

- Information density is a feature. Prefer compact rows, labels, and controls.
- Hierarchy comes from tone, border weight, spacing, and type before color.
- `accent` is the only product accent. It marks selection, focus, playhead,
  active transport, and snap state. Never assign decorative colors per clip.
- Status colors are functional signals only. They never identify navigation,
  selection, panels, tracks, or media.
- Motion confirms a state change. It never delays input or decorates idle UI.
- Every visual value in the application must resolve to a token below. When a
  new value is needed, add a token here before using it in code.

## Color tokens

All values are sRGB. Alpha variants use the named color with the stated opacity;
they are not separate colors.

### Neutral surfaces

| Token | Hex | Use |
| --- | --- | --- |
| `canvas` | `#0A0D11` | Application and preview surround |
| `panel` | `#10141A` | Docked panel background |
| `surface` | `#161B22` | Cards, timeline lanes, inputs |
| `surface-raised` | `#1C222B` | Popovers, dialogs, hovered cards |
| `surface-active` | `#222A34` | Pressed controls and active drag bodies |
| `border-subtle` | `#252C36` | Panel dividers and quiet rules |
| `border-strong` | `#3A4553` | Focus-independent control outlines |
| `text-primary` | `#E6ECF2` | Primary copy and high-value numbers |
| `text-secondary` | `#A5AFBB` | Labels and supporting metadata |
| `text-muted` | `#707B88` | Disabled and tertiary information |
| `media-shadow` | `#05070A` | Preview letterbox and thumbnail fallback |

### Product accent

| Token | Hex | Use |
| --- | --- | --- |
| `accent` | `#42C7C9` | The one product accent |

Allowed accent treatments are `accent` at 100% for the playhead and keyboard
focus, 72% for selected borders, 28% for selected fills, 16% for hover fills,
and 10% for subtle focus/snap wash. Do not introduce another accent hue.

### Functional status colors

These colors communicate outcomes only and are not part of the product accent
system.

| Token | Hex | Use |
| --- | --- | --- |
| `status-success` | `#70C391` | Completed export or available service |
| `status-warning` | `#D7B26D` | Confirmation, cost, or degraded state |
| `status-danger` | `#F06C75` | Destructive action and error |

### Marker color token indices

Project files store marker color as a stable token index, never as an RGB
value. M13 keeps markers inside the existing accent and neutral system:

| Index | Token | Use |
| ---: | --- | --- |
| `0` | `accent` | Default editorial marker |
| `1` | `text-primary` | High-emphasis neutral marker |
| `2` | `text-secondary` | Supporting neutral marker |
| `3` | `text-muted` | Low-emphasis neutral marker |

Selection still uses `accent` regardless of the stored marker token. New marker
colors require a design-token change and a corresponding stable index; raw or
decorative per-marker colors are not allowed.

### Title presentation token indices

Title project data stores stable indices rather than raw presentation values.
Font-size index `0` is Small (40 px at 1080 lines), `1` is Standard (64 px),
and `2` is Display (96 px). The media renderer scales these sizes with output
height. Color index `0` resolves to `text-primary`, `1` to `text-secondary`,
and `2` to `accent`. Title scrims use `media-shadow` at 72%. Inter is the only
title family in M14; its embedded bytes are shared with the media renderer.

### Color application

- Window and central canvas: `canvas`.
- Left/right docks: `panel`, separated by a 1 px `border-subtle` rule.
- Inputs and timeline lanes: `surface`.
- Cards and standard buttons: `surface`; hover uses `surface-raised`; pressed
  uses `surface-active`.
- Selected media and clips: `accent` at 28% fill with an `accent` 72% border.
- Video imagery is shown at 78% opacity under a 24% `media-shadow` veil so text,
  trim handles, and waveforms remain legible.
- Audio waveforms use `text-primary` at 64% when idle and `accent` at 72% when
  selected.
- Disabled content uses `text-muted` at 55%.

## Spacing tokens

Logical egui points, based on a compact 4 pt rhythm:

| Token | Value | Use |
| --- | ---: | --- |
| `space-0` | 0 | Flush joins and painter overlays |
| `space-0.5` | 2 | Icon optical adjustment and hairline gaps |
| `space-1` | 4 | Inline gaps and dense internal padding |
| `space-1.5` | 6 | Button padding and compact row gaps |
| `space-2` | 8 | Card padding and standard item spacing |
| `space-3` | 12 | Panel inset and grouped controls |
| `space-4` | 16 | Section separation |
| `space-6` | 24 | Major region separation |
| `space-8` | 32 | Empty-state breathing room only |

Standard panel inset is `space-3`. Standard control height is 26. Compact icon
controls are 26 square; primary transport is 30 square. Timeline header is 32,
ruler is 24, and each track lane is 72.

The default desktop viewport is 1,440 by 900 points. The minimum supported
viewport is 1,100 by 700 points; below that size, the three-column editing
workspace no longer has enough room to remain a professional tool.

## Type tokens

Inter is embedded for the interface. JetBrains Mono is embedded for timecode,
frame counts, tool arguments, paths, and other machine-shaped data. Both are
licensed under the SIL Open Font License 1.1.

egui uses logical points and does not expose CSS-style line-height or tracking;
the sizes below are the source of truth and vertical rhythm comes from the
spacing tokens.

| Token | Family | Size | Treatment | Use |
| --- | --- | ---: | --- | --- |
| `type-title` | Inter | 18 | strong | Dialog title and major empty state |
| `type-heading` | Inter | 14 | strong | Panel and section heading |
| `type-body` | Inter | 12 | regular | Default UI copy |
| `type-button` | Inter | 12 | medium/strong | Button and tab label |
| `type-caption` | Inter | 10 | regular | Metadata and badges |
| `type-micro` | Inter | 9 | strong | Track labels and status lozenges |
| `type-timecode` | JetBrains Mono | 13 | medium | Transport timecode |
| `type-ruler` | JetBrains Mono | 9 | regular | Timeline ruler labels |
| `type-code` | JetBrains Mono | 10 | regular | Tool arguments, paths, and logs |

Use sentence case. Panel headings are not all-caps. Compact machine-state labels
may be uppercase when they are no longer than twelve characters.

## Corner-radius tokens

| Token | Radius | Use |
| --- | ---: | --- |
| `radius-none` | 0 | Panel joins, ruler, timeline lanes |
| `radius-xs` | 2 | Trim handles, badges, progress fills |
| `radius-sm` | 4 | Buttons, inputs, clip blocks |
| `radius-md` | 6 | Cards, menus, chat messages |
| `radius-lg` | 8 | Dialog windows only |

No pill-shaped controls. A badge may use `radius-xs`; it must not become a
capsule unless its height is 4 points or less.

## Border and elevation tokens

The application uses borders instead of broad drop shadows.

| Token | Value | Use |
| --- | --- | --- |
| `border-hairline` | 1 px `border-subtle` | Panel divisions, cards, lanes |
| `border-control` | 1 px `border-strong` | Inputs and idle controls |
| `border-selected` | 1 px `accent` at 72% | Selected card or clip |
| `border-focus` | 2 px `accent` | Keyboard focus and active trim edge |
| `border-danger` | 1 px `status-danger` | Destructive confirmation |
| `elevation-0` | none | Docked panels and lanes |
| `elevation-1` | 0 4 14 `media-shadow` at 48% | Menus and tooltips |
| `elevation-2` | 0 10 30 `media-shadow` at 64% | Modal dialogs only |

## Motion tokens

| Token | Duration | Use |
| --- | ---: | --- |
| `motion-instant` | 0 ms | Press, drag, playhead, trim, snapping |
| `motion-fast` | 80 ms | Hover and selection wash |
| `motion-standard` | 140 ms | Panel disclosure and dialog state |
| `motion-navigation` | 180 ms | Timeline zoom and scroll interpolation |

Motion uses linear interpolation for direct manipulation and egui's standard
easing for disclosure. No animation exceeds 180 ms. Playback, scrubbing, drag,
and snap feedback are immediate. `Context::animate_value_with_time` drives only
the viewport's zoom and scroll targets; media decode never gates animation.

## Icon tokens

Icons use a 16 by 16 SVG viewbox, 1.5 px round strokes, no fill unless the
symbol requires a solid primitive, and `currentColor` semantics represented by
the exported `text-secondary` stroke. Hover and active states recolor the icon
through widget state; selected/playing uses `accent`.

| Token | Value | Use |
| --- | ---: | --- |
| `icon-sm` | 14 | Inline metadata and disclosure |
| `icon-md` | 16 | Standard toolbar and panel action |
| `icon-lg` | 18 | Primary transport action |

The initial set covers play, pause, step backward, step forward, split, delete,
undo, redo, import, add-to-timeline, send, stop, export, folder, filmstrip,
waveform, and panel/chat identity. Icons are original OpenReel assets and ship
under the repository's GPL-3.0-only license; no external icon library is
bundled.

## Component rules

### Application frame

The top bar is 34 points tall. Project actions sit left, a quiet status line
occupies the center, and export/errors sit right. Docks have no floating-card
treatment. The preview is centered in `canvas` with a 1 px `border-subtle`
frame and a `media-shadow` letterbox.

### Timeline

The timeline is the dominant lower work surface. Its toolbar uses icon buttons,
a compact zoom control, and a visible scale readout. The 24 point ruler adapts
major/minor tick intervals to keep labeled ticks at least 72 points apart and
minor ticks at least 8 points apart. Labels are SMPTE-like
`HH:MM:SS:FF` timecode using `type-ruler`.

Each clip has three visual layers:

1. Filmstrip frames tile across the full clip at 78% opacity.
2. A lower waveform band uses 42% of clip height and a `media-shadow` veil.
3. The clip name sits at top-left on a compact surface scrim; source duration
   sits top-right only when space permits.

Hover adds an `accent` 16% wash. Selection uses the selected fill/border tokens.
Active drag uses `surface-active`, a `border-focus` outline, and 92% opacity.
Trim handles are 6 points wide and appear on hover/selection. Snap candidates
are clip edges on every track, project markers, the playhead, and visible ruler
ticks. Holding Alt during a drag disables snapping. A snap uses a 1 px `accent`
guide through the timeline plus a 4 point `accent` diamond at the snapped edge.
The snap tolerance is 8 screen points, independent of zoom.

Project markers are compact ruler flags using their stored marker color token.
Hover reveals the marker label; selection and active drag use `accent`. The
timeline toolbar's ripple-mode control uses the standard selected button fill
and border plus a compact `RIPPLE` accent state label while enabled.

Title clips use an `accent` 16% fill, an `accent` 72% border, a compact Inter
`T` glyph, and the first line of title text. Selection and drag reuse the
standard timeline states. They remain ordinary video-track clips; their visual
treatment distinguishes content without adding a decorative per-clip color.

The playhead is a 2 px `accent` line with a 10 by 8 downward handle in the ruler.
The ruler and handle use immediate pointer tracking. Zoom and horizontal scroll
settle over `motion-navigation` without decoding or file access.

### Media bin

Assets appear as full-width 16:9 cards. A cached thumbnail fills the image area;
`media-shadow` is the fallback. The duration badge is bottom-right in
`surface` at 88% with `type-caption` timecode. Name and media metadata occupy
one compact row below. Selection uses the standard selected tokens. The
add-to-timeline action is an icon control revealed on hover or selection.

### Agent panel

The conversation is visually distinct from configuration. User messages align
right with `accent` at 16%; agent messages align left on `surface`; tool calls
use `surface-raised`, a `border-subtle` left rule, and `type-code` for arguments.
Tool results remain collapsible. Cost and token usage are compact metadata, not
chat bubbles. Confirmations use `status-warning`; rejection/destructive actions
use `status-danger`.

### Inspector panel

The inspector occupies the top of the existing right dock, above the agent
panel. It is collapsible, remembers disclosure through egui panel state, and
caps its expanded height at 360 points so it never claims another timeline
column. Exact frame and second ranges, paths, and raster dimensions use the
monospace data treatment. Controls stay in compact descriptor-driven rows;
the empty state is a single `text-muted` sentence with `space-3` breathing room.

### Transport

Transport is a centered 34 point bar. Step, play/pause, and seek controls are
icons; play/pause is the only `icon-lg` control. The current timecode uses
`type-timecode` and `text-primary`, the total duration uses `text-secondary`.
The scrub rail stays visually quiet because the timeline playhead is primary.

### Transcript and utility panels

Transcript scopes are compact segmented controls. Words use `type-body`; active
words use the standard accent selection treatment. Errors, harness detection,
and advanced settings begin collapsed unless they require action.

### Dialogs and confirmations

Dialogs use `surface-raised`, `radius-lg`, `elevation-2`, and a 1 px
`border-strong` outline. Labels align in a compact grid. The primary action uses
the accent selection treatment. Cancel remains neutral. Destructive actions use
`status-danger` text and `border-danger`; they never use the accent fill.

## Performance contract

- The UI thread performs no media decode, hashing, cache file access, or image
  encoding/decoding from disk.
- Waveform and thumbnail-strip requests are content-addressed and coalesced.
  Workers publish results through bounded channels; the UI only polls and
  uploads ready RGBA bytes to egui textures.
- Waveforms are peak envelopes, not raw samples. Store at most 2,048 min/max
  pairs per asset and downsample again at paint time to at most one vertical
  stroke per screen point.
- Timeline thumbnail strips request a bounded set of periodic frames at a fixed
  small width. Keep at most 128 decoded thumbnails or 64 MiB, whichever comes
  first. Media-bin thumbnails share the same service.
- The worker queue is bounded and duplicate asset/time/size requests coalesce.
  Requests outside the visible timeline are not issued.
- Per-frame painting is proportional to visible clips and visible pixels, never
  source duration. Texture handles and peak vectors are reused between frames.
- Playback and direct manipulation take priority over derived-art requests. If
  thumbnail density threatens the frame budget, reduce tile count before
  reducing interaction fidelity.

## Licensing and distribution

- Inter font files and `Inter-OFL.txt` are embedded in `openreel-app` and must be
  copied into the Windows installer's `LICENSES/` directory.
- JetBrains Mono font files and `JetBrains-Mono-OFL.txt` are embedded in
  `openreel-app` and must be copied into the Windows installer's `LICENSES/`
  directory.
- OpenReel SVG icons are original project assets covered by GPL-3.0-only. The
  installer does not need a separate third-party icon license.
- The installer licensing manifest must name both fonts and preserve their OFL
  texts verbatim.
