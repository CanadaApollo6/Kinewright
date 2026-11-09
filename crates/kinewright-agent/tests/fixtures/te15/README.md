The frozen TE1 still image is presented as a 100-frame lossless video so that
Studio can admit its real duration through the owned-media store. The original
TE1 corpus and hashes remain unchanged. The eval oracle changes only the source
locator and the measured fingerprint; timing, colour, raster and all edits stay
exact. Both arms import these identical bytes.

Generated with the provisioned FFmpeg:

```sh
ffmpeg -v error -loop 1 -framerate 25 -i ../te1/talking-head.ppm \
  -frames:v 100 -an -c:v ffv1 -level 3 -pix_fmt bgra -map_metadata -1 \
  -fflags +bitexact -flags:v +bitexact talking-head-owned.mkv
```

The measurement pin records the file hash. This fixture is unpaid and contains
no provider output, authentication or grant.

The original WAV is carried unchanged as PCM beside a black 16x16, 25-fps
raw BGRA stream in `talking-head-audio-owned.mov`. Audio-only admission otherwise
uses 30 fps, which would change the corpus's source-frame trim contract. The
carrier is on the Audio track and contributes no visible layer. Its admitted
kind/raster/colour are explicit fixture metadata; duration and frame rate stay
100 frames at 25 fps. Checks compare the final render and PCM against the
original Image/WAV oracle, not against a newly regenerated picture or sound.

```sh
ffmpeg -v error -f lavfi -i color=c=black:s=16x16:r=25:d=4 \
  -i ../te1/talking-head.wav -frames:v 100 -map 0:v -map 1:a \
  -c:v rawvideo -pix_fmt bgra -c:a pcm_s16le \
  -af asetnsamples=n=320:p=0 -map_metadata -1 \
  -fflags +bitexact -flags:v +bitexact talking-head-audio-owned.mov
```

BGRA keeps the source RGB bytes lossless and is accepted by the production
owned-frame evidence reader. No production pixel-format guard changes.

The MOV carrier retains the original 8-kHz mono PCM. Its 320-sample audio
packets align to the 25-fps source frames. Production range decoding at
0..40, 40..50, 50..100 and 61..100 must match the frozen WAV. Equal whole-file
PCM is insufficient: the former Matroska carrier seeks and primes the
resampler differently at these boundaries. The oracle always opens the WAV.
