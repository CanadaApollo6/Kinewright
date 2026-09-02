# Color pipeline smoke test

Status: hands-on test procedure for CC0 through CC7, written 2026-09-02 for
Windows and Omarchy. This document uses Simplified Technical English with
American spelling. Text in quotation marks is the exact label in the
application, which uses the spelling "Colour".

## 1. Purpose

This procedure lets a person who is not a colorist confirm that the color
pipeline works on a real desktop. Each test tells you what to do and what you
must see. The test footage has known values, so you do not need to judge
whether a picture looks good. You only compare what you see with the expected
result.

Do the full procedure once on Windows and once on Omarchy. Record the result
of each test in the record sheet in section 15.

## 2. Before you start

1. Build the workspace on the test machine. See `BUILDING.md`.
2. Make sure the build completes without errors.
3. Make the test footage. Run this command from the repository root:

   ```text
   cargo run -p kinewright-agent --bin kinewright-eval -- --write-color-smoke-media <DIRECTORY>
   ```

   Replace `<DIRECTORY>` with an empty folder, for example `~/kinewright-smoke`.
4. Make sure the folder contains these seven files:

   | File | Content | Use in tests |
   | --- | --- | --- |
   | `cc7-camera-a.mkv` | The reference camera. Correct color. 60 frames. | 4, 5, 6, 8, 9, 10, 12, 13 |
   | `cc7-camera-b.mkv` | The same scene, warm and half a stop dark. | 6 |
   | `cc7-camera-c1.mkv` | The same scene, cool and 1.5 stops dark. Recoverable. | 7 |
   | `cc7-camera-c2.mkv` | The same scene, cool and 2.5 stops dark. Not fully recoverable. | 7 |
   | `cc7-log-carrier.mkv` | The same scene in a flat, log-like encoding. | 9 |
   | `cc7-tracked.mkv` | A red square that moves on a gray field. 100 frames. | 11 |
   | `cc7-log-inverse-65.cube` | The LUT that undoes the log-like encoding. | 9 |

5. Prepare one real video clip from a phone or a camera, with sound, at least
   ten seconds long. Tests 13 and 14 use it.
6. Prepare a second monitor or a printout of this document. Some tests ask
   you to compare two pictures.
7. Start the application: `cargo run -p kinewright-app`.

The synthetic footage is small: 320 × 180 pixels at 25 frames per second.
That is correct. Do not scale it.

## 3. How to read a test

Each test has three parts:

- **Steps.** Do them in order. Each step is one action.
- **Expected result.** What you must see after the steps. If you see
  something different, the test fails.
- **Record.** The values you write in the record sheet.

Use these words with these meanings:

- **Click** means press and release the left mouse button once.
- **Set** means move a slider or type a value until the readout shows the
  value given.
- **Select** means click an item so that it becomes the current item.
- **Sample** means click "Sample current frame" in the "SCOPES" section.

If a step tells you to expect a number, the number comes from the CC7
contract (`CC7-WORKFLOW-EVALUATION.md`). A small difference of one or two
units is acceptable unless the step says "exactly".

## 4. Test 1 — Source color metadata (CC0)

### Steps

1. Create a new project with "File" › "New".
2. Import `cc7-camera-a.mkv`.
3. Find the asset in the media bin.
4. Read the line that starts with "SOURCE COLOR".

### Expected result

- The line shows `P:bt709`, `T:bt709`, `M:bt709`, and `R:tv`.
- The line does not contain the word "ASSUMPTION".
- The line does not contain the word "BLOCKED".
- The button "Assume SDR Rec.709 metadata" is not necessary for this asset.
  If it is shown, do not click it.

### Record

The full "SOURCE COLOR" line.

## 5. Test 2 — Scopes on a known picture (CC2)

### Steps

1. Put `cc7-camera-a.mkv` on the video track.
2. Pause playback on frame 10.
3. In the "SCOPES" section, select "Waveform".
4. Sample.
5. Read the resolution label next to the scope.
6. Select "RGB parade". Sample.
7. Select "Vectorscope". Sample.
8. Read the "CLIPPING · ABSOLUTE" group.

### Expected result

- After step 5 the label reads "FULL RESOLUTION" in green. It shows `320×180`.
- After step 4 the waveform shows a straight diagonal line from the bottom
  left to the top right. That line is the gray ramp across the top of the
  picture. Below it, the waveform shows twelve flat steps. Those are the
  twelve gray chart patches.
- After step 6 the three parade channels look the same as each other. The
  picture is neutral.
- After step 7 the vectorscope shows five dots away from the center: green,
  blue, cyan, magenta, and yellow. There is no red dot. That is correct: the
  test picture has no pure red patch.
- After step 8 each of the four rows shows a "black" value and a "white"
  value in basis points. Both values are small but not zero, because the
  ramp and the chart contain a pure black patch and a pure white patch.

### Record

The four "black" and "white" values.

## 6. Test 3 — Match two cameras (CC1, CC2)

The second camera is warm and half a stop dark. You correct it to match the
first camera with three slider values from the contract.

### Steps

1. Put `cc7-camera-b.mkv` on the video track directly after camera A.
2. Move to frame 10, inside camera A.
3. Click "Capture reference shot".
4. Make sure the scopes show "Reference retained".
5. Move to frame 70, inside camera B.
6. Sample.
7. Read the "CURRENT − REFERENCE" group. Write the R, G, B, and Y values down.
8. Select the camera B clip.
9. In the inspector, under "Correction", click "+ Correction" and select
   "Primary correction".
10. Set "Exposure" to `+0.477 stops`.
11. Set "Temperature" to `-45%`.
12. Set "Tint" to `+6%`.
13. Sample.
14. Read the "CURRENT − REFERENCE" group again.
15. Press Ctrl+Z three times.
16. Sample.

### Expected result

- After step 7 the Y value is negative and the R value is positive. Camera B
  is darker and warmer than the reference.
- After step 13 the picture looks the same as camera A. The twelve gray chart
  patches look gray, not orange.
- After step 14 the R, G, B, and Y values are much smaller than in step 7.
  The contract allows a spread of at most 5 code values across the chart
  patches after this correction.
- After step 16 the values are the same as in step 7 again. Undo reversed the
  three slider changes one at a time.

### Record

The R, G, B, and Y values from step 7 and from step 14.

## 7. Test 4 — Recover a wrong white balance (CC1, CC3)

Camera C1 is cool and 1.5 stops dark. The primary controls can recover it.
Camera C2 is cool and 2.5 stops dark. The temperature control reaches its
limit before the picture is neutral. Both results are correct.

### Steps

1. Remove camera B from the timeline.
2. Put `cc7-camera-c1.mkv` after camera A.
3. Select the camera C1 clip. Add a "Primary correction".
4. Set "Exposure" to `+1.432 stops`.
5. Set "Temperature" to `+77%`.
6. Set "Tint" to `-3%`.
7. Look at the chart patches on the program monitor.
8. Remove camera C1. Put `cc7-camera-c2.mkv` after camera A.
9. Select the camera C2 clip. Add a "Primary correction".
10. Set "Exposure" to `+2.410 stops`.
11. Try to set "Temperature" to `+150%`.
12. Read the "Temperature" readout.
13. Set "Tint" to `-30%`.
14. Under "Correction", click "+ Correction" and select "Colour wheels".
15. Drag the "Gain" wheel slowly toward yellow and orange.
16. Stop when the chart patches look gray.
17. Click "Bypass" on the "Colour wheels" card.
18. Click "Bypass" again.
19. Click "Reset" on the "Colour wheels" card.

### Expected result

- After step 7 the chart patches look gray. Camera C1 is recovered.
- After step 12 the readout shows `+100%`. The slider does not go further.
  The chart patches still look slightly blue. That is the contract: camera
  C2 is beyond the authority of the primary control, and the application
  says so instead of hiding it.
- After step 16 the chart patches look gray. The wheel finished what the
  primary control could not.
- After step 17 the patches look blue again. After step 18 they look gray
  again. Bypass is a lossless switch.
- After step 19 the wheel returns to its center and the patches look blue.

### Record

The "Temperature" readout from step 12. Whether steps 17 through 19 worked.

## 8. Test 5 — Curves (CC3)

### Steps

1. Select the camera A clip.
2. Add a "Colour curves" node with "+ Correction".
3. Make sure the "Master" tab is selected.
4. Click the middle of the curve to add a point.
5. Drag the point upward by about one quarter of the editor height.
6. Sample with "Waveform" selected.
7. Right-click the point.
8. Click "Reset curve".

### Expected result

- After step 5 the whole picture is brighter, and the gray ramp is brighter in
  its middle than at its ends.
- After step 6 the diagonal ramp line in the waveform bows upward.
- After step 7 the point is removed and the curve is a straight line again.
- After step 8 the footer shows `2 points`.

### Record

Pass or fail.

## 9. Test 6 — Normalize a log-like clip with a technical LUT (CC4)

### Steps

1. Save the project with Ctrl+S. Give it a name, for example `smoke`.
2. Put `cc7-log-carrier.mkv` on the video track after camera A.
3. Look at the log clip on the program monitor.
4. Select the log clip.
5. In the inspector, under "Input transform", click "+ Technical LUT…".
6. Select `cc7-log-inverse-65.cube` in the file dialog.
7. Look at the log clip again.
8. Read the "Technical LUT" card.
9. Move between the last frame of camera A and the first frame of the log
   clip several times.

### Expected result

- After step 3 the picture looks flat, gray, and low in contrast.
- Before step 1 the "+ Technical LUT…" button is disabled and shows the
  message `project_not_saved`. After step 1 the button is enabled.
- After step 7 the picture has normal contrast. The chart patches look the
  same as on camera A.
- After step 8 the card shows `Stage 1`, a sixteen-character hash, and the
  line "Mix is pinned at full strength: a partially applied technical
  normalization is not a meaningful state (CC4 §5.1)."
- After step 9 you cannot see a difference between the two clips. The
  contract allows a difference of at most 12 code values at the darkest
  patch.
- The project folder now contains a folder named `smoke.kinewright-assets`
  with a `luts` folder inside. The `luts` folder contains one `.cube` file
  whose name is a hash.

### Record

The sixteen-character hash from the card. Pass or fail for the folder.

## 10. Test 7 — Apply a creative look (CC4, CC6)

### Steps

1. Select the camera A clip.
2. Under "Creative look", click "Browse looks…".
3. In the "Looks" window, find the row `warm`.
4. Click "Select".
5. On the "Creative look" card, set the mix slider to `0`.
6. Set the mix slider to `100`.
7. Press and hold the A/B button. Release it.
8. Open "View" › "Colour QC…", or press Ctrl+Shift+C.
9. Click "Measure current frame".
10. Read the "GAMUT" section.

### Expected result

- After step 4 the picture is warmer. The gray patches have an orange tint.
- After step 5 the picture looks the same as without the look. After step 6
  the look is back at full strength.
- During step 7 the picture shows the clip without the look. After you
  release, the look returns.
- After step 10 the "GAMUT" section reports out-of-gamut pixels. The
  contract measures exactly `1480` pixels, which is `256` basis points, on
  camera A with the `warm` look at full strength.

### Record

The out-of-gamut pixel count.

## 11. Test 8 — Secondary correction with a window (CC5)

The correction in this test applies to one small red patch only. The window
values below come from the contract and select that patch exactly.

### Steps

1. Select the camera A clip.
2. Remove the "Creative look" node from test 7.
3. Add a "Primary correction".
4. Set "Saturation" to `+40%`.
5. Open the "Matte (this correction)" section.
6. Click "Enable matte".
7. Click "Add window".
8. Set "Centre X" to `1687`.
9. Set "Centre Y" to `4666`.
10. Set "Half width" to `187`.
11. Set "Half height" to `444`.
12. Set "Feather" to `1000`.
13. On the program monitor, click "Matte view".
14. Look at the picture.
15. Click "Matte view" again.
16. Compare the red patch and the four skin patches to its left with the same
    patches before step 3. Use Ctrl+Z and Ctrl+Y to compare.
17. Click "Select in viewer" on the window row.
18. Drag one handle on the program monitor a small distance.
19. Read the "Centre X" readout.

### Expected result

- After step 14 the picture is black except one white rectangle with a soft
  edge. The rectangle sits on the red patch in the patch row, which is the
  fifth patch from the left in the row under the color primaries.
- After step 16 the red patch is more saturated. The four skin patches are
  unchanged. Nothing outside the window changed.
- After step 19 the readout changed. Dragging on the monitor edits the
  window.

### Record

Pass or fail for step 14 and step 16.

## 12. Test 9 — Secondary correction with a qualifier (CC5)

### Steps

1. On the same "Primary correction" card, click "Remove" on the window row.
2. Click "Qualifier (HSL)".
3. Click "Matte view".
4. Set "Hue centre (cd)" to the hue of the red patch. Start at `36000` and
   decrease the value slowly until the red patch turns white in the matte
   view and the skin patches stay dark.
5. Set "Hue width (cd)" to `1500`.
6. Click "Matte view" again.

### Expected result

- After step 4 the matte view shows white on the red patch only. The skin
  patches are dark or nearly dark. If a skin patch is partly white, the hue
  width is too wide.
- After step 6 the red patch is more saturated and the skin patches are
  unchanged.

### Record

The "Hue centre (cd)" value that isolated the patch.

## 13. Test 10 — Tracked secondary (CC5, CC7)

This test needs an agent CLI. If no agent CLI is installed, record "not
tested" and continue.

### Steps

1. Create a new project. Save it.
2. Put `cc7-tracked.mkv` on the video track.
3. Select the clip. Add a "Primary correction". Set "Saturation" to `+40%`.
4. Enable the matte and add one window with "Centre X" `5000`,
   "Centre Y" `5000`, "Half width" `563`, and "Half height" `1000`.
5. Point at the "Track window…" button. Read the tooltip.
6. In the chat panel, ask the agent: `Track the matte window on this clip
   from local frame 0 to local frame 48 with a step of 5 frames.`
7. Approve the prepared plan when the agent asks.
8. Look at the "Primary correction" card.
9. Turn on "Matte view". Step through frames 0 to 42 with the arrow keys.
10. Step to frames 43 through 47.

### Expected result

- After step 5 the button is disabled. The tooltip explains that tracking
  is agent-driven in CC5 and that the button would pretend to work.
- After step 8 the window controls show the "KEYFRAMED" badge.
- During step 9 the white window follows the red square along its path.
- During step 10 the square is not drawn in the picture. The window stays
  at its last known position. The agent reported one sample, frame 47, as
  low confidence. That is the contract: the tracker does not re-acquire
  after an occlusion.

### Record

The frames the agent reported as low confidence.

## 14. Test 11 — QC and delivery (CC6, AD0)

### Steps

1. Open a project with only `cc7-camera-a.mkv` on the timeline.
2. Press Ctrl+E to open the "Export" dialog.
3. Under "Delivery depth", select "8-bit H.264".
4. Under "Loudness target", select "Measure only".
5. Export to a new file.
6. Wait for the "DELIVERY VERIFICATION" block.
7. Read the status label at the top of the block.
8. Read each budget line.
9. Read the "DECODED AUDIO" lines.
10. Repeat steps 2 through 8 with "10-bit H.264".
11. Read the line that starts with the output path.

### Expected result

- After step 7 the label reads "VERIFIED" in green.
- After step 8 every budget line ends with "within". The "PSNR" line reads
  "within".
- After step 9 the block reads "the file carries no audio stream · nothing
  to measure". The synthetic clip has no sound.
- After step 11 the line contains "10-bit lane" and "decoded yuv420p10le".
- Both exports open and play in a media player outside Kinewright with the
  same colors as the program monitor.

### Record

Both status labels. Whether the two files play outside Kinewright.

## 15. Test 12 — Real footage (CC0 through CC6, AD0)

### Steps

1. Create a new project. Save it.
2. Import your real clip. Read the "SOURCE COLOR" line in the media bin.
3. If the line contains "BLOCKED", click "Assume SDR Rec.709 metadata".
   Read the line again.
4. Put the clip on the timeline. Sample with "Waveform" selected.
5. Add a "Primary correction". Move "Exposure" up and down. Watch the
   waveform while you move it.
6. Open "Colour QC…". Click "Measure current frame". Read "RANGE".
7. Set "Exposure" to `+3.000 stops`. Measure again. Read "RANGE".
8. On the program monitor, click "Clipping" under "QC MASK".
9. Set "Exposure" back to `0.000 stops`.
10. Press Ctrl+E. Select "Loudness target" › "Streaming −14 LUFS".
11. Export. Wait for the verification block.
12. Read the status label and the "DECODED AUDIO" lines.

### Expected result

- After step 3 the "SOURCE COLOR" line no longer contains "BLOCKED". The
  change is undoable with Ctrl+Z.
- After step 5 the waveform moves up when exposure goes up and moves down
  when exposure goes down. The program monitor changes at the same time.
- After step 7 the "RANGE" section shows more clipped pixels than after
  step 6. Bright parts of the picture are clamped to white.
- After step 8 the clamped areas are painted red on the monitor. The legend
  explains the colors.
- After step 12 the block shows the codec, the rate, and the channel count of
  the file's audio. It shows an integrated loudness in LUFS, a true peak in
  dBTP, and a "gain to target" line. If the loudness is more than 1 LU away
  from −14 LUFS, the label reads "AUDIO OUT OF SPEC" and an error line names
  the code `audio_integrated_loudness_below_target` or
  `audio_integrated_loudness_above_target`. That result is correct for an
  unmixed clip. It is a report, not a failure of the export.
- If you have a loudness meter in another program, measure the exported
  file with it. The integrated loudness must agree within 0.5 LU and the
  true peak within 0.3 dB.

### Record

The "SOURCE COLOR" line before and after step 3. The integrated loudness,
the true peak, and the status label from step 12. The reading from an
external meter, if you have one.

## 16. Test 13 — Save, reopen, and move (CC4)

### Steps

1. Open the project from test 6, with the technical LUT.
2. Save it. Close the application.
3. Move the project file and its `.kinewright-assets` folder together to a
   different folder.
4. Start the application. Open the moved project.
5. Select the log clip. Read the "Technical LUT" card.
6. Close the application. Delete the `.cube` file inside the moved
   `luts` folder.
7. Start the application. Open the project. Read the "Technical LUT" card.

### Expected result

- After step 5 the card shows the same hash as in test 6. The picture is
  normalized. The look browser shows the asset as "verified".
- After step 7 the card shows a banner that says the LUT is "Missing" and
  that export and proof are blocked. The card offers "Locate file…" and
  "Replace…". The picture shows the flat log clip.

### Record

Pass or fail for step 5 and step 7.

## 17. Record sheet

Fill in one sheet per machine. Keep the sheet with the release record; the
roadmap requires it for any release that changes native media, GPU, audio,
or packaging behavior.

```text
Machine:                Windows / Omarchy (circle one)
OS build or snapshot:
GPU and driver:
FFmpeg archive SHA-256: (from third_party/ffmpeg/.archive-sha256)
Rust version:           (rustc --version)
Commit:                 (git rev-parse HEAD)
Date:

Test  1 Source color metadata      PASS / FAIL   line: ____________________
Test  2 Scopes                     PASS / FAIL   black/white bp: __________
Test  3 Match two cameras          PASS / FAIL   deltas before/after: _____
Test  4 Wrong white balance        PASS / FAIL   temperature readout: _____
Test  5 Curves                     PASS / FAIL
Test  6 Technical LUT              PASS / FAIL   hash: ____________________
Test  7 Creative look              PASS / FAIL   gamut pixels: ____________
Test  8 Window secondary           PASS / FAIL
Test  9 Qualifier secondary        PASS / FAIL   hue centre: ______________
Test 10 Tracked secondary          PASS / FAIL / NOT TESTED   low-confidence frames: ____
Test 11 QC and delivery            PASS / FAIL   labels: __________________
Test 12 Real footage               PASS / FAIL   LUFS: ______ dBTP: ______
Test 13 Save, reopen, move         PASS / FAIL

Notes (anything that looked wrong, slow, or surprising):
```

## 18. What to do with a failure

1. Write down the test number, the step number, and what you saw.
2. Take a screenshot.
3. Do not try to repair the project. Save a copy of the project folder.
4. Continue with the next test.
5. Give the record sheet and the screenshots to the developer. The contract
   documents for each slice list every exit gate a failure can be compared
   against.
