# PO0: Kinewright's design language

Status: proposal, rev 2.3, 2026-10-28 (probe outcome: §3 settle table, §6 spring bar and outcome, §7 step 0). Nothing here binds until Riel approves the mockup
(`.review/po/mockup-studio-art.html`). On approval, this document replaces the "Cut Room"
direction and the token sections of `docs/DESIGN.md`. The component rules in
`DESIGN.md` (timeline, mixer, inspector and the others) stay. Only their token names
change.

Rev 2 follows Riel's review of rev 1. Riel chose the Apple variant
(`.review/po/mockup-studio-apple.html`, "definitely better") over the Screening Room
variant (`.review/po/mockup-studio.html`), and accepted the lead's hybrid: Apple's feel
without the Mac look.

Rev 2.1 follows Riel's review of rev 2 (Riel loves the direction). It changes three
things in place:
- **Commentary reads like a chat.** The newest message sits at the bottom of the rail
  (§2 Commentary rail).
- **A little more colour.** Each specialist has a hue, the Editor's lanes are coloured
  by type, and the frame's light and the filmstrip carry more colour. All of it still
  goes grey in the neutral surround (§2 Role hues).
- **Annotations.** The person can pin a note to someone in the frame, and the team can
  mark up the frame. This is the one sanctioned exception to "nothing sits on the
  picture" (§2 Annotations).

Rev 2.2 makes one change. Riel likes rev 2.1 "a lot more", and rejected one thing: the
Final Cut yellow on the timeline. The playhead is now a near-white line lit by the
frame, and selection is the person's blue (§2 Accent and status). There is no yellow
token any more.

Note: `DESIGN.md` and `theme.rs` already disagree. For example, `canvas` is `#0A0D11`
in the doc and `#131519` in code, and body type is 12 in the doc and 13 in code.
PO0 replaces both, so the drift ends at rollout step 1.

## The north star

> "Kinewright should almost itself feel like art that helps you create art. It should,
> on its own, be stunning to look at and use." (Riel)

The art principles below are the bar for every surface, in Studio and in the Editor.
A surface that breaks one is not finished.

1. **The picture is the light source.** The window is lit by the current frame. The
   glow, the faint tint on the rail and timeline, and the specular edges that face the
   picture all take their colour from it. When the version changes, the light changes
   with it.
2. **Composed like a framed print.** One grid, generous margins, and nothing crowds the
   picture. Every edge lines up with another edge. The window must look composed as a
   still screenshot.
3. **Typography as craft.** An editorial hierarchy: an eyebrow over the project title,
   a caption plate under the picture, and fine tabular timecode. We use only fonts we
   ship.
4. **Living data, drawn beautifully.** The waveform, the filmstrip and the timeline are
   drawn from real data, and drawn with care. The timeline reads as a landscape, not a
   spreadsheet.
5. **Choreography that explains.** Every motion says what happened. A version switch
   dissolves, and the light follows it. A saved note flies to its marker. Commentary
   arrives with a quiet reveal. Nothing moves only for decoration.
6. **Idle, empty and loading states are composed pieces.** An empty project, a render in
   progress and a paused frame are each designed in their own right.
7. **Real materials.** Very subtle grain on large surfaces, real specular edges, and a
   clear depth order: the picture, then glass, then fills.
8. **The art never competes with the picture.** The picture is always judged neutrally.
   One switch turns off all the light, every role hue and every annotation, and Reduce
   motion covers every motion.

## 1. The direction: Screening Room, Apple's feel without the Mac look

The picture is the only light in the room. Studio is a quiet screening room: the cut
plays large, the agent team's work arrives as a calm running commentary, and you talk
to the room through one composer under the picture. The feel is Apple's pro apps: spring
motion, a soft glow taken from the frame, concentric radii, and a near-white playhead
lit by the picture. The look is not a Mac's. Kinewright ships on Windows and Linux,
so there are no traffic lights, no SF fonts and no macOS chrome. Inter is the face, the
OS owns the window frame, and glass appears only where it reads as a material (the rail
and the composer). The cut room is through the door: one click opens the full editor,
and the timeline rises from the slim strip already on screen.

Rules:

1. **The picture is the light.** Surfaces are neutral greys with no colour cast of their
   own. Every tint comes from the frame. It is off wherever colour is judged: the
   Editor, the neutral-surround switch, colour work and scopes. The neutral surround
   also greys every role hue and the filmstrip (§2 Role hues).
2. **Nothing sits on the picture, with one exception.** No glass, no HUD, no chip, no
   badge, no scrim, no fill and no grain is ever painted over the frame. Transport,
   version and status live on the caption plate under it. The only things inside the
   picture are what the cut itself contains (captions, cards) and annotations: the
   person's pins and drawings, and the team's markup. Annotations follow the exact
   rules in §2 Annotations. They are thin strokes, they show only when invoked or at
   their moment, and they are never exported.
3. **Colour means state, or who.** Blue (open, §2) marks what you chose and what you
   made: focus, the active version, your selection, your notes and your pins. The
   playhead is near-white, lit by the frame, and carries no hue. Status colours mark
   outcomes. Each specialist has
   one pale hue that says who did something: its monogram, its markup, its work in the
   status pill, and in the Editor the lanes it owns. Panels, chrome and body text never
   get a hue.
4. **Calm by default, dense on demand.** Studio uses comfortable density: 32 pt controls
   and generous insets. The Editor keeps today's 26 pt pro density. Both use the same
   tokens.
5. **Motion explains a change of state.** Something moves only because something changed:
   it arrived, it left, it opened, it was replaced or it made progress. Nothing loops,
   pulses or breathes at idle, so there are no repaints at idle.
6. **Your hand is never animated.** Scrub, trim, drag, playhead, typing and pressing
   respond on the next frame. Animation applies to the system's response, never to the
   person's input.
7. **Springs settle, they don't bounce.** One response time for everything. Controls
   carry a 0.6% overshoot, which reads as alive but is under a point of travel. Panels
   are critically damped (§3).
8. **Restraint is part of the art.** Every effect has a reason and a measured cost (§5).
   If a budget fails, the effect goes and the composition must still hold.

## 2. Tokens

All colours are sRGB. Contrast ratios are WCAG 2.x, computed against opaque
backgrounds; translucent text is composited first. Dark mode only. **Light mode is out
of scope for now.** The tokens are semantic, so a light set can be added later without
renaming anything.

### Neutral surfaces

Rev 2 moves from opaque greys to Apple-style translucent white fills over one dark base,
which is what lets the frame's light show through every surface.

| Token | Value | Use |
| --- | --- | --- |
| `ink` | `#000000` | Behind the picture, letterbox |
| `base` | `#0B0B0D` | Window base, Studio stage |
| `well` | `#131316` → `#0F0F11` (top to bottom) | Timeline surface, before the frame tint |
| `glass` | `#18181C` at 52% plus the ambient light (§Translucency) | Rail, composer |
| `fill` | white 6.5% | Chips, cards, inputs, rows at rest, hover |
| `fill-2` | white 10% | Tinted buttons, active chips, play button |
| `fill-3` | white 16% | Pressed |
| `line` | white 7% | Hairlines and dividers |
| `text` | `#F5F5F7` | Primary copy |
| `text-2` | `#EBEBF5` at 64% | Labels, supporting copy, commentary body at 82% |
| `text-3` | `#EBEBF5` at 52% | Metadata, timestamps, eyebrows, ruler |
| `text-off` | `#EBEBF5` at 28% | Disabled. Decorative only, never needed for reading |

Text contrast (composited):

| | `base` | `glass` (≈ `#1C1C20`) | `well` (`#131316`) | `fill-2` on glass |
| --- | ---: | ---: | ---: | ---: |
| `text` | 18.1 | 15.6 | 17.0 | 13.1 |
| `text-2` | 7.1 | 6.6 | 6.9 | 5.9 |
| `text-3` | 5.0 | 4.8 | 5.0 | 4.4 |

`text-3` is AA (4.5:1 or more) everywhere except on `fill-2`. Don't put `text-3` on
`fill-2`. Rev 2 raised it from 44% (3.9:1) to 52% for this reason.

### Accent and status

| Token | Value | Use | On `base` |
| --- | --- | --- | ---: |
| `accent` | `#0A84FF` (blue, **open**) | Focus ring, the send arrow, links, "working" when no specialist owns the work | 5.4 |
| `accent-fill` | `#0071E3`, hover `#0068D1`, press `#005EC0` | Fills that carry white text: primary buttons, the version thumb, the "N new" pill, badges, the person's avatar | white on it 4.70 (hover 5.38) |
| `you` | `#0A84FF` | The person's own marks: note markers, pins, the Point brackets, the active Point chip | 5.4 |
| `select` | `you` (`#0A84FF`): a 2 pt inset outline, a 1 pt dark keyline (`#08080A` at 50%) inside it, and a `you` tint at 14% (story beats) or 16% (clips) | Selected beats and clips | 5.1 on `well`; at least 3.3 on any lane fill |
| `playhead` | `#F5F5F7`, 2 pt line with a 12 × 12 pt shield head; a 0.75 pt black keyline at 85% round both; a 10 pt glow of `lit` at 55% | The playhead | 17.1 on `well` |
| `success` | `#30D158` | Delivered, verified, ready | 9.7 |
| `warning` | `#E7B24F` | Confirm, cost, degraded, muted track | 9.6 |
| `danger` | `#F26B73` | Destructive, error | 6.3 |
| `danger-ink` | `#FF9A9F` | Danger text on a tinted tag | see tags |

Rev 2.1 splits the accent into `accent` for lines and marks and `accent-fill` for fills
under white text. Rev 2's white on `#0A84FF` was 3.65:1, which fails AA for 13 pt
labels. `#0071E3` passes at 4.70:1 with the same hue (h 256 against 255).

**State tags.** A tag is `type-caption` 600 in its status colour on a 14% fill of the
same colour: "Ready" in `success`, "Your call" and "Flagged" in `warning`. Worst case,
on the lightest rail background (`#30343A`): `warning` 4.89, `success` 4.78,
`danger-ink` 4.73. Plain `danger` is only 3.53 there, so danger tags use `danger-ink`.
State colours keep their colour in the neutral surround, because they carry meaning,
not decoration.

**Blue, not iris (decided, §8).** The mockup uses Apple blue. Rev 2.1's role hues change the
question. Iris `#8C96FF` sits at h 278, 27° from the Lead's violet (h 305). Next to a
Lead monogram it would read as "the Lead did this", not "you did this". Blue sits 33°
from the Editor's sky (h 222), but at nearly twice the chroma (0.21 against 0.11) and
much lower lightness (L 0.62 against 0.84). Blue therefore reads as a different kind of
colour, not as a seventh role. `accent-fill` also removes rev 2's white-on-blue
contrast exception. Rev 2.1 recommends blue, and §8 records blue as decided.

**Rev 2.2: no yellow.** Riel rejected the Final Cut yellow on the timeline (revs 2
and 2.1). The yellow `select` token (`#FFD60A`) is gone, and nothing on the timeline is
yellow.

The playhead is the brightest line on the timeline, and the only one without a hue.
- **Lit by the picture.** Its glow is `lit`, so it warms with version C and goes
  neutral with the light in the Editor and the neutral surround (`lit-neutral`). It
  follows over `motion-ambient`, with the rest of the light.
- **Legible on any thumbnail.** On `well` the line is 17.1:1. Over a bright thumbnail
  the white alone can drop to 1.3:1 (a white shirt in the current beat), so the black
  keyline carries the edge there, as the halo does for annotations.
- **Never a marker.** Note markers are 7 pt `you` dots and pinned notes are 10 × 14 pt
  `you` pin glyphs, both above the story lane. The playhead differs in all three ways:
  it is near-white rather than blue (3.4:1 between the two), it is the only line that
  runs the full height of the timeline, and its head is a 12 pt shield, not a dot or a
  ring.

Selection is the person's blue, the same as their focus ring and pins, so "what I
selected" and "what I marked" both read as mine. The dark keyline inside the outline
keeps it separate from the Editor's sky-blue picture lane (3.3:1 against that lane's
fill). Selection is state, so it stays blue in the neutral surround.

### Role hues

Each specialist has one hue. The hues share one OKLCH lightness, L 0.84, so no role is
louder than another. Each role has two values:
- `hue-*`: an ink, for letters, strokes, dots and hairlines;
- `deep-*`: a darker fill, for monogram discs and clip bodies.

| Role | Token | Ink | Deep | OKLCH hue |
| --- | --- | --- | --- | ---: |
| Lead | `hue-lead` | `#D9BBFE` violet | `#9B6BCE` | 305 |
| Editor | `hue-editor` | `#70DBFE` sky | `#0395B7` | 222 |
| Audio | `hue-audio` | `#6FE2CD` teal | `#039C89` | 180 |
| Colour | `hue-colour` | `#FEB98F` apricot | `#C66A28` | 52 |
| Motion | `hue-motion` | `#FEADDF` orchid | `#C15D9F` | 342 |
| Review | `hue-review` | `#BDD77C` pear | `#78931D` | 122 |
| (neutral) | `hue-off` | `#A1A1A6` | `#5A5A5F` | none |

The hues stay clear of the meaning colours: blue (h 255), green (147), amber (80),
danger (19) and the iris alternative (278). They are pale and low in chroma
(C 0.10 to 0.12), so they read as identity, not as alarm.

Where a role hue appears:
- **Monogram.** The letter in `hue-*`, on `deep-*` at 38%, with a 1 pt ring of
  `hue-*` at 46%. When the specialist's message arrives, the ring widens to 3 pt at 42%
  and settles back.
- **Status pill.** The running specialist's hue (`who`) colours the pill's dot, a 9%
  tint of the pill and the rail footer's progress bar. Idle and success go back to
  neutral and `success`.
- **Markup.** The specialist's annotations and the pen glyph on its ref chip
  (§Annotations).
- **Story lane.** The 2 pt changed-beat bar takes the hue of the specialist who
  changed the beat.
- **Editor lanes, by type, as in Final Cut.** Each lane takes the hue of its owner:
  picture clips use Editor, cards and titles use Motion, voice uses Audio. Music uses
  Audio's deep mixed 58% into `#141416`, and its ink mixed 62% into `#46464B`, so the
  bed reads as a quieter cousin of voice. Clip labels are white at 88% on the deep
  fill.

Contrast: a monogram letter is at least 4.6:1 on the lightest rail background
(`#30343A`) and about 6:1 on a typical one. The hue dot is at least 7.2:1 against the
pill. Clip labels are about 7.9:1 on the deep fills.

**Neutral surround.** The switch sets every `hue-*` to `hue-off` and every `deep-*` to
`deep-off` over `motion-ambient`, with the light. It also takes the filmstrip's
saturation to 0 and hides annotations (§Annotations). State colours stay. Colour pages
and scopes force the neutral surround on.

**The Editor** turns the frame's light off but keeps its lane hues. They are dark,
small in area and sit under the viewer, and they are the lane identity that Final Cut
users expect. When the person judges colour in the Editor, the neutral-surround switch
greys them too.

### Frame-derived light

The light is computed from the presented frame, never authored.

| Token | Value | Use |
| --- | --- | --- |
| `light-source` | 32×18 downsample of the presented frame, blurred, scaled 1.5× around the picture | The ambient glow |
| `light-strength` | 92% source opacity at saturation 1.6 under a radial falloff (≈ 18% at the picture edge), 0% in Editor and neutral surround | How much light reaches the room |
| `light-falloff` | Ellipse 1.05 × picture width by 1.2 × picture height, at the picture centre: full to 28%, 40% at 60%, none at 100% | Where the light reaches |
| `lit` | Mean of the downsample's column that faces the rail (right) and its top row, with its OKLCH chroma ×1.6 | Specular edges facing the picture, the rail and timeline tints, the waveform's played colour |
| `lit-warm` | Mean of the warmest third of the downsample | Reserved for the second edge light; the mockup carries it but doesn't paint it yet |
| `lit-neutral` | `#8A8A8E` | Replaces `lit` in Editor and neutral surround |
| `light-rate` | ≤ 4 Hz during playback, at once on a version switch | How often the downsample updates |
| `light-follow` | 420 ms `ease-in-out` (`motion-ambient`) | The glow and `lit` follow a new frame |

Rev 2.1 carries more of the frame's colour: the glow is saturated 1.6× and `lit` keeps
1.6× the measured chroma. The mockup's per-version values: A `#A0BBD5`, B `#AEC6DB`,
C `#C09656`, and the empty state `#8A909D`. Version C is warmer and darker on the rail side because its chart card and amber bar sit in the top
right of the frame. That is the light following the picture, not a theme.

Where the light lands:
- **Glow:** behind the picture, through the falloff. It never paints under text that has
  no surface beneath it, so contrast is computed against the lit base and still passes.
- **Glass:** the rail and the composer show the glow behind them (§Translucency). The
  rail's tint also carries `lit` at 6%.
- **Specular edges:** the 1 pt edge of each surface that faces the picture is
  `lit` at 40–60%: the rail's left edge, the composer's top edge, the timeline's top edge.
- **Timeline tint:** `well` mixed with `lit` at 10% at the top, falling to 3% at 70%.
- **Waveform:** the played part takes `lit` at 22–55% mixed into white.

### Materials

| Token | Value | Use |
| --- | --- | --- |
| `grain` | Tiling greyscale noise (128 px in the app, 180 px in the mockup), 5–6% alpha, zero-mean | Window base, rail, timeline. Stops gradients banding on 8-bit panels. Never on the picture, never on text |
| `spec-top` | 1 pt, white 12% at the lit corner fading to 4.5% | Top edge of glass and raised fills |
| `spec-lit` | 1 pt, `lit` at 40–60%, on the edge that faces the picture, as a gradient border | Rail, composer, timeline |
| `depth-picture` | 0 28 70 black 55%, plus 0 6 18 black 35%, plus a 1 pt white 7% hairline | The picture, top of the depth order |
| `depth-glass` | 0 16 40 black 30%, plus 0 1 3 black 20% | Rail, composer |
| `depth-fill` | 0 10 30 black 28% | Timeline |

Depth order, nearest first: the picture, then glass, then fills. The picture always has
the deepest shadow and the crispest edge.

### Components

**Caption plate.** A 40 pt row under the picture, flush with its left and right edges,
16 pt below it, like a museum label under a print (minimum 680 pt wide in the Editor,
where the picture shrinks). Three columns:
- left: the version picker (A, B, C in a segmented control), then the version name in
  `type-plate` over its status line (dot plus "11:42 · Delivered 4 min ago");
- centre: back 5 s, play (36 pt circle, `fill-2`), forward 5 s, on the picture's axis;
- right: timecode in `type-timecode` over "of 11:42:00", then Draw, Marks (show or hide
  annotations) and the neutral-surround switch.

**Commentary rail.** The team's commentary reads like a chat, newest last:
- The newest message sits at the bottom, just above the rail footer. Older messages
  scroll up. The list starts from the bottom, so a short thread sits low with space
  above it.
- While the person reads at the bottom, the list sticks to the bottom and a new
  message grows in from below (§3 List insert).
- When the person has scrolled up, nothing moves. New messages wait behind an
  "↓ N new" pill at the bottom of the list. Clicking the pill, or scrolling back to the
  bottom, releases them.
- The person's own message (Ask now, a describe in the empty state) always goes to the
  bottom and scrolls there, because the person just wrote it.
- Past the cap (7 in the mockup, a scrollback limit in the app), the oldest message
  leaves from the top.
- In the empty project, the team intro (the six monograms and one sentence) is the head
  of the thread. It sits centred in the rail. As the first messages arrive under it, it
  rises on the same spring as the rows, and then scrolls up with the thread.
- A message may carry a ref chip: a thumbnail plus "B · 03:10". Clicking it seeks
  there. If the message carries markup, the chip shows a pen glyph in the specialist's
  hue, and clicking it also draws the markup (§Annotations).

**Notes pane.** Notes stay newest first, at the top. Notes are not a conversation:
they are a checklist of pending changes that the person sends to the team as a batch,
and the newest one is the one being written. A note pinned to a person shows the pin
on its thumbnail and "· Speaker" after its time.

**Filmstrip story lane.** Each story beat is a 40 pt block filled with thumbnails from the
version's real frames, tiled at the frame's aspect (71 pt per tile at 40 pt), with 1 pt
dark seams between tiles. Punch-in beats tile a tighter crop. Thumbnails sit at 60%
brightness and 96% saturation (rev 2: 56% and 72%). The darkening, not the
desaturation, keeps them from competing with the picture. The beat under the playhead
lifts to 84% brightness and 1.06× saturation, and hover to 92%. In the neutral surround
the saturation goes to 0. The beat name sits top-left over a dark gradient in
`type-label` 600. A 2 pt bar marks beats changed in this version, in the hue of the
specialist who changed it. `select` blue outlines the selection. Note markers are
7 pt `you` dots above the lane. A note pinned to a person shows the pin glyph instead
(§Annotations).

**Waveform.** Mirrored bars, one per ~3 pt, 1.6 pt wide with round caps, about the centre
line. The fill is a vertical gradient that is brightest at the centre. The unplayed part
is `text` at 10–30%; the played part is white mixed with `lit`, at 35–90%. The
envelope comes from the real audio; in the mockup it is generated.

**Ambient light.** See Frame-derived light. In the empty state there is no frame, so a
dim neutral radial glow (`#C8CEDA`, about 15–28% after falloff) stands in: an unlit screen.

**Empty project.** The picture's frame stays, as an unlit screen with a soft radial
light. Inside it: an import glyph, "Every video starts with a take." in `type-display`,
one line of invitation, Import footage (primary) and Describe the video (focuses the
composer). The plate says where versions will appear. The rail shows the team as six
monograms with one sentence. The timeline shows an empty filmstrip well and a flat
waveform line, which is silence.

**Rendering.** The incoming version's frame resolves from low resolution as passes
arrive (in the app, the renderer's latest progressive pass, upscaled with linear
filtering). A light edge in `lit` sweeps across the story lane at the render's
progress, with rendered thumbnails behind it and dimmed lanes ahead. The plate says
"Rendering · 44% · about 6 s left". The sweep moves only when progress moves.

### Annotations

Annotations are the one sanctioned exception to rule 2. An annotation is a thin vector
stroke drawn over the picture, at a moment, by someone. There are two kinds.

**Feedback pinned to a person.** The composer's Point chip (next to Draw) arms Point
mode. As the pointer moves over a person, four `you` corner brackets frame that
person's head. A click attaches a pin to the person, and the moment chip reads
"@ 03:10 on B · Speaker". The saved note keeps the time, the version, who was pinned
and their region. In the app, the pin follows that person through the shot. The mockup
fakes the track with one pose per version and a gentle drift during playback. Escape,
or clicking Point again, cancels. When the playhead reaches the note, the pin appears on
the person. The note's timeline marker is the pin glyph, and its thumbnail in the Notes
list shows the pin. Two notes pinned to the same person in the same window share one
pin.

**The team marks up the frame.** A specialist's message can carry markup tied to a
version and a moment. The mockup has three: the Reviewer's circle round the cropped chin
at 03:10 on B, the Editor's box showing the reframe on C, and Motion's arrow at the
chart's tight-end bar. Clicking the message's ref chip seeks there and draws the markup
in the specialist's hue. The stroke draws itself on, like a pen. It looks hand-drawn
but lands exactly: the circle runs about 30° past its start and drifts 5% inwards, the
box's sides cross by 8 units at the corners, and the arrow is one curve with its head
drawn last. The shapes are generated from the specialist's region, not drawn freehand.
The person's own drawings (Draw) follow the same rules, in `you`.

**The rule, precisely.** An annotation may be painted over the picture only when all
four conditions hold:
1. **Invoked, or at its moment.** A pin shows only while the playhead is inside its
   window (`mark-window`) on its version, or while it is being placed. Markup shows only
   after the person clicks its chip. It leaves when the playhead leaves its window, when
   the version changes, when the person clicks the chip again, or on Escape. Nothing is
   on the picture at rest.
2. **Never in the neutral surround or a colour-critical view, unless the person asks.**
   The neutral surround, colour pages and scopes hide every annotation. The Marks button
   on the plate shows them again, for that view only. Turning the neutral surround on
   turns Marks off again. Marks off hides annotations everywhere. The empty project has
   none.
3. **Never part of the output.** Annotations live in notes and commentary, not in the
   timeline. Render, export, publish and thumbnails never see them. The frame-derived
   light samples the decoded frame, not the painted viewer, so annotations never tint
   the room. The presented-frame identity (JA-1 G8) is the frame's, not the overlay's.
4. **Thin, with no fills and no glass.** Strokes only: no filled shapes, no labels, no
   text, no chips, no glass and no scrim on the picture. The pin's head is a ring, not a
   disc.

**Colour.** The person's marks use `you`. A specialist's marks use `mark-*`: its
`hue-*` mixed 65% with its `deep-*` in OKLab, so the stroke holds on skin and white
shirts and still reads as that specialist's hue. When Marks is on in the neutral
surround, the marks keep their hues. A grey mark would disappear into the picture and
would not say who made it.

| Token | Value | Use |
| --- | --- | --- |
| `mark-stroke` | 2.4 frame units on a 960-unit-wide frame (≈ 2 pt on Studio's 792 pt picture), round caps and joins | Markup |
| `mark-halo` | black 46%, 5.4 units wide, under every stroke | Separates the stroke from any picture |
| `mark-pin` | Anchor ring r 3.3 (stroke 1.8), head ring r 8.6 (stroke 2.2) at +17, −30 from the anchor, joined by a 1.5-unit lead; halo 4 on the lead | The person's pin |
| `mark-bracket` | 18-unit corner arms, stroke 1.8, `you` at 90%, halo 4.4 | Point mode's hover |
| `mark-lead` … `mark-review` | Lead `#C39FED`, Editor `#53C2E5`, Audio `#53C9B5`, Colour `#EB9D6D`, Motion `#E991C8`, Review `#A4BF5E` | Specialist markup |
| `mark-draw` | `cubic-bezier(0.45, 0.05, 0.25, 1)`: circle 580 ms, box 640 ms, arrow shaft 460 ms then head 200 ms from 420 ms | Draw-on |
| `mark-pin-in` | Scale 0.55 to 1 and opacity 0 to 1 on `spring`, about the anchor | A pin appears |
| `mark-fade` | 200 ms `ease-in` | A mark leaves |
| `mark-window` | 0.5 s before the moment to 4 s after it | When an annotation is at its moment |
| `mark-marker` | 10 × 14 pt pin glyph in `you`, lands from 1.8× over 520 ms | A pinned note's timeline marker |

Contrast: WCAG 1.4.11 asks 3:1 for graphics, and the halo gives every stroke a second
edge, so at least one edge clears it on any picture. On a white shirt the halo edge is
3.47:1 against the shirt. On mid skin (`#8A5A40`) the ink is 5.8:1 against its halo.
On a dark background the ink is 7.9:1 against the picture.

Reduce motion: no draw-on and no scale. Marks appear with a 100 ms fade and leave with
a 100 ms fade.

### Type

**Inter** (variable, SIL OFL 1.1) is used for the interface. **JetBrains Mono**
(variable, SIL OFL 1.1) stays for paths, tool arguments and machine data. Both are
bundled with their OFL texts in `crates/kinewright-app/assets/`.

Verified: the bundled `Inter-Variable.ttf` has `opsz` 14–32 and `wght` 100–900, and
`JetBrainsMono-Variable.ttf` has `wght` 100–800. These were read from their `fvar`
tables. Inter Display is the same file at `opsz` 32, so the display cut needs no new
font. The three static Inter TTFs can be dropped at rollout. Verified: the Inter
copyright notice in `assets/licenses/Inter-OFL.txt` declares no Reserved Font Name.

Tracking follows Inter's size-dependent curve (`-0.0223 + 0.185 · e^(−0.1745 · size)` em).

| Token | Family, axes | Size / line | Tracking | Use |
| --- | --- | ---: | ---: | --- |
| `type-display` | Inter opsz 28, wght 640 | 28 / 34 | −2.1% | Empty state, ready card |
| `type-title` | Inter opsz 17, wght 620 | 17 / 22 | −1.3% | Project title |
| `type-plate` | Inter opsz 15, wght 620 | 15 / 20 | −0.9% | Version name on the plate |
| `type-eyebrow` | Inter opsz 14, wght 600, caps, ≤ 32 chars | 10.5 / 13 | +7% | Project kind over the title, rail section labels |
| `type-heading` | Inter opsz 14, wght 600 | 13 / 18 | −0.3% | Panel and section headings, commentary names |
| `type-body` | Inter opsz 14, wght 420 | 13 / 18 | −0.3% | Default copy, commentary |
| `type-label` | Inter opsz 14, wght 500 | 12 / 16 | 0 | Chips, tabs, controls |
| `type-caption` | Inter opsz 14, wght 450 | 11.5 / 15 | +0.3% | Metadata, status lines, timestamps |
| `type-micro` | Inter opsz 14, wght 500 | 10 / 12 | +1% | Ruler labels |
| `type-timecode` | Inter Tabular, wght 500 | 15 / 20 | +1% | Plate timecode; frames in `text-3` |
| `mono-small` | JetBrains Mono wght 450 | 11 / 14 | 0 | Paths, tool arguments |

Use sentence case. The hierarchy is editorial: the eyebrow names the kind of work
("Talking head · Couch set"), the title names the work, the plate names the version,
and the status line says where it stands.

**Inter Tabular.** Numbers that must not jitter (timecode, counters, percentages, the
ruler) use Inter's tabular figures, which read finer than mono. egui exposes no
OpenType features (§5), so the tabular figures are frozen into a second family at build
time: a derived `Inter-Tabular.ttf` with `tnum` (and `case` for colons) remapped into the
default cmap with fontTools' feature freezer, registered as its own egui family. The OFL
allows modified versions; with no Reserved Font Name the derived file may keep a
descriptive name, and it ships with the OFL text. Fallback if the build step is
refused: JetBrains Mono 500 for timecode, as in rev 1.

### Spacing and the grid

Spacing uses a 4 pt grid: `space-0.5` 2, `space-1` 4, `space-1.5` 6, `space-2` 8,
`space-3` 12, `space-4` 16, `space-5` 20, `space-6` 24, `space-8` 32 and `space-10` 40.

Studio is one composition with one margin, `space-6` (24 pt):
- The window has a 24 pt margin on the left, right and bottom.
- Two columns: the stage, and the rail (336 pt) after a 24 pt gutter. The top bar uses
  the same two columns. The mode switch is centred on the stage column, and the status
  cluster ends on the rail's right edge.
- The picture fills the stage column exactly. Its left edge, the plate's edges, the
  timeline's left edge and the project mark share one vertical line. The rail's right
  edge, the timeline's right edge and the Publish button share another.
- The picture's top aligns with the rail's top. The composer's bottom aligns with the
  rail's bottom.
- One vertical axis runs through the mode switch, the picture, the play button and the
  composer.

| | Studio | Editor |
| --- | ---: | ---: |
| Control height | 32 | 26 |
| List row | 40 | 28 |
| Window margin | `space-6` | `space-3` |
| Top bar | 64 | 34 |
| Rail width | 336 | 300 |
| Timeline | 112 | 268 |

The Editor's track, ruler and mixer sizes in `DESIGN.md` are unchanged.

### Corner radii

Radii are concentric: an inner radius is the outer radius minus the inset.

| Token | Radius | Use |
| --- | ---: | --- |
| `r-0` | 0 | Panel joins, ruler, lanes |
| `r-print` | 4 | The picture. A print has nearly square corners, and the frame's corners stay visible for judging |
| `r-xs` | 5 | Filmstrip beats, clip blocks, note thumbnails |
| `r-sm` | 10 | List rows and note cards inside the rail (rail 16 − inset 6) |
| `r-md` | 12 | Timeline surface |
| `r-lg` | 16 | Rail, sheets and dialogs |
| `r-full` | height / 2 | Buttons, chips, segmented controls and thumbs, the composer (26 with 16 pt chips inset 10), avatars |

Rev 2 makes buttons capsules, as in the chosen Apple variant. Window corners belong to
the OS or the compositor. On Riel's Hyprland, `decoration:rounding` is 0 (verified with
`hyprctl`), so Kinewright never paints its own window corners. The mockup paints a
12 pt window only so it reads as a window on a page.

### Translucency

**Glass is used on the rail and the composer only. Never on the picture, and never over
the picture.**

| Token | Value | Use |
| --- | --- | --- |
| `glass-rail` | `#18181C` at 52% mixed with `lit` at 6%, a white 4.5% top sheen over 180 pt, over the ambient light blurred 40 pt, saturation 190% | The rail |
| `glass-composer` | `#1E1E22` at 50%, a white 6% to 2% sheen, over the ambient light blurred 30 pt | The composer |
| `veil-media` | `ink` 24% | Disabled clips, as before |

Rules:
- Glass shows only the room's light behind it: the ambient glow, the base and the grain.
  The composer is laid out so it never overlaps the picture, even fully open:
  its top stays 10 pt below the picture.
- The top bar and the timeline are not glass. The top bar is the lit base itself; the
  timeline is a tinted fill.
- Rev 1's focus scrim over the stage is gone: it was an overlay on the picture. Opening
  the composer instead fades the plate, which the open composer covers.
- OS window glass (Mica, compositor blur) is a separate, later option (§4). It never
  sits behind the viewer, the timeline or a scope. With it off, everything is opaque
  and the design must look finished.

Because only the room's light is behind the glass, the app needs no backdrop blur: it
paints the ambient light again inside the glass shape, more blurred and veiled (§5).

## 3. Motion

### Tokens

| Token | Duration | Curve | Use |
| --- | ---: | --- | --- |
| `motion-instant` | 0 | none | Press, scrub, trim, drag, playhead, snapping, typing |
| `motion-micro` | 90 ms | `ease-out` | Hover in, colour washes |
| `motion-quick` | 160 ms | `ease-in` | Hover out, exits, tab content swap |
| `motion-standard` | 220 ms | `ease-in-out` | Picture dissolve, filmstrip swap, small reveals |
| `motion-ambient` | 420 ms | `ease-in-out` | The light following the picture. Never content |

| Curve | Definition |
| --- | --- |
| `ease-out` | `cubic-bezier(0.16, 1, 0.3, 1)`, for entrances and the label rise |
| `ease-in` | `cubic-bezier(0.4, 0, 1, 1)`, for exits |
| `ease-in-out` | `cubic-bezier(0.4, 0, 0.2, 1)`, for in-place changes and dissolves |

**Springs.** Every spring shares one response, 0.35 s, so everything in the app moves at
the same tempo. Mass 1, stiffness ω₀² where ω₀ = 2π / 0.35 s.

| Spring | Response | Damping ζ | Stiffness | Damping coeff. | 95% | 99% | Overshoot | Settles (0.1%) | Use |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | --- |
| `spring` | 0.35 s | 0.85 | 322.3 | 30.5 | 206 ms | 249 ms | 0.63% at 332 ms | 488 ms | Segmented thumbs, composer size, chips, list rows, the "N new" pill, switches |
| `spring-panel` | 0.35 s | 1.00 | 322.3 | 35.9 | 265 ms | 370 ms | none | 514 ms | Rail open and close, timeline rise, track heads |

Why ζ = 0.85 for controls: the overshoot is 0.63% of the travel, so a 30 pt thumb
overshoots by 0.2 pt and a 160 pt composer by 1 pt. That reads as settling with life,
like Apple's springs, without a visible bounce. Why ζ = 1 for panels: overshoot scales
with travel, and 0.63% of a 336 pt rail is 2 pt, which shows as a wobble at the edge of
the stage. Both are 95% of the way there in 210–265 ms, so nothing the person waits on
feels longer than rev 1's 300 ms rule.

**The derived curve.** Where a spring must be expressed as a curve (CSS in the mockup, a
precomputed table in egui for non-interruptible moves), it is the exact step response,
sampled at 41 points over the settle time:
- ζ < 1: `x(t) = 1 − e^(−ζω₀t) · (cos ω_d t + (ζω₀ / ω_d) · sin ω_d t)`, with
  ω_d = ω₀√(1 − ζ²);
- ζ = 1: `x(t) = 1 − (1 + ω₀t) · e^(−ω₀t)`;
- settle time T: the first t after which |1 − x| stays under 0.1%, which is
  `ln(1000 · √(1 + (ζω₀/ω_d)²)) / (ζω₀)` for ζ < 1, and found numerically for ζ = 1.

The mockup writes `linear(x(0), x(T/40), …, 1)` over T into CSS and runs the
segmented thumbs on a real interruptible spring (semi-implicit Euler, 4 ms substeps).

Rules:

- Springs are interruptible: a new target keeps the current velocity. Curves are used
  for opacity, and springs for geometry.
- Exits are shorter than entrances, at about 70%.
- Settle threshold: within 0.25 pt and under 1 pt/s, snap to the target and stop
  requesting repaints. This absolute threshold, not the 0.1% settle time above, ends a
  live spring, so the repaint window grows with travel. Measured by the PO0 probe
  (`docs/reports/po0/probe.md`) at 60 Hz, from rest:

  | Travel | 32 pt | 64 pt | 160 pt | 336 pt |
  | --- | ---: | ---: | ---: | ---: |
  | `spring` (ζ 0.85) | 333 ms | 517 ms | 567 ms | 600 ms |
  | `spring-panel` (ζ 1) | 483 ms | 533 ms | 583 ms | 633 ms |

  The motion is visually done at the 99% point (249 ms and 370 ms). The time after
  that is sub-point creep. A 520 ms hard snap was measured as an alternative. It leaves
  0.32 pt and 5.1 pt/s, which shows as a one-pixel edge step in a frame-by-frame
  capture. The physics stays as specified. Riel's five-minute look decides whether
  the snap is ever wanted.
- Everything runs on time, not frames: a slow frame skips ahead and never stretches
  the animation.

### Reduce motion

A three-way setting: `Auto` (the default), `On` or `Off`. `Auto` follows the OS.
- Windows: `SPI_GETCLIENTAREAANIMATION`.
- macOS: `accessibilityDisplayShouldReduceMotion`.
- Linux: the `org.gnome.desktop.interface enable-animations` key, read through the
  XDG settings portal.

These OS queries are assumed, not verified. egui, egui-winit and eframe 0.35 expose
no reduce-motion signal (verified by a source search).

With reduce motion on, movement goes away and short dissolves stay:
- Movement means translation, size growth, springs and flight. Its duration drops to 0.
- Opacity changes are capped at 100 ms.
- The picture dissolve becomes a 100 ms dissolve. The light follows at once.
- The saved note doesn't fly: its marker fades in over 100 ms and the Notes badge
  lights.
- The version label swaps with a 100 ms fade and no rise.
- Commentary rows appear in place with an 80 ms fade, with no ring.
- Annotations don't draw on or scale. They fade in and out over 100 ms.
- The render sweep and the resolve still track progress, because they are data, like
  the playhead.

### Signature transitions

| Moment | Spec | Reduce motion |
| --- | --- | --- |
| **Hover** | Fill to `fill` over `motion-micro`. Back over `motion-quick`. | 0 ms colour change |
| **Press** | Instant change to `fill-3`. The background (not the label) insets 1 pt. Release returns over 120 ms. Contents are never scaled. | Same; it is already instant |
| **Panel open/close** | Rail width runs on `spring-panel`. Contents fade in over the last 60% of the open, and fade out over `motion-quick` before the close starts. The picture reflows every frame, the top bar's mode switch rides the stage's centre, and the light moves with the picture. | Instant |
| **Version switch** | Starts only when the new version's frame is actually presented, which is JA-1 G8's displayed identity. The old held still dissolves to the new picture over `motion-standard`. The light (glow, `lit`, edges, waveform tint) follows over `motion-ambient`. The filmstrip dissolves to the new version's thumbnails. The plate's name fades out in 80 ms and the new one rises 3 pt in 240 ms `ease-out`. The picker's thumb runs on `spring`. A second click mid-dissolve restarts from the current blend. Playback never pauses. | 100 ms dissolve, light at once |
| **A note is saved** | A small light leaves the composer's moment chip and flies to the note's time on the story lane in 640 ms: across on `ease-in-out`, up and over on an arc that peaks 46 pt above the higher end. On landing, the marker settles from 2.1× with a ring that fades over 520 ms, and the Notes badge lights `accent-fill` for 900 ms. It says: your note is pinned here. | Marker fades in, badge lights |
| **List insert/remove** | Insert: the row height grows on `spring`, and content fades in over 220 ms after 40 ms, moving 5 pt from the side it enters on: from above in Notes (newest first), from below in the commentary (newest last). Rows ride the height. Remove: fade over 120 ms, then collapse over 160 ms. When the person has scrolled away from the newest item, new rows queue behind an "N new" pill (JA-1 §3). | 80 ms fade, no height animation |
| **Commentary arrives** | A list insert at the bottom, while the list sticks to the bottom so the new row pushes the thread up. The monogram's ring widens once in the specialist's hue, to 3 pt at 42% and back over 320 ms. A single change, not a loop. If the person has scrolled up, the "↓ N new" pill rises 10 pt into place at the bottom on `spring` and nothing else moves. Past the cap, the oldest row leaves from the top. | Fade only, no ring, no rise |
| **The team intro gives way** | In the empty project the intro sits centred in the rail. Each arrival under it shrinks its bottom padding on `spring`, with the row's height, so the intro rises as the thread fills. | Instant |
| **A pin is placed or reached** | The pin scales from 0.55 to 1 about its anchor on `spring` and fades in. It leaves with `mark-fade`. During playback it follows the person's track every frame, like the playhead. | 100 ms fade |
| **Markup draws on** | The stroke draws itself along its length with `mark-draw` (the arrow's head after its shaft), and the ref chip tints in the specialist's hue. It leaves with `mark-fade` when its moment ends or the person dismisses it. | 100 ms fade, no draw-on |
| **Composer expands** | On focus, the composer grows from 560×52 to 720×112 on `spring`, upward into the plate's space, never over the picture. The plate fades out over `motion-quick` and sinks 4 pt. Chips fade in with a 30 ms stagger. It collapses on blur only when empty. Keystrokes land from the first frame. | Instant size, 80 ms fade |
| **Rendering** | The frame resolves pass by pass. The sweep's light edge and the percentage follow progress. On completion, the last pass dissolves to the final frame over `motion-standard`, the status turns `success`, and the state returns to watching. | Same, without the final dissolve |
| **Empty → first import** | The unlit screen fades out as the first frame fades in over `motion-ambient`, and the room's light comes up with it. | 100 ms dissolve |
| **Studio → Editor** | The timeline rises to editor height on `spring-panel`, the track heads slide in, the lanes fade in, and the light fades out to the neutral surround over `motion-ambient`. The viewer keeps its frame and version throughout. | Instant |

## 4. Translucency per OS

In-app glass (the rail and the composer) is painted by Kinewright on every OS and needs
nothing from the OS (§2, §5). This section is only about optional **OS window glass**
behind the window itself.

Verified means read in source or measured on this machine on 2026-10-27. Assumed means
not yet checked.

| OS | Plan | Status |
| --- | --- | --- |
| **Linux** (first platform) | **Opaque by default.** The optional `Window glass: Compositor` setting calls `ViewportBuilder::with_transparent(true)` and lets the base show the desktop at the window edges only. Hyprland or KDE blur it only if the person has blur on. GNOME doesn't blur, so the setting stays off there. | Verified: Riel's Hyprland 0.56.2 has `decoration:blur:enabled = false`, so a transparent window would show a sharp wallpaper. That is why opaque is the default. Verified: egui-wgpu 0.35 asks for a `PreMultiplied` or `PostMultiplied` surface alpha mode when transparency is requested, and only logs a warning if the surface supports neither (`egui-wgpu/src/winit.rs:265–282`). Assumed: NVIDIA's and lavapipe's Vulkan Wayland surfaces offer one of those modes. GNOME behaviour is assumed. |
| **Windows 11** | Mica behind the top bar only, using `window_vibrancy::apply_mica` on the HWND from `eframe::Frame`. `eframe::Frame` implements `HasWindowHandle` (verified, `eframe/src/epi.rs:695`; raw-window-handle 0.6.2, winit 0.30.13 in `Cargo.lock`). Fallback: opaque `base`. | Assumed: the crate's API and version. Assumed: wgpu's swapchain composites with the DWM backdrop. A flip-model HWND swapchain may refuse per-pixel alpha, which would need DX12 composition-visual presentation. This is the riskiest item here, and the probe answers it. |
| **Windows 10** | Opaque. No Acrylic. | Assumed: Acrylic lags while the window resizes on Windows 10. |
| **macOS** (**open**, see §8) | `apply_vibrancy` under the rail, plus `with_fullsize_content_view(true)` and `with_titlebar_shown(false)`, still without traffic-light styling of our own. | Verified: both `ViewportBuilder` methods exist. Everything else is assumed. |

OS glass is the least important light in the room. The frame-derived light does the
job, and it works the same on every OS.

## 5. egui feasibility

Verified items were read in the egui, epaint, emath, egui-wgpu and eframe 0.35.0
sources in `~/.cargo/registry`.

| Area | egui gives us | We write | Ceiling |
| --- | --- | --- | --- |
| Surfaces, borders, radii | `Visuals`, `WidgetVisuals`, alpha `Color32`, per-corner `CornerRadius` (u8) | nothing | none |
| Type | `FontTweak::coords` per registered font, `TextFormat::coords` per text run, `TextFormat::line_height`, `extra_letter_spacing` (all verified) | Families registered at fixed opsz/wght coordinates, plus the frozen `Inter Tabular` family | No OpenType features such as `tnum`, hence the frozen family |
| Elevation | `Shadow { offset, blur, spread, color }` (verified) | Two-layer depth shadows painted by hand | One shadow per call, so two calls for `depth-picture` |
| Specular edges | Meshes with per-vertex colour | A 1 pt ring mesh with a colour gradient along it, tinted by `lit` | none |
| Hover, press | `animate_bool_with_time_and_easing` with an `fn(f32) -> f32` easing (verified, `context.rs:3129`), `WidgetVisuals::expansion` | Asymmetric in and out durations, and bézier easings as plain `fn`s | No per-widget transforms. Press insets the background, never the label |
| Panel slide | `Panel::show_collapsible` with a slide, eased `cubic_out` at the global `Style::animation_time` (verified, `panel.rs:25`) | Width driven by `spring-panel` per panel, interruptible | Whole-window re-layout every frame (see below) |
| Version dissolve | `painter.image` with an alpha tint. The viewer is a registered native wgpu texture with a `CallbackTrait` paint marker (`presenter.rs:282`, `app.rs:3445`) | `HeldFrame`: a GPU copy of the outgoing texture plus a dissolve driven by displayed identity. It holds one extra frame for 220 ms (8.3 MB at 1080p RGBA8) | The PF1 R-5 ack must keep its meaning: the incoming image is acked on its first paint, even at low alpha. The PF1 owners must agree |
| Frame-derived light | Native textures, `TextureOptions::LINEAR`, meshes with per-vertex colour | A 32×18 downsample on the decode worker, a two-pass box blur on that tiny image, `lit` colours from its columns, and a radial-falloff mesh under the stage | Possible banding on very dark frames, which the grain covers |
| Glass | nothing: egui has no backdrop filter | The ambient mesh painted again inside the rail and composer shapes, with a larger blur on the tiny texture, then the glass tint and sheen | Exact only while the glass shows nothing but the room's light, which rule 2 guarantees |
| Grain | `TextureOptions` with `TextureWrapMode::Repeat` (verified, `epaint/src/textures.rs:195`); premultiplied-alpha blending (verified, `egui-wgpu/src/renderer.rs:414`, colour `src_factor: One`) | One 128×128 premultiplied RGBA noise tile (64 KB) whose pixels are white or black at a small alpha, so it averages to no shift. One textured quad per large surface | No soft-light blend. The zero-mean tile stands in for it |
| Filmstrip | Native textures, `painter.image` with UV rects | A thumbnail atlas: frames decoded on a worker at 144×80 (2× of 72×40), cached per version and time, painted as UV sub-rects with 1 pt seams | Decode competes with playback (§6) |
| Waveform | `Shape::line_segment`, meshes | Bars as a single mesh of rounded quads with a vertical colour ramp, clipped at the playhead into two colourings | none |
| Note flight | `Area` with a fixed order, painter on a top layer | A spring-free time curve for x and a two-segment arc for y, then the marker's land animation | none |
| List insert/remove | nothing | `AnimatedList`: per-id height spring, opacity, and ghosts kept for removed rows | Needs stable ids for every row |
| Commentary, chat order | `ScrollArea::stick_to_bottom(true)` (verified, `egui/src/containers/scroll_area.rs:655`) keeps the view at the bottom while it is there | The queue behind the "N new" pill, the bottom-anchored layout for short threads, and the person's own message forcing a scroll | Must not fight the row-height springs: the stick has to follow the growing content every frame of the insert |
| Role hues | `Color32`, per-vertex mesh colour | Hues as tokens, plus a neutral-surround blend that eases every hue to `hue-off` over `motion-ambient` | none |
| Annotations | `Painter` on a layer above the viewer, after the native texture's paint callback; `PathShape` and `Stroke` with feathered tessellation | Strokes as polylines in frame units mapped to the letterboxed image rect; the halo as a wider pass under the ink; draw-on by cutting the polyline at a fraction of its arc length each frame; pins and brackets as small shapes. The matte overlay already paints and hit-tests over the viewer this way (`matte_overlay_ui.rs`, CC5 §6) | `Shape::dashed_line_with_offset` exists (verified, `epaint/src/shapes/shape.rs:189`) but makes segment shapes with butt ends, so draw-on cuts the polyline itself. Strokes are anti-aliased by feathering, not MSAA |
| Pin tracking | nothing | A per-take subject track from the analysis side, interpolated per presented frame. The agent's `track_reframe_subject` already follows an explicitly seeded region by template matching (`kinewright-agent/src/server.rs:14256`); a pin can seed it with the clicked region | No learned person detection exists ("no learned person or face detection", `server.rs:6431`). Point's hover brackets need one, or they fall back to a fixed seed box round the pointer |
| Composer expand | `Area`, `set_transform_layer` with a `TSTransform` (verified, `context.rs:2963`), `Ui::set_opacity` (verified, `ui.rs:560`) | A spring-driven rect. The rect is animated, not scaled | Scaling a layer stretches rasterised text, so we don't |
| Springs | none. `animate_value_with_time` is linear-only (verified, `context.rs:3162`) | `Spring` with semi-implicit Euler substeps on `stable_dt`, state in `Memory`, plus the derived-curve table | none |
| Repaint | `request_repaint` while an animation runs (built in), `request_repaint_after` | Stop requesting at settle | No partial damage: every animated frame repaints the whole viewport |
| Reduce motion | none (verified) | An OS query per platform, plus the setting | Assumed OS APIs |

The motion layer is one module, `crates/kinewright-app/src/motion.rs`. It holds the
tokens, the bézier `fn`s, `Spring`, `AnimatedList`, `HeldFrame`, the reduce-motion
state, and one `animate(ui, id, target, token)` entry point. Code never writes a
duration directly. The light is a second module, `light.rs`: the downsample, the `lit`
colours, the glow and glass meshes, and the grain tile.

### The cost of each art move

| Move | Work per frame or event | Expected cost (assumed until the probe) | Memory |
| --- | --- | --- | --- |
| **Backdrop blur pass** | **Not built.** Glass re-paints the ambient mesh in its own shape. A true blur (dual Kawase, 4 passes at quarter resolution through `CallbackTrait`) is kept only as a measured option for a future surface that scrolls content under glass | Re-paint: two extra small meshes, ≪ 0.1 ms. True blur if ever built: ≈ 0.1–0.3 ms on the RTX 3090 at 1440p, unknown on lavapipe | Re-paint: none. Blur: two quarter-res targets, ≈ 1.9 MB at 1440p |
| **Blurred frame for the glow** | A 32×18 strided sample of the decoded frame on the decode worker at ≤ 4 Hz, a box blur on 576 pixels, an upload of 2.3 KB, and one radial mesh under the stage | ≈ 0.1–0.3 ms per update on the worker, off the UI thread. GPU: one textured mesh across the stage, ≪ 0.1 ms on NVIDIA, ≈ 0.5 ms fill on lavapipe | 2.3 KB per version, kept for A, B, C |
| **Grain** | One textured quad per large surface (base, rail, timeline), every repaint | NVIDIA ≪ 0.1 ms. lavapipe and WARP: a full-window blended pass, ≈ 0.5–1.5 ms at 1440p | 64 KB |
| **Filmstrip thumbnails** | Seek and decode one frame per tile on a low-priority worker, scale to 144×80, upload into an atlas. Studio shows ≈ 16 tiles; the Editor more when zoomed | ≈ 5–30 ms per tile for 1080p H.264 (keyframe seek dominates); 16 tiles in ≈ 0.1–0.5 s, filled progressively. Reuses proxies when they exist. Paused while scrubbing | 46 KB per tile; a 256-tile atlas cache is ≈ 12 MB, evicted per version |
| **Springs** | A few multiply-adds per animated value, negligible. The real cost is that they keep the window repainting: 490–515 ms per move instead of 220–300 ms with curves | Doubles repaint windows per transition; each frame costs what ceiling 1 says | none |
| **Light following** | The glow texture crossfades and `lit` interpolates for 420 ms | Repaints for 420 ms on each version switch; a playback update at ≤ 4 Hz eases over 250 ms, so the window repaints during playback anyway | none |
| **Annotations** | At most a few marks at once, each a polyline of ≤ 100 points, tessellated twice (halo and ink), every frame they are visible or drawing on | ≪ 0.1 ms CPU and GPU on NVIDIA. A draw-on repaints for 460–640 ms. During playback the window repaints anyway | Polylines only: a few KB per mark |
| **Pin tracking** | Template matching over the take's thumbnails, once per pinned note, on the analysis worker. Playback only looks up and interpolates the cached track | Unknown until measured. The existing tracker runs offline on composited thumbnails, so it must yield to playback like the filmstrip decode | One box per sampled frame per pin: about 16 bytes × frames |

The honest ceilings, in order of risk:

1. **Every animated frame is a full UI frame.** egui re-lays out and re-tessellates
   the whole window while anything moves. Springs make each move about twice as long.
   Studio is light, and the Editor with a busy timeline may not fit 120 Hz. The probe
   measures this. The fallback is to cap animation repaints at 60 Hz in the Editor.
2. **Thumbnail decode can hurt playback.** It shares the decoder and the disk with the
   player. It must yield to playback, and the probe checks PF1 G1 while it runs.
3. **Text may step by a pixel during slow vertical moves.** This is assumed: I found
   only a sub-pixel binning option, not sub-pixel vertical text placement. Moves of 6 pt
   or less (the label rise is 3 pt, list rows 5 pt) hide it.
4. **Glass is exact only while nothing but light is behind it.** If a later design puts
   content under glass, the re-paint trick no longer holds and the blur pass is needed.
5. **The physical present time isn't observable.** PF1 says so for acks. Frame pacing
   is measured from update intervals plus wgpu timestamp queries. The app already
   requests `TIMESTAMP_QUERY` where the adapter has it (`app.rs:4406`).

## 6. Probe plan

The probe runs after Riel approves the mockup. It builds these moments in the real app
behind `KINEWRIGHT_MOTION_PROBE=1`, on a real talking-head project, scripted 200 times
each, paused and during 1080p30 playback:

1. a rail slide on `spring-panel`;
2. hover and press across a row of 12 buttons;
3. the A↔B version switch, using the real presenter textures, with the light following;
4. a commentary list insert at 1 per second, with 40 rows present;
5. a saved note flying to its marker;
6. the Studio window with the light, glass re-paint and grain on, against the same
   window with all three off;
7. the filmstrip filling from cold (no cache) for an 11-minute 1080p H.264 project,
   paused and during playback;
8. a render resolve with the sweep, fed by a scripted progressive source;
9. a commentary insert at the bottom with `stick_to_bottom`, while the person is
   reading at the bottom and while scrolled up;
10. a markup draw-on and a tracked pin during 1080p30 playback.

Measure with the existing `KINEWRIGHT_PERF_REPORT` probe (`performance.rs`, which
reports the median, p95 and max today), extended to report:
- the update-to-update interval at p50 and p99;
- CPU frame time (`frame.info().cpu_usage`);
- GPU time from timestamp queries, split into the light pass (glow plus glass
  re-paint), the grain quads and the rest;
- dropped frames, meaning an interval over 1.5× the refresh period;
- process CPU while animating and while idle;
- repaints after settle, and the length of each repaint window per transition;
- the downsample's worker time per update;
- thumbnail decode time per tile (p50, p95), time to fill the visible strip, and PF1 G1
  during the fill;
- memory: the atlas, the held frame and the light textures.

Run it on Linux with NVIDIA (RTX 3090, 60 Hz and 120 Hz) and with lavapipe, and on
the Windows VM (WARP).

| Lane | Pass bar |
| --- | --- |
| NVIDIA, 120 Hz | CPU frame time p50 ≤ 3 ms and p99 ≤ 6 ms. Interval p99 ≤ 1.25× the period. Dropped frames ≤ 0.5% of animated frames, and never two in a row |
| NVIDIA, 60 Hz | CPU p99 ≤ 10 ms. Dropped frames ≤ 0.5% |
| All NVIDIA runs | Animation adds ≤ 10% of one core over the static baseline. Zero motion-layer repaints one frame after settle. Each transition ends within ±1 frame of its token. The dissolve starts ≤ 1 frame after the new version presents. PF1 G1 still holds during the dissolve (≤ 1% late, held or dropped) |
| Light, glass and grain (NVIDIA) | Together ≤ 0.3 ms GPU at 1440p. Downsample ≤ 0.5 ms per update on the worker. No visible banding in the glow on a near-black frame, to Riel's eye |
| Light, glass and grain (lavapipe, WARP) | Together ≤ 2 ms per frame |
| Filmstrip | Visible strip filled ≤ 1 s paused and ≤ 3 s during playback. PF1 G1 unchanged during the fill. Atlas ≤ 16 MB |
| Springs | Same frame bars as above. Repaint window ≤ the settle time for the move's travel (§3 rules table) + 1 frame, and zero repaints after settle. Rev 2.2's flat 520 ms cap was wrong for travel over about 64 pt |
| lavapipe, 60 Hz | A correctness lane. Transitions end on time (they are time-based). Interval p99 ≤ 33 ms. A press is visible on the next frame |
| Windows VM, WARP | Interval p99 ≤ 33 ms. Dropped frames ≤ 5%. It records whether Mica composites with the wgpu swapchain |
| Linux NVIDIA, Wayland | It records whether the surface offers a transparent alpha mode |

**What drops if a budget fails**, in this order. The composition must still hold after
each drop:
1. **The true backdrop blur**, which is already not built.
2. **Grain on software lanes** (lavapipe, WARP) if its pass breaks the 2 ms bar. Banding
   is accepted there.
3. **Live light during playback.** If the downsample or the follow repaints cost too
   much, the light updates only on a version switch and when paused. The glow stays.
4. **Springs in the Editor.** If the Editor misses 120 Hz under springs, it uses the
   `ease-out` curves at 220 ms, which halve the repaint windows. Studio keeps springs.
5. **Filmstrip thumbnails during playback.** If the fill touches PF1 G1, it runs only
   while paused. If it still fails, the story lane falls back to plain beat blocks with
   one thumbnail at the beat's start.
6. **The note flight**, if it ever costs a dropped frame; the marker's landing stays.

Never dropped: the neutral surround switch, Reduce motion, the dissolve on a version
switch, and the rule that nothing sits on the picture.

Riel then looks at the probe for five minutes on the desktop. The probe passes on
the numbers plus Riel's "yes, that's the feel".

What would justify revisiting the toolkit:
- the NVIDIA 120 Hz CPU p99 stays above 8 ms in Studio after two days of profiling;
- the dissolve can't hold the outgoing frame without a second presenter pipeline,
  which JA-1's one-player-per-GUI rule forbids;
- text in motion visibly steps or blurs to Riel's eye, and a snapping strategy can't
  fix it.

These would not justify it: having no backdrop blur, having no CSS-style transitions,
having no soft-light blend, or a Mica failure on Windows (the fallback is opaque).

**Probe outcome (2026-10-28, `docs/reports/po0/probe.md`).** The probe met none of the
toolkit conditions, so egui stays. Two drops are taken:
- **Drop 3.** Light is frozen during playback. It updates on a version switch and while
  paused, and the glow stays. The worker's downsample took up to 1.18 ms per update
  (bar 0.5 ms). Together, the light, glass and grain took up to 0.56 ms on the GPU
  (bar 0.3 ms).
- **Drop 5, in full.** The story lane uses plain beat blocks with one thumbnail at each
  beat's start. A cold strip fill took up to 14 s paused (bar 1 s), and during
  playback 92–95% of frames were late.

Drops 2, 4 and 6 are not triggered. The generic 60 Hz frame and CPU bars failed on both
lanes. Those runs carried load from leaked startup CLI checks, and the effects-off
control failed too, so the failures are not attributed to motion. A clean baseline is
the first step of the rollout (§7). The 120 Hz, WARP and Mica lanes were not run. Riel's
five-minute look is still owed.

## 7. Rollout order, once the probe passes

0. **A clean baseline.** Fix the leaked startup CLI checks first. Then rerun the probe's
   frame, CPU and PF1 G1 bars with effects off and on, on a matched baseline, at 60 Hz
   and, if Riel sets a monitor to it, 120 Hz.
1. **Tokens, type and the motion layer.** Rewrite `theme.rs` and the token sections of
   `DESIGN.md` together: colours, Inter opsz and wght, the `Inter Tabular` build step,
   radii, depth, the near-white playhead and blue selection. Land `motion.rs`. The whole app restyles with
   no layout changes.
2. **The light.** Land `light.rs`: the downsample, `lit`, the glow, the glass re-paint and
   the grain, behind the neutral-surround switch from day one.
3. **Studio** (the JA-1 S3 surfaces): top bar, the picture with its plate, the version
   picker and dissolve, the commentary and notes rail, and the composer. S3 is being
   built now on the current tokens. PO1 restyles it afterwards rather than blocking it.
4. **Living data:** the filmstrip story lane, the waveform, note markers and the flight,
   and the role hues on the rail and the Editor's lanes.
5. **Annotations:** the overlay painter, team markup, then pins once the analysis side
   has a subject track.
6. **States:** the empty project and the render resolve.
7. **The Studio ↔ Editor transition.**
8. **The Editor shell:** top bar, rails, media bin, inspector and agent panel.
9. **Timeline:** clip states, hover washes, the playhead and selection. Direct
   manipulation code stays untouched.
10. **Mixer, colour pages, dialogs, export and settings.**
11. **OS window glass:** Windows Mica first, then the Linux compositor option, then macOS
    if it ships.

## 8. Mockup notes and open questions

- The mockup is `.review/po/mockup-studio-art.html`, built from the Apple variant. Its
  page controls switch between three states (Watching, Rendering C, Empty project) and
  toggle Reduce motion. The plate's half-circle button is the neutral surround, and the
  pin button next to it is Marks.
- To see the annotations: click the Reviewer's "B · 03:10" chip for the circle, the
  Editor's "C · 03:10" chip for the box, or Motion's "C · 02:52" chip for the arrow. For
  a pin, click Point in the composer, hover the speaker and click.
- The mockup fakes the subject track: one pose per version, plus a small sinusoidal
  drift during playback so the pin visibly follows.
- Point and Draw are hidden in the empty project, because there is no picture to mark.
- The mockup uses Inter from `crates/kinewright-app/assets/fonts/Inter-Variable.ttf`,
  read from disk, with `font-optical-sizing: auto`. Timecode uses Inter's `tnum` in CSS,
  which the app gets from the frozen family. It loads no network resource.
- The springs are real: the segmented thumbs run the spring itself, and everything else
  uses the exact derived curve as CSS `linear()`.
- The mockup uses `backdrop-filter` for the rail and composer. Behind them is only the
  glow, so it shows the same thing as the app's re-paint (§5). The glow is the blurred
  frame image in CSS, where the app uses a mesh over a tiny texture. The render resolve
  uses a CSS blur, where the app shows real low-resolution passes. The grain is an
  SVG noise tile with `soft-light`, where the app uses a zero-mean tile.
- The per-version `lit` colours are hand-transcribed from a 4×3 downsample of each frame
  (`magick -resize 4x3!`), including version C's chart card.
- Cut from the hybrid: the HUD capsule, the version chip and the delivery badge on the
  picture (all moved to the plate), and rev 1's focus scrim (an overlay on the picture).

**Decisions (2026-10-27):**
1. **The accent is blue.** Riel approved rev 2.2, which uses blue ("everything else
   looks good"). Iris sits too close to the Lead's violet (§2).
2. **The composer layout is approved.** The composer sits centred under the picture and
   binds to the paused moment. Commentary and notes live in the right rail, with
   commentary in chat order: newest at the bottom. Riel approved it with rev 2.2. The
   JA-1 S3 plan's C2 layout changes to match (see `.review/ja/ja1-s3-addendum-po0-layout.md`).
3. **Point gets a person detector.** Riel chose the detector, so the hover brackets
   find people before the click. This is new analysis work: a small, local, Rust-hosted
   model, with no bundled Python or Node (see the efficiency principle). It is scoped as
   its own item after S3. Until it lands, Point seeds a fixed box round the pointer.
4. **macOS is undecided, leaning no.** Riel can't say whether a macOS build ships
   within a year, and leans towards no. PO0 stays OS-neutral, nothing depends on
   macOS, and the macOS row in §4 stays unbuilt until that changes.
