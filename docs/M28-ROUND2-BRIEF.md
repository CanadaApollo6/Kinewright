# M28 Round 2 — Reference-Grounded Material Brief

Measured from Zed's source (assets/themes/one/one.json, crates/ui styles),
Radix dark scales (src/dark.ts), and Geist's role bands. Full sources in
the commit that added this file.

## Diagnosis: three measured failures

1. **The floor is too low.** CANVAS #0A0D11 (R=10) sits below Radix
   slate1 (R=17) and 30 RGB points below Zed's darkest surface (R=40).
   Our per-step deltas are Zed-sized but spent in the perceptual dead
   zone near black, where they read as nothing.
2. **Border tokens overlap fill tokens — the #1 flatness bug.**
   BORDER_SUBTLE #252C36 is +3/+2/+2 above SURFACE_ACTIVE #222A34:
   invisible. In Zed and Radix the entire border range sits strictly
   above the entire fill range.
3. **Too saturated.** Our surfaces run S 35–41%; Zed runs 22–24%, Radix
   ~11%. Near black, chroma reads as murk, not blue.

Also: Zed's hierarchy is *inverted* — content wells darkest, chrome
lightest (status bar #3B414D). Elevation in dark mode = lighter surface
first, tight small shadow second; Zed's popover shadow is (0,2,blur 3,
12%) + (0,1,0,6%), nothing like egui's fat default blob. Hover is a +7
RGB whisper (#363C46 over panel); active is +22 and decisive (#454A56).
Accent appears in ~4 places total; selected borders use a *dim* accent
(#293B5B), never full strength.

## The revised ladder (binding)

| Token | Old | New |
|---|---|---|
| CANVAS | #0A0D11 | **#131519** (video letterbox itself may stay #0B0C0E) |
| PANEL | #10141A | **#1A1D23** |
| SURFACE | #161B22 | **#21252C** |
| SURFACE_RAISED | #1C222B | **#272C34** |
| SURFACE_ACTIVE | #222A34 | **#2E343D** |
| BORDER_SUBTLE | #252C36 | **#353C46** |
| BORDER_STRONG | #3A4553 | **#46505C** |
| TEXT_PRIMARY | #E6ECF2 | keep |
| TEXT_SECONDARY | #A5AFBB | **#A8B0BA** |
| TEXT_MUTED | #707B88 | **#78818C** |
| ACCENT | #42C7C9 | keep; add ACCENT_DIM_BORDER **#1E4E50**, ACCENT_WASH = accent @ 14% alpha |

## Five changes, in order of leverage

1. **Re-anchor the ladder** per the table. Non-negotiables: darkest fill
   R≥18; border range strictly above fill range; content wells (viewer,
   timeline tracks) darkest with chrome lighter.
2. **Border deletion program.** Adjacent surfaces differing by ≥1 ladder
   step get NO border — the fill delta is the border. 1px #353C46 only
   where same-fill surfaces meet; #46505C only for structural divisions
   and input outlines.
3. **Desaturate neutrals to S≈24%** (encoded in the new values); chroma
   is reserved for meaningful state.
4. **Elevation grammar, Zed's numbers.** Panels/toolbars: zero shadow.
   Popups/menus: SURFACE_RAISED + Shadow{(0,3), blur 8, 32% black}.
   Modals: SURFACE_ACTIVE + Shadow{(0,6), blur 16, 40% black}. Kill
   egui's default huge soft shadow.
5. **State deltas + accent starvation.** Hover = +1 ladder step,
   neutral. Active/selected = +2 steps. Persistent selection =
   ACCENT_WASH fill + 1px ACCENT_DIM_BORDER. Full accent in exactly
   four places: playhead, focus ring, record/active toggle glyphs,
   links.

Supporting: UI text 14/12/10 at weight 400 (500 max emphasis); mono for
all timecode/durations/frame counts; radii 3–4px controls / 6px popups /
8px modals / 0 docked edges; 22px default control height; spacing steps
{2,4,6,8,12,16,24}.
