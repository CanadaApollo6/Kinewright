# PO0 per-run load

Every accepted primary and deadline-variant chunk is listed below, followed by superseded/interrupted original-matrix rows and separate final captures. No load threshold selected or excluded a run. A flagged other process is a sampled observation, not causal attribution. GPU cells show physical NVIDIA utilisation/clocks even on lavapipe. CPU percentages use one logical core as 100%. Empty report values receive no completion credit. Full timestamps, top-five snapshots and five-second flags remain in each source JSON. Final cleanup identified 51 earlier own app-startup version/auth-status checks that had outlived their GUIs; some recorded ambient load was therefore probe-originated. Their per-window contribution was not isolated. Only those identity-verified owned checks were stopped after all timing and captures; see `target/po0-probe/owned-cli-cleanup.json`. No rows were discarded and no post-cleanup timing rerun was made.

## Accepted nvidia-60

| Run | Source | Reps | Exit / errors | Load 1m start → end | Other CPU peak (one core %) | Own CPU static → active (%) | GPU start → end |
| --- | --- | ---: | --- | --- | --- | --- | --- |
| m01-paused-on-p0 | `c2cf9ab5` | 40 | 0 | 81.89 → 77.45 | python PID 3145844: 428.2 | 1.00 → 63.92 | P3, 1035 MHz, 5001 MHz, 13 %, 1999 MiB → P2, 1800 MHz, 9501 MHz, 13 %, 7029 MiB |
| m01-paused-on-p1 | `c2cf9ab5` | 40 | 0 | 77.45 → 76.95 | rustc PID 3317432: 275.8 | 1.00 → 66.55 | P2, 1800 MHz, 9501 MHz, 8 %, 7048 MiB → P2, 1935 MHz, 9501 MHz, 47 %, 13392 MiB |
| m01-paused-on-p2 | `c2cf9ab5` | 40 | 0 | 76.95 → 77.63 | rustc PID 3541467: 135.8 | 1.00 → 67.30 | P2, 1935 MHz, 9501 MHz, 52 %, 13392 MiB → P2, 1935 MHz, 9501 MHz, 81 %, 13674 MiB |
| m01-paused-on-p3 | `c2cf9ab5` | 40 | 0 | 77.63 → 80.35 | rustc PID 3793107: 257.9 | 1.34 → 65.50 | P2, 1935 MHz, 9501 MHz, 79 %, 13657 MiB → P2, 1935 MHz, 9501 MHz, 96 %, 13502 MiB |
| m01-paused-on-p4 | `c2cf9ab5` | 40 | 0 | 80.35 → 75.00 | rustc PID 4093392: 215.7 | 1.00 → 65.21 | P2, 1935 MHz, 9501 MHz, 92 %, 13503 MiB → P2, 1935 MHz, 9501 MHz, 55 %, 13578 MiB |
| m01-play-on-p0 | `c2cf9ab5` | 40 | 0 | 75.00 → 80.39 | rustc PID 4162418: 243.0 | 118.42 → 121.65 | P2, 1935 MHz, 9501 MHz, 55 %, 13578 MiB → P2, 1935 MHz, 9501 MHz, 81 %, 13586 MiB |
| m01-play-on-p1 | `c2cf9ab5` | 40 | 0 | 82.84 → 79.18 | python PID 3145844: 115.2 | 118.93 → 121.92 | P2, 1935 MHz, 9501 MHz, 66 %, 13586 MiB → P2, 1935 MHz, 9501 MHz, 72 %, 13657 MiB |
| m01-play-on-p2 | `c2cf9ab5` | 40 | 0 | 79.18 → 84.08 | python PID 3145844: 116.3 | 120.27 → 121.66 | P2, 1935 MHz, 9501 MHz, 95 %, 13657 MiB → P2, 1935 MHz, 9501 MHz, 77 %, 13586 MiB |
| m01-play-on-p3 | `c2cf9ab5` | 40 | 0 | 84.08 → 86.15 | rustc PID 955458: 196.8 | 124.02 → 122.54 | P2, 1935 MHz, 9501 MHz, 77 %, 13586 MiB → P2, 1935 MHz, 9501 MHz, 81 %, 13627 MiB |
| m01-play-on-p4 | `c2cf9ab5` | 40 | 0 | 86.15 → 83.42 | python PID 3145844: 116.0 | 122.42 → 129.44 | P2, 1935 MHz, 9501 MHz, 70 %, 13627 MiB → P2, 1935 MHz, 9501 MHz, 97 %, 13586 MiB |
| m02-paused-on-p0 | `c2cf9ab5` | 40 | 0 | 83.42 → 78.14 | rustc PID 1626550: 282.1 | 1.00 → 55.48 | P2, 1935 MHz, 9501 MHz, 99 %, 13586 MiB → P2, 1935 MHz, 9501 MHz, 78 %, 13647 MiB |
| m02-paused-on-p1 | `c2cf9ab5` | 40 | 0 | 78.14 → 74.98 | python PID 3145844: 116.8 | 1.67 → 58.26 | P2, 1935 MHz, 9501 MHz, 76 %, 13647 MiB → P2, 1935 MHz, 9501 MHz, 93 %, 13667 MiB |
| m02-paused-on-p2 | `c2cf9ab5` | 40 | 0 | 74.98 → 77.28 | rustc PID 38983: 223.2 | 1.00 → 54.61 | P2, 1935 MHz, 9501 MHz, 81 %, 13667 MiB → P2, 1935 MHz, 9501 MHz, 34 %, 13667 MiB |
| m02-paused-on-p3 | `c2cf9ab5` | 40 | 0 | 77.28 → 74.75 | rustc PID 1214733: 215.0 | 1.00 → 55.66 | P2, 1935 MHz, 9501 MHz, 82 %, 13667 MiB → P2, 1935 MHz, 9501 MHz, 64 %, 13727 MiB |
| m02-paused-on-p4 | `c2cf9ab5` | 40 | 0 | 73.97 → 71.66 | rustc PID 2096355: 272.0 | 1.67 → 55.19 | P2, 1935 MHz, 9501 MHz, 69 %, 13727 MiB → P2, 1935 MHz, 9501 MHz, 99 %, 13727 MiB |
| m02-play-on-p0 | `c2cf9ab5` | 40 | 0 | 71.66 → 83.35 | rustc PID 3281877: 260.6 | 127.27 → 124.15 | P2, 1935 MHz, 9501 MHz, 99 %, 13727 MiB → P2, 1935 MHz, 9501 MHz, 44 %, 13736 MiB |
| m02-play-on-p1 | `c2cf9ab5` | 40 | 0 | 83.35 → 79.65 | rustc PID 3568373: 279.6 | 126.67 → 125.08 | P2, 1935 MHz, 9501 MHz, 44 %, 13736 MiB → P2, 1950 MHz, 9501 MHz, 99 %, 13948 MiB |
| m02-play-on-p2 | `c2cf9ab5` | 40 | 0 | 79.65 → 92.10 | python PID 3145844: 115.3 | 177.86 → 126.65 | P2, 1950 MHz, 9501 MHz, 86 %, 13948 MiB → P2, 1950 MHz, 9501 MHz, 87 %, 13797 MiB |
| m02-play-on-p3 | `c2cf9ab5` | 40 | 0; audio output stream failed | 92.10 → 81.77 | ffmpeg PID 2200626: 452.0 | 169.11 → 75.26 | P2, 1950 MHz, 9501 MHz, 100 %, 13797 MiB → P2, 1950 MHz, 9501 MHz, 100 %, 14022 MiB |
| m02-play-on-p4 | `c2cf9ab5` | 40 | 0 | 81.77 → 77.93 | ffmpeg PID 2371585: 392.0 | 115.80 → 121.96 | P2, 1950 MHz, 9501 MHz, 45 %, 14022 MiB → P2, 1950 MHz, 9501 MHz, 83 %, 13730 MiB |
| m04-paused-on-p0 | `c2cf9ab5` | 40 | 0 | 88.40 → 84.49 | rustc PID 2055256: 143.9 | 1.67 → 41.31 | P2, 1950 MHz, 9501 MHz, 100 %, 13991 MiB → P2, 1950 MHz, 9501 MHz, 57 %, 14049 MiB |
| m04-paused-on-p1 | `c2cf9ab5` | 40 | 0 | 84.49 → 80.54 | au3_fixtures-66 PID 2189060: 119.5 | 1.00 → 41.26 | P2, 1950 MHz, 9501 MHz, 45 %, 14267 MiB → P2, 1950 MHz, 9501 MHz, 85 %, 14194 MiB |
| m04-paused-on-p2 | `c2cf9ab5` | 40 | 0 | 80.54 → 74.05 | rustc PID 2573941: 121.1 | 1.33 → 41.65 | P2, 1950 MHz, 9501 MHz, 69 %, 14194 MiB → P2, 1950 MHz, 9501 MHz, 95 %, 13968 MiB |
| m04-paused-on-p3 | `c2cf9ab5` | 40 | 0 | 74.05 → 76.58 | rustc PID 2723349: 272.4 | 1.33 → 41.26 | P2, 1950 MHz, 9501 MHz, 77 %, 13968 MiB → P2, 1950 MHz, 9501 MHz, 35 %, 13961 MiB |
| m04-paused-on-p4 | `c2cf9ab5` | 40 | 0 | 76.58 → 78.18 | python PID 3145844: 114.4 | 1.00 → 39.53 | P2, 1950 MHz, 9501 MHz, 35 %, 14190 MiB → P2, 1950 MHz, 9501 MHz, 91 %, 13973 MiB |
| m04-play-on-p0 | `c2cf9ab5` | 40 | 0 | 78.18 → 71.26 | python PID 3145844: 114.3 | 130.59 → 123.75 | P2, 1950 MHz, 9501 MHz, 89 %, 13973 MiB → P2, 1950 MHz, 9501 MHz, 98 %, 14224 MiB |
| m04-play-on-p1 | `c2cf9ab5` | 40 | 0 | 71.26 → 73.23 | rustc PID 4038140: 134.1 | 124.11 → 126.36 | P2, 1950 MHz, 9501 MHz, 56 %, 14224 MiB → P2, 1950 MHz, 9501 MHz, 70 %, 13968 MiB |
| m04-play-on-p2 | `c2cf9ab5` | 40 | 0 | 73.23 → 73.28 | rustc PID 4096947: 234.8 | 178.91 → 137.89 | P2, 1950 MHz, 9501 MHz, 89 %, 13968 MiB → P2, 1950 MHz, 9501 MHz, 87 %, 13973 MiB |
| m04-play-on-p3 | `c2cf9ab5` | 40 | 0 | 73.28 → 76.31 | rustc PID 380552: 227.8 | 123.25 → 124.63 | P2, 1950 MHz, 9501 MHz, 87 %, 13973 MiB → P2, 1800 MHz, 9501 MHz, 16 %, 7278 MiB |
| m04-play-on-p4 | `c2cf9ab5` | 40 | 0 | 76.31 → 82.33 | python PID 678531: 436.8 | 125.39 → 122.94 | P2, 1800 MHz, 9501 MHz, 10 %, 7278 MiB → P2, 1800 MHz, 9501 MHz, 15 %, 7045 MiB |
| m05-paused-on-p0 | `c2cf9ab5` | 40 | 0 | 82.33 → 78.01 | rustc PID 955361: 229.5 | 1.00 → 73.21 | P2, 1800 MHz, 9501 MHz, 10 %, 7045 MiB → P3, 1455 MHz, 5001 MHz, 56 %, 1955 MiB |
| m05-paused-on-p1 | `c2cf9ab5` | 40 | 0 | 78.01 → 82.40 | rustc PID 1578574: 262.4 | 1.33 → 72.46 | P3, 1455 MHz, 5001 MHz, 13 %, 1955 MiB → P3, 540 MHz, 5001 MHz, 31 %, 1999 MiB |
| m05-paused-on-p2 | `c2cf9ab5` | 40 | 0 | 82.40 → 75.75 | rustc PID 2199174: 254.5 | 1.00 → 71.48 | P3, 495 MHz, 5001 MHz, 22 %, 1955 MiB → P3, 1260 MHz, 5001 MHz, 13 %, 1883 MiB |
| m05-paused-on-p3 | `c2cf9ab5` | 40 | 0 | 75.75 → 70.90 | rustc PID 2512666: 255.7 | 1.34 → 71.02 | P3, 1260 MHz, 5001 MHz, 13 %, 1883 MiB → P3, 660 MHz, 5001 MHz, 24 %, 1889 MiB |
| m05-paused-on-p4 | `c2cf9ab5` | 40 | 0 | 70.90 → 71.99 | rustc PID 2817602: 268.7 | 1.33 → 70.14 | P3, 585 MHz, 5001 MHz, 16 %, 1889 MiB → P3, 1200 MHz, 5001 MHz, 13 %, 1831 MiB |
| m05-play-on-p0 | `c2cf9ab5` | 40 | 0 | 71.99 → 74.44 | rustc PID 3758643: 234.8 | 123.36 → 123.61 | P3, 615 MHz, 5001 MHz, 13 %, 1831 MiB → P3, 1170 MHz, 5001 MHz, 40 %, 1881 MiB |
| m05-play-on-p1 | `c2cf9ab5` | 40 | 0 | 74.44 → 76.45 | rustc PID 4067939: 258.1 | 115.62 → 124.02 | P3, 1170 MHz, 5001 MHz, 12 %, 1879 MiB → P3, 1170 MHz, 5001 MHz, 14 %, 1870 MiB |
| m05-play-on-p2 | `c2cf9ab5` | 40 | 0; audio output stream failed | 76.45 → 75.44 | rustc PID 500458: 255.5 | 123.58 → 94.55 | P5, 810 MHz, 810 MHz, 51 %, 1870 MiB → P3, 975 MHz, 810 MHz, 21 %, 1834 MiB |
| m05-play-on-p3 | `c2cf9ab5` | 40 | 0 | 75.44 → 81.48 | python PID 1131814: 244.5 | 125.38 → 123.12 | P3, 1170 MHz, 5001 MHz, 49 %, 1891 MiB → P3, 1095 MHz, 5001 MHz, 22 %, 1838 MiB |
| m05-play-on-p4 | `c2cf9ab5` | 40 | 0 | 81.48 → 80.94 | rustc PID 1428283: 252.7 | 161.70 → 129.65 | P3, 585 MHz, 5001 MHz, 12 %, 1838 MiB → P3, 390 MHz, 5001 MHz, 33 %, 1880 MiB |
| m06-paused-off-p0 | `c2cf9ab5` | 40 | 0 | 80.94 → 81.35 | python PID 1131814: 221.0 | 1.33 → 76.63 | P3, 600 MHz, 810 MHz, 27 %, 1880 MiB → P3, 1170 MHz, 5001 MHz, 21 %, 1874 MiB |
| m06-paused-on-p0 | `c2cf9ab5` | 40 | 0 | 81.35 → 79.62 | rustc PID 1737040: 240.8 | 1.00 → 66.00 | P3, 1170 MHz, 5001 MHz, 14 %, 1874 MiB → P3, 450 MHz, 5001 MHz, 36 %, 1869 MiB |
| m06-paused-off-p1 | `c2cf9ab5` | 40 | 0 | 79.62 → 80.27 | python PID 1131814: 227.9 | 1.34 → 75.13 | P3, 405 MHz, 5001 MHz, 27 %, 1869 MiB → P3, 1275 MHz, 5001 MHz, 24 %, 1863 MiB |
| m06-paused-on-p1 | `c2cf9ab5` | 40 | 0 | 80.27 → 79.49 | python PID 1131814: 228.4 | 0.67 → 73.43 | P5, 1035 MHz, 5001 MHz, 49 %, 1863 MiB → P3, 420 MHz, 5001 MHz, 28 %, 1865 MiB |
| m06-paused-off-p2 | `c2cf9ab5` | 40 | 0 | 79.49 → 78.94 | rustc PID 2038307: 254.4 | 1.00 → 75.01 | P3, 405 MHz, 5001 MHz, 28 %, 1865 MiB → P3, 1335 MHz, 5001 MHz, 14 %, 1864 MiB |
| m06-paused-on-p2 | `c2cf9ab5` | 40 | 0 | 78.94 → 80.60 | python PID 1131814: 207.0 | 1.00 → 74.50 | P3, 1335 MHz, 5001 MHz, 14 %, 1864 MiB → P3, 450 MHz, 5001 MHz, 35 %, 1866 MiB |
| m06-paused-off-p3 | `c2cf9ab5` | 40 | 0 | 80.60 → 80.64 | python PID 1131814: 228.0 | 1.00 → 71.74 | P3, 420 MHz, 5001 MHz, 27 %, 1866 MiB → P3, 1320 MHz, 5001 MHz, 16 %, 1864 MiB |
| m06-paused-on-p3 | `c2cf9ab5` | 40 | 0 | 80.64 → 82.69 | python PID 1131814: 228.7 | 1.00 → 74.22 | P3, 1320 MHz, 5001 MHz, 14 %, 1864 MiB → P3, 405 MHz, 5001 MHz, 33 %, 1873 MiB |
| m06-paused-off-p4 | `c2cf9ab5` | 40 | 0 | 82.69 → 84.48 | python PID 1131814: 211.9 | 0.67 → 72.00 | P3, 375 MHz, 5001 MHz, 26 %, 1872 MiB → P3, 1305 MHz, 5001 MHz, 15 %, 1869 MiB |
| m06-paused-on-p4 | `c2cf9ab5` | 40 | 0 | 84.48 → 85.14 | python PID 1131814: 231.0 | 1.00 → 74.23 | P3, 1305 MHz, 5001 MHz, 14 %, 1869 MiB → P3, 465 MHz, 5001 MHz, 30 %, 1862 MiB |
| m06-play-off-p0 | `c2cf9ab5` | 40 | 0 | 85.14 → 83.93 | rustc PID 2731119: 241.5 | 121.57 → 122.78 | P3, 435 MHz, 5001 MHz, 24 %, 1862 MiB → P3, 435 MHz, 5001 MHz, 31 %, 1866 MiB |
| m06-play-on-p0 | `c2cf9ab5` | 40 | 0 | 83.93 → 92.09 | python PID 1131814: 230.0 | 164.39 → 155.75 | P3, 420 MHz, 5001 MHz, 28 %, 1866 MiB → P3, 1305 MHz, 5001 MHz, 14 %, 1867 MiB |
| m06-play-off-p1 | `c2cf9ab5` | 40 | 0 | 92.09 → 91.64 | python PID 1131814: 226.3 | 126.81 → 119.41 | P3, 1305 MHz, 5001 MHz, 14 %, 1867 MiB → P3, 480 MHz, 5001 MHz, 30 %, 1855 MiB |
| m06-play-on-p1 | `c2cf9ab5` | 40 | 0 | 91.64 → 91.07 | python PID 1131814: 227.8 | 122.62 → 127.77 | P3, 450 MHz, 5001 MHz, 25 %, 1855 MiB → P3, 510 MHz, 5001 MHz, 36 %, 1846 MiB |
| m06-play-off-p2 | `c2cf9ab5` | 40 | 0 | 91.07 → 95.47 | python PID 1131814: 230.1 | 125.04 → 122.28 | P3, 480 MHz, 5001 MHz, 24 %, 1846 MiB → P3, 1005 MHz, 5001 MHz, 21 %, 1859 MiB |
| m06-play-on-p2 | `c2cf9ab5` | 40 | 0 | 95.47 → 95.31 | rustc PID 3380239: 127.9 | 121.27 → 126.71 | P3, 1005 MHz, 5001 MHz, 13 %, 1859 MiB → P3, 450 MHz, 5001 MHz, 21 %, 1857 MiB |
| m06-play-off-p3 | `c2cf9ab5` | 40 | 0 | 95.31 → 95.12 | rustc PID 3449786: 102.9 | 124.59 → 126.40 | P3, 420 MHz, 810 MHz, 21 %, 1857 MiB → P3, 555 MHz, 5001 MHz, 27 %, 1857 MiB |
| m06-play-on-p3 | `c2cf9ab5` | 40 | 0 | 95.12 → 97.53 | rustc PID 3661677: 79.7 | 129.32 → 124.04 | P3, 495 MHz, 5001 MHz, 19 %, 1857 MiB → P3, 480 MHz, 5001 MHz, 26 %, 1888 MiB |
| m06-play-off-p4 | `c2cf9ab5` | 40 | 0 | 97.53 → 94.72 | codex PID 2307914: 33.4 | 205.69 → 151.80 | P3, 450 MHz, 5001 MHz, 26 %, 1887 MiB → P3, 480 MHz, 5001 MHz, 26 %, 1874 MiB |
| m06-play-on-p4 | `c2cf9ab5` | 40 | 0 | 94.72 → 99.21 | rustc PID 3842417: 82.8 | 125.74 → 126.90 | P3, 435 MHz, 5001 MHz, 24 %, 1874 MiB → P3, 555 MHz, 5001 MHz, 23 %, 1874 MiB |
| m07-paused-on-p0 | `c2cf9ab5` | 40 | 0 | 99.21 → 97.24 | rustc PID 4131489: 255.0 | 1.00 → 176.99 | P3, 510 MHz, 5001 MHz, 23 %, 1874 MiB → P3, 1365 MHz, 5001 MHz, 17 %, 1873 MiB |
| m07-paused-on-p1 | `c2cf9ab5` | 40 | 0 | 97.24 → 113.78 | muse PID 2164178: 652.0 | 1.33 → 140.57 | P3, 750 MHz, 5001 MHz, 17 %, 1873 MiB → P3, 645 MHz, 5001 MHz, 17 %, 3064 MiB |
| m07-paused-on-p2 | `c2cf9ab5` | 40 | -6 | 113.78 → 107.19 | rustc PID 3289469: 240.2 | 1.00 → 172.97 | P3, 570 MHz, 5001 MHz, 19 %, 3064 MiB → P2, 1800 MHz, 9501 MHz, 10 %, 7105 MiB |
| m07-paused-on-p3 | `c2cf9ab5` | 40 | 0 | 107.19 → 105.37 | rustc PID 346877: 276.8 | 1.01 → 169.45 | P2, 1800 MHz, 9501 MHz, 10 %, 7104 MiB → P2, 1950 MHz, 9501 MHz, 90 %, 8148 MiB |
| m07-paused-on-p4 | `c2cf9ab5` | 40 | 0 | 105.37 → 100.10 | rustc PID 2415694: 257.1 | 1.33 → 168.39 | P2, 1950 MHz, 9501 MHz, 57 %, 8149 MiB → P2, 1950 MHz, 9501 MHz, 70 %, 8189 MiB |
| m07-play-on-p0 | `c2cf9ab5` | 40 | 0; audio output stream failed | 100.10 → 108.31 | rustc PID 3719098: 272.1 | 126.70 → 198.50 | P2, 1950 MHz, 9501 MHz, 64 %, 8189 MiB → P2, 1950 MHz, 9501 MHz, 83 %, 8168 MiB |
| m07-play-on-p1 | `c2cf9ab5` | 40 | 0 | 108.31 → 115.64 | rustc PID 1596844: 266.4 | 129.18 → 223.75 | P2, 1950 MHz, 9501 MHz, 48 %, 8168 MiB → P2, 1950 MHz, 9501 MHz, 75 %, 8231 MiB |
| m07-play-on-p2 | `c2cf9ab5` | 40 | 0; audio output stream failed | 115.64 → 106.34 | rustc PID 2077568: 269.4 | 120.82 → 177.23 | P2, 1950 MHz, 9501 MHz, 86 %, 8231 MiB → P2, 1950 MHz, 9501 MHz, 78 %, 8281 MiB |
| m07-play-on-p3 | `c2cf9ab5` | 40 | 0 | 105.27 → 122.01 | rustc PID 411688: 273.3 | 135.34 → 228.45 | P2, 1950 MHz, 9501 MHz, 63 %, 8281 MiB → P2, 1950 MHz, 9501 MHz, 62 %, 8276 MiB |
| m07-play-on-p4 | `c2cf9ab5` | NOT RUN | -9 | 122.01 → 116.06 | rustc PID 874606: 264.3 | NOT RUN | P2, 1950 MHz, 9501 MHz, 73 %, 8236 MiB → P2, 1950 MHz, 9501 MHz, 80 %, 8231 MiB |
| m08-paused-on-p0 | `c2cf9ab5` | 40 | 0 | 116.06 → 104.85 | python PID 2261117: 113.6 | 1.33 → 77.02 | P2, 1950 MHz, 9501 MHz, 59 %, 8231 MiB → P2, 1950 MHz, 9501 MHz, 70 %, 8277 MiB |
| m08-paused-on-p1 | `c2cf9ab5` | 40 | 0 | 104.85 → 110.07 | rustc PID 1920051: 129.1 | 1.33 → 75.56 | P2, 1950 MHz, 9501 MHz, 70 %, 8275 MiB → P2, 1950 MHz, 9501 MHz, 54 %, 8234 MiB |
| m08-paused-on-p2 | `c2cf9ab5` | 40 | 0 | 110.07 → 98.83 | rustc PID 1989876: 247.8 | 1.33 → 75.95 | P2, 1950 MHz, 9501 MHz, 65 %, 8234 MiB → P2, 1950 MHz, 9501 MHz, 66 %, 8226 MiB |
| m08-paused-on-p3 | `c2cf9ab5` | 40 | 0 | 99.00 → 100.02 | python PID 2261117: 114.8 | 1.00 → 78.24 | P2, 1950 MHz, 9501 MHz, 56 %, 8258 MiB → P2, 1950 MHz, 9501 MHz, 65 %, 8234 MiB |
| m08-paused-on-p4 | `c2cf9ab5` | 40 | 0 | 100.02 → 98.29 | rustc PID 2682810: 164.0 | 1.33 → 77.04 | P2, 1950 MHz, 9501 MHz, 66 %, 8234 MiB → P2, 1950 MHz, 9501 MHz, 70 %, 8230 MiB |
| m08-play-on-p0 | `c2cf9ab5` | 40 | 0 | 98.29 → 105.36 | rustc PID 2754652: 233.5 | 126.64 → 125.79 | P2, 1950 MHz, 9501 MHz, 70 %, 8230 MiB → P2, 1950 MHz, 9501 MHz, 68 %, 8227 MiB |
| m08-play-on-p1 | `c2cf9ab5` | 40 | 0 | 105.36 → 105.65 | rustc PID 3198772: 229.2 | 117.93 → 126.15 | P2, 1950 MHz, 9501 MHz, 79 %, 8458 MiB → P2, 1950 MHz, 9501 MHz, 73 %, 8225 MiB |
| m08-play-on-p2 | `c2cf9ab5` | 40 | -6 | 103.76 → 95.43 | python PID 2261117: 114.0 | 124.59 → 123.25 | P2, 1950 MHz, 9501 MHz, 66 %, 8225 MiB → P2, 1950 MHz, 9501 MHz, 90 %, 8229 MiB |
| m08-play-on-p3 | `c2cf9ab5` | 40 | 0 | 94.84 → 101.15 | python PID 2261117: 113.6 | 125.36 → 124.51 | P2, 1950 MHz, 9501 MHz, 69 %, 8229 MiB → P2, 1950 MHz, 9501 MHz, 59 %, 8226 MiB |
| m08-play-on-p4 | `c2cf9ab5` | 40 | 0 | 101.15 → 97.12 | python PID 2261117: 114.3 | 124.76 → 122.93 | P2, 1950 MHz, 9501 MHz, 67 %, 8226 MiB → P2, 1950 MHz, 9501 MHz, 79 %, 8226 MiB |
| m09-paused-on-p0 | `c2cf9ab5` | 40 | 0 | 97.12 → 104.10 | python PID 2261117: 114.5 | 1.00 → 22.91 | P2, 1950 MHz, 9501 MHz, 82 %, 8226 MiB → P2, 1950 MHz, 9501 MHz, 80 %, 8225 MiB |
| m09-paused-on-p1 | `c2cf9ab5` | 40 | 0 | 104.10 → 99.40 | python PID 2261117: 114.2 | 1.33 → 22.30 | P2, 1950 MHz, 9501 MHz, 48 %, 8225 MiB → P2, 1950 MHz, 9501 MHz, 67 %, 8229 MiB |
| m09-paused-on-p2 | `c2cf9ab5` | 40 | 0 | 106.34 → 105.26 | rustc PID 816931: 237.3 | 1.00 → 22.58 | P2, 1950 MHz, 9501 MHz, 66 %, 8229 MiB → P2, 1950 MHz, 9501 MHz, 84 %, 8226 MiB |
| m09-paused-on-p3 | `c2cf9ab5` | 40 | 0 | 105.26 → 97.45 | rustc PID 1019655: 188.4 | 1.67 → 22.82 | P2, 1950 MHz, 9501 MHz, 58 %, 8226 MiB → P2, 1950 MHz, 9501 MHz, 58 %, 8229 MiB |
| m09-paused-on-p4 | `c2cf9ab5` | 40 | 0 | 97.45 → 97.24 | rustc PID 1442221: 222.4 | 1.00 → 22.35 | P2, 1950 MHz, 9501 MHz, 71 %, 8229 MiB → P2, 1950 MHz, 9501 MHz, 82 %, 8226 MiB |
| m09-play-on-p0 | `c2cf9ab5` | 40 | 0 | 97.24 → 97.54 | python PID 2261117: 115.7 | 118.30 → 123.35 | P2, 1950 MHz, 9501 MHz, 71 %, 8226 MiB → P2, 1950 MHz, 9501 MHz, 64 %, 8229 MiB |
| m09-play-on-p1 | `c2cf9ab5` | 40 | 0 | 97.41 → 97.33 | python PID 2261117: 114.1 | 127.38 → 124.78 | P2, 1950 MHz, 9501 MHz, 66 %, 8229 MiB → P2, 1950 MHz, 9501 MHz, 69 %, 8226 MiB |
| m09-play-on-p2 | `c2cf9ab5` | 40 | 0 | 97.33 → 99.89 | python PID 2261117: 115.2 | 124.60 → 123.26 | P2, 1950 MHz, 9501 MHz, 71 %, 8226 MiB → P2, 1950 MHz, 9501 MHz, 69 %, 8265 MiB |
| m09-play-on-p3 | `c2cf9ab5` | 40 | 0 | 99.89 → 102.13 | rustc PID 2660722: 156.5 | 122.16 → 123.98 | P2, 1950 MHz, 9501 MHz, 71 %, 8247 MiB → P2, 1950 MHz, 9501 MHz, 74 %, 8205 MiB |
| m09-play-on-p4 | `c2cf9ab5` | 40 | 0 | 102.13 → 95.45 | python PID 2261117: 113.8 | 123.54 → 122.40 | P2, 1950 MHz, 9501 MHz, 76 %, 8205 MiB → P2, 1950 MHz, 9501 MHz, 64 %, 8314 MiB |
| m10-paused-on-p0 | `c2cf9ab5` | 40 | 0 | 95.45 → 93.77 | rustc PID 3293674: 220.6 | 1.00 → 80.42 | P2, 1950 MHz, 9501 MHz, 86 %, 8314 MiB → P2, 1950 MHz, 9501 MHz, 73 %, 8241 MiB |
| m10-paused-on-p1 | `c2cf9ab5` | 40 | -11 | 93.77 → 93.31 | python PID 2261117: 114.7 | 1.67 → 83.59 | P2, 1950 MHz, 9501 MHz, 91 %, 8241 MiB → P2, 1950 MHz, 9501 MHz, 67 %, 8274 MiB |
| m10-paused-on-p2 | `c2cf9ab5` | 40 | -6 | 93.31 → 101.76 | python PID 2261117: 113.9 | 0.67 → 79.16 | P2, 1950 MHz, 9501 MHz, 52 %, 8273 MiB → P2, 1950 MHz, 9501 MHz, 54 %, 8226 MiB |
| m10-paused-on-p3 | `c2cf9ab5` | 40 | 0 | 101.76 → 94.94 | python PID 2261117: 115.0 | 1.00 → 81.24 | P2, 1950 MHz, 9501 MHz, 69 %, 8226 MiB → P2, 1950 MHz, 9501 MHz, 66 %, 8213 MiB |
| m10-paused-on-p4 | `c2cf9ab5` | 40 | 0 | 94.94 → 97.84 | rustc PID 114548: 262.6 | 1.33 → 81.03 | P2, 1950 MHz, 9501 MHz, 68 %, 8213 MiB → P2, 1950 MHz, 9501 MHz, 95 %, 8214 MiB |
| m10-play-on-p0 | `c2cf9ab5` | 40 | 0 | 97.84 → 96.19 | rustc PID 476661: 196.5 | 121.96 → 166.82 | P2, 1950 MHz, 9501 MHz, 61 %, 8214 MiB → P2, 1950 MHz, 9501 MHz, 77 %, 8253 MiB |
| m10-play-on-p1 | `c2cf9ab5` | 40 | 0 | 96.19 → 98.32 | python PID 2261117: 112.1 | 125.45 → 168.33 | P2, 1950 MHz, 9501 MHz, 79 %, 8253 MiB → P2, 1950 MHz, 9501 MHz, 71 %, 8286 MiB |
| m10-play-on-p2 | `c2cf9ab5` | 40 | 0 | 98.32 → 98.47 | rustc PID 1001535: 258.0 | 120.90 → 170.69 | P2, 1950 MHz, 9501 MHz, 65 %, 8277 MiB → P2, 1950 MHz, 9501 MHz, 80 %, 8350 MiB |
| m10-play-on-p3 | `c2cf9ab5` | 40 | 0 | 97.79 → 103.32 | python PID 2261117: 114.8 | 125.74 → 168.85 | P2, 1950 MHz, 9501 MHz, 75 %, 8350 MiB → P2, 1950 MHz, 9501 MHz, 76 %, 8251 MiB |
| m10-play-on-p4 | `c2cf9ab5` | 40 | 0; audio output stream failed | 103.32 → 105.51 | rustc PID 1551711: 228.7 | 120.44 → 126.05 | P2, 1950 MHz, 9501 MHz, 51 %, 8251 MiB → P2, 1950 MHz, 9501 MHz, 68 %, 8525 MiB |
| m03-paused-on-p0 | `60e8befc` | 40 | 0 | 91.52 → 99.47 | rustc PID 3953157: 253.3 | 1.67 → 100.77 | P2, 1950 MHz, 9501 MHz, 64 %, 8311 MiB → P2, 1950 MHz, 9501 MHz, 75 %, 8368 MiB |
| m03-paused-on-p1 | `60e8befc` | 40 | 0 | 99.47 → 100.60 | rustc PID 4171236: 135.2 | 1.99 → 99.87 | P2, 1950 MHz, 9501 MHz, 78 %, 8368 MiB → P2, 1950 MHz, 9501 MHz, 75 %, 8284 MiB |
| m03-paused-on-p2 | `60e8befc` | 40 | 0 | 100.60 → 96.12 | rustc PID 35048: 277.8 | 2.00 → 98.62 | P2, 1950 MHz, 9501 MHz, 75 %, 8284 MiB → P2, 1950 MHz, 9501 MHz, 60 %, 8344 MiB |
| m03-paused-on-p3 | `60e8befc` | 40 | 0 | 96.12 → 92.42 | rustc PID 313678: 244.4 | 1.33 → 100.09 | P2, 1950 MHz, 9501 MHz, 70 %, 8344 MiB → P2, 1950 MHz, 9501 MHz, 61 %, 8341 MiB |
| m03-paused-on-p4 | `60e8befc` | 40 | 0 | 92.42 → 97.15 | rustc PID 590575: 241.7 | 1.00 → 99.10 | P2, 1950 MHz, 9501 MHz, 65 %, 8341 MiB → P2, 1950 MHz, 9501 MHz, 72 %, 8284 MiB |
| m03-play-on-p0 | `60e8befc` | 40 | 0 | 97.15 → 86.78 | rustc PID 979977: 262.0 | 130.89 → 181.58 | P2, 1950 MHz, 9501 MHz, 59 %, 8284 MiB → P2, 1950 MHz, 9501 MHz, 71 %, 8290 MiB |
| m03-play-on-p1 | `60e8befc` | 40 | 0 | 86.78 → 102.76 | rustc PID 2703844: 253.2 | 129.02 → 183.49 | P2, 1950 MHz, 9501 MHz, 71 %, 8290 MiB → P5, 405 MHz, 810 MHz, 56 %, 1948 MiB |
| m03-play-on-p2 | `60e8befc` | 40 | 0 | 102.76 → 88.36 | rustc PID 35473: 286.1 | 129.67 → 183.28 | P5, 735 MHz, 810 MHz, 56 %, 1953 MiB → P3, 1050 MHz, 5001 MHz, 48 %, 2117 MiB |
| m03-play-on-p3 | `60e8befc` | 40 | 0 | 89.53 → 91.65 | kinewright_app- PID 678435: 84.1 | 131.94 → 186.03 | P3, 1050 MHz, 5001 MHz, 15 %, 2117 MiB → P3, 1245 MHz, 5001 MHz, 19 %, 2101 MiB |
| m03-play-on-p4 | `60e8befc` | 40 | 0 | 91.65 → 95.58 | python PID 3249655: 292.5 | 130.02 → 139.77 | P3, 690 MHz, 5001 MHz, 12 %, 2101 MiB → P3, 390 MHz, 5001 MHz, 29 %, 2161 MiB |

## Accepted lavapipe-60

| Run | Source | Reps | Exit / errors | Load 1m start → end | Other CPU peak (one core %) | Own CPU static → active (%) | GPU start → end |
| --- | --- | ---: | --- | --- | --- | --- | --- |
| m01-paused-on-p0 | `c2cf9ab5` | 40 | 0 | 105.51 → 104.06 | python PID 2261117: 114.5 | 41.84 → 222.47 | P2, 1950 MHz, 9501 MHz, 74 %, 8523 MiB → P2, 1950 MHz, 9501 MHz, 73 %, 8251 MiB |
| m01-paused-on-p1 | `c2cf9ab5` | 40 | 0 | 104.06 → 103.56 | python PID 2261117: 114.9 | 15.26 → 209.52 | P2, 1950 MHz, 9501 MHz, 84 %, 8251 MiB → P2, 1950 MHz, 9501 MHz, 64 %, 8251 MiB |
| m01-paused-on-p2 | `c2cf9ab5` | 40 | 0 | 103.56 → 102.34 | python PID 2261117: 115.7 | 18.84 → 231.73 | P2, 1950 MHz, 9501 MHz, 54 %, 8251 MiB → P2, 1950 MHz, 9501 MHz, 76 %, 8246 MiB |
| m01-paused-on-p3 | `c2cf9ab5` | 40 | 0 | 102.34 → 107.25 | rustc PID 2625140: 140.3 | 15.17 → 230.87 | P2, 1950 MHz, 9501 MHz, 52 %, 8246 MiB → P2, 1950 MHz, 9501 MHz, 62 %, 8247 MiB |
| m01-paused-on-p4 | `c2cf9ab5` | 40 | 0 | 107.25 → 103.46 | rustc PID 2688971: 238.8 | 16.61 → 223.67 | P2, 1950 MHz, 9501 MHz, 74 %, 8247 MiB → P2, 1950 MHz, 9501 MHz, 70 %, 8251 MiB |
| m01-play-on-p0 | `c2cf9ab5` | 40 | 0 | 103.46 → 113.21 | python PID 2261117: 113.6 | 249.66 → 232.50 | P2, 1950 MHz, 9501 MHz, 70 %, 8251 MiB → P2, 1950 MHz, 9501 MHz, 63 %, 8275 MiB |
| m01-play-on-p1 | `c2cf9ab5` | 40 | 0 | 113.21 → 112.31 | rustc PID 3333317: 214.6 | 266.04 → 252.14 | P2, 1950 MHz, 9501 MHz, 69 %, 8275 MiB → P2, 1950 MHz, 9501 MHz, 77 %, 8251 MiB |
| m01-play-on-p2 | `c2cf9ab5` | 40 | 0 | 112.31 → 114.94 | python PID 2261117: 116.0 | 252.39 → 229.76 | P2, 1950 MHz, 9501 MHz, 77 %, 8251 MiB → P2, 1950 MHz, 9501 MHz, 65 %, 8250 MiB |
| m01-play-on-p3 | `c2cf9ab5` | 40 | 0 | 114.94 → 106.55 | python PID 2261117: 115.4 | 243.04 → 230.72 | P2, 1950 MHz, 9501 MHz, 53 %, 8250 MiB → P2, 1950 MHz, 9501 MHz, 84 %, 8250 MiB |
| m01-play-on-p4 | `c2cf9ab5` | 40 | 0 | 106.55 → 100.69 | python PID 2261117: 115.8 | 251.25 → 245.90 | P2, 1950 MHz, 9501 MHz, 71 %, 8250 MiB → P2, 1950 MHz, 9501 MHz, 91 %, 8461 MiB |
| m02-paused-on-p0 | `c2cf9ab5` | 40 | 0 | 100.69 → 109.24 | python PID 2261117: 115.6 | 13.19 → 233.56 | P2, 1950 MHz, 9501 MHz, 60 %, 8461 MiB → P2, 1950 MHz, 9501 MHz, 85 %, 8560 MiB |
| m02-paused-on-p1 | `c2cf9ab5` | 40 | 0 | 109.24 → 107.83 | python PID 2261117: 116.1 | 14.66 → 237.22 | P2, 1950 MHz, 9501 MHz, 85 %, 8560 MiB → P2, 1950 MHz, 9501 MHz, 73 %, 8567 MiB |
| m02-paused-on-p2 | `c2cf9ab5` | 40 | 0 | 107.83 → 109.94 | ffmpeg PID 1953600: 622.2 | 13.66 → 194.48 | P2, 1950 MHz, 9501 MHz, 74 %, 8567 MiB → P2, 1950 MHz, 9501 MHz, 69 %, 8478 MiB |
| m02-paused-on-p3 | `c2cf9ab5` | 40 | 0 | 109.94 → 124.33 | ffmpeg PID 2642580: 581.0 | 13.53 → 170.78 | P2, 1950 MHz, 9501 MHz, 69 %, 8535 MiB → P2, 1950 MHz, 9501 MHz, 73 %, 8538 MiB |
| m02-paused-on-p4 | `c2cf9ab5` | 40 | 0 | 124.33 → 110.05 | ffmpeg PID 2693306: 500.4 | 12.16 → 221.21 | P2, 1950 MHz, 9501 MHz, 64 %, 8538 MiB → P2, 1950 MHz, 9501 MHz, 76 %, 8515 MiB |
| m02-play-on-p0 | `c2cf9ab5` | 40 | 0; audio output stream failed | 110.05 → 103.30 | python PID 2261117: 114.5 | 260.72 → 210.25 | P2, 1950 MHz, 9501 MHz, 78 %, 8515 MiB → P2, 1950 MHz, 9501 MHz, 79 %, 8510 MiB |
| m02-play-on-p1 | `c2cf9ab5` | 40 | 0; audio output stream failed | 103.30 → 108.07 | python PID 2261117: 113.3 | 222.34 → 224.86 | P2, 1950 MHz, 9501 MHz, 44 %, 8510 MiB → P2, 1950 MHz, 9501 MHz, 59 %, 8566 MiB |
| m02-play-on-p2 | `c2cf9ab5` | 40 | 0 | 108.07 → 109.99 | python PID 2261117: 113.8 | 269.19 → 250.64 | P2, 1950 MHz, 9501 MHz, 66 %, 8552 MiB → P2, 1950 MHz, 9501 MHz, 74 %, 8509 MiB |
| m02-play-on-p3 | `c2cf9ab5` | 40 | 0 | 109.99 → 118.82 | python PID 2261117: 113.7 | 248.20 → 242.30 | P2, 1950 MHz, 9501 MHz, 72 %, 8509 MiB → P2, 1950 MHz, 9501 MHz, 71 %, 8506 MiB |
| m02-play-on-p4 | `c2cf9ab5` | 40 | 0 | 118.82 → 112.59 | python PID 2261117: 112.8 | 247.49 → 243.53 | P2, 1950 MHz, 9501 MHz, 73 %, 8506 MiB → P2, 1950 MHz, 9501 MHz, 90 %, 8533 MiB |
| m04-paused-on-p0 | `c2cf9ab5` | 40 | 0 | 97.90 → 99.43 | rustc PID 1317173: 282.4 | 13.23 → 171.66 | P2, 1950 MHz, 9501 MHz, 79 %, 8300 MiB → P2, 1950 MHz, 9501 MHz, 65 %, 8260 MiB |
| m04-paused-on-p1 | `c2cf9ab5` | 40 | 0 | 99.43 → 100.13 | rustc PID 1638255: 218.4 | 12.93 → 174.45 | P2, 1950 MHz, 9501 MHz, 84 %, 8260 MiB → P2, 1950 MHz, 9501 MHz, 49 %, 8260 MiB |
| m04-paused-on-p2 | `c2cf9ab5` | 40 | 0 | 99.32 → 98.19 | rustc PID 1753607: 256.9 | 16.43 → 177.13 | P2, 1950 MHz, 9501 MHz, 49 %, 8260 MiB → P2, 1950 MHz, 9501 MHz, 85 %, 8278 MiB |
| m04-paused-on-p3 | `c2cf9ab5` | 40 | 0 | 98.19 → 96.90 | python PID 2261117: 114.1 | 13.59 → 161.18 | P2, 1950 MHz, 9501 MHz, 85 %, 8278 MiB → P2, 1950 MHz, 9501 MHz, 71 %, 8284 MiB |
| m04-paused-on-p4 | `c2cf9ab5` | 40 | 0 | 96.90 → 103.70 | rustc PID 2511637: 150.7 | 14.17 → 174.13 | P2, 1950 MHz, 9501 MHz, 54 %, 8284 MiB → P2, 1950 MHz, 9501 MHz, 76 %, 8378 MiB |
| m04-play-on-p0 | `c2cf9ab5` | 40 | 0 | 103.70 → 112.87 | rustc PID 2872718: 205.1 | 257.70 → 260.84 | P2, 1950 MHz, 9501 MHz, 80 %, 8378 MiB → P2, 1950 MHz, 9501 MHz, 58 %, 8352 MiB |
| m04-play-on-p1 | `c2cf9ab5` | 40 | 0 | 112.87 → 107.41 | rustc PID 3142494: 285.8 | 294.09 → 260.37 | P2, 1950 MHz, 9501 MHz, 75 %, 8353 MiB → P2, 1950 MHz, 9501 MHz, 87 %, 8278 MiB |
| m04-play-on-p2 | `c2cf9ab5` | 40 | 0 | 106.02 → 105.10 | rustc PID 3430451: 210.4 | 284.25 → 253.10 | P2, 1950 MHz, 9501 MHz, 81 %, 8278 MiB → P2, 1950 MHz, 9501 MHz, 58 %, 8274 MiB |
| m04-play-on-p3 | `c2cf9ab5` | 40 | 0 | 105.10 → 106.07 | rustc PID 3737373: 243.5 | 247.33 → 250.40 | P2, 1950 MHz, 9501 MHz, 76 %, 8274 MiB → P2, 1950 MHz, 9501 MHz, 81 %, 8335 MiB |
| m04-play-on-p4 | `c2cf9ab5` | 40 | 0 | 106.07 → 106.89 | rustc PID 3926300: 213.5 | 243.06 → 276.02 | P2, 1950 MHz, 9501 MHz, 62 %, 8335 MiB → P2, 1950 MHz, 9501 MHz, 79 %, 8488 MiB |
| m03-paused-on-p0 | `60e8befc` | 40 | 0 | 95.58 → 101.25 | python PID 3249655: 294.4 | 24.10 → 128.31 | P5, 915 MHz, 810 MHz, 51 %, 2161 MiB → P3, 1200 MHz, 5001 MHz, 11 %, 1904 MiB |
| m03-paused-on-p1 | `60e8befc` | 40 | 0 | 101.25 → 106.79 | python PID 3249655: 90.2 | 18.16 → 116.72 | P3, 1200 MHz, 5001 MHz, 11 %, 1904 MiB → P3, 1230 MHz, 5001 MHz, 12 %, 1961 MiB |
| m03-paused-on-p2 | `60e8befc` | 40 | 0 | 106.79 → 98.00 | python PID 3249655: 107.8 | 16.98 → 123.34 | P3, 1230 MHz, 5001 MHz, 12 %, 1961 MiB → P3, 1140 MHz, 5001 MHz, 14 %, 1965 MiB |
| m03-paused-on-p3 | `60e8befc` | 40 | 0 | 98.00 → 97.77 | rustc PID 1039164: 196.1 | 20.98 → 125.25 | P3, 1140 MHz, 5001 MHz, 12 %, 1965 MiB → P5, 900 MHz, 810 MHz, 22 %, 2172 MiB |
| m03-paused-on-p4 | `60e8befc` | 40 | 0 | 97.77 → 94.39 | python PID 3266184: 86.4 | 22.99 → 130.64 | P3, 1095 MHz, 5001 MHz, 15 %, 2172 MiB → P5, 840 MHz, 810 MHz, 50 %, 2115 MiB |
| m03-play-on-p0 | `60e8befc` | 0 | 0; audio output stream failed | 94.39 → 109.87 | ffmpeg PID 2665439: 581.1 | 199.14 → 125.65 | P3, 1155 MHz, 5001 MHz, 13 %, 2114 MiB → P3, 585 MHz, 5001 MHz, 18 %, 2133 MiB |
| m03-play-on-p1 | `60e8befc` | 40 | 0; audio output stream failed | 109.08 → 101.46 | ffmpeg PID 2868936: 540.6 | 180.61 → 249.27 | P3, 510 MHz, 5001 MHz, 19 %, 2133 MiB → P2, 1800 MHz, 9501 MHz, 10 %, 8480 MiB |
| m03-play-on-p2 | `60e8befc` | NOT RUN | TIMEOUT | 101.46 → 96.23 | voxtype PID 2230: 249.6 | NOT RUN | P2, 1800 MHz, 9501 MHz, 10 %, 8480 MiB → P2, 1950 MHz, 9501 MHz, 85 %, 15057 MiB |
| m03-play-on-p3 | `60e8befc` | NOT RUN | TIMEOUT | 96.23 → 83.19 | python PID 3057186: 130.2 | NOT RUN | P2, 1950 MHz, 9501 MHz, 100 %, 15092 MiB → P2, 1950 MHz, 9501 MHz, 52 %, 15308 MiB |
| m03-play-on-p4 | `60e8befc` | 40 | 0 | 82.53 → 93.99 | rustc PID 2406620: 167.8 | 289.26 → 292.50 | P2, 1950 MHz, 9501 MHz, 54 %, 15376 MiB → P2, 1950 MHz, 9501 MHz, 84 %, 15232 MiB |
| m05-paused-on-p0 | `60e8befc` | 40 | 0 | 93.99 → 96.27 | rustc PID 2475959: 256.2 | 14.92 → 229.51 | P2, 1950 MHz, 9501 MHz, 97 %, 15233 MiB → P2, 1950 MHz, 9501 MHz, 92 %, 15228 MiB |
| m05-paused-on-p1 | `60e8befc` | 40 | 0 | 96.27 → 93.75 | python PID 3057186: 125.1 | 15.00 → 234.37 | P2, 1950 MHz, 9501 MHz, 96 %, 15228 MiB → P2, 1950 MHz, 9501 MHz, 95 %, 15225 MiB |
| m05-paused-on-p2 | `60e8befc` | 40 | 0 | 93.75 → 87.26 | rustc PID 3459784: 265.1 | 14.58 → 247.31 | P2, 1950 MHz, 9501 MHz, 72 %, 15225 MiB → P2, 1950 MHz, 9501 MHz, 59 %, 15270 MiB |
| m05-paused-on-p3 | `60e8befc` | 40 | 0 | 84.59 → 85.73 | rustc PID 6236: 266.1 | 14.10 → 241.90 | P2, 1950 MHz, 9501 MHz, 97 %, 15273 MiB → P2, 1950 MHz, 9501 MHz, 94 %, 15256 MiB |
| m05-paused-on-p4 | `60e8befc` | 40 | 0 | 85.73 → 89.93 | rustc PID 541459: 226.3 | 15.57 → 243.87 | P2, 1950 MHz, 9501 MHz, 94 %, 15308 MiB → P2, 1950 MHz, 9501 MHz, 73 %, 15301 MiB |
| m05-play-on-p0 | `60e8befc` | 40 | 0 | 89.93 → 93.89 | rustc PID 961827: 192.4 | 305.87 → 250.97 | P2, 1950 MHz, 9501 MHz, 73 %, 15318 MiB → P2, 1950 MHz, 9501 MHz, 49 %, 15309 MiB |
| m05-play-on-p1 | `60e8befc` | 40 | 0 | 93.89 → 93.16 | rustc PID 1119977: 214.4 | 269.38 → 273.20 | P2, 1950 MHz, 9501 MHz, 78 %, 15315 MiB → P2, 1950 MHz, 9501 MHz, 98 %, 15319 MiB |
| m05-play-on-p2 | `60e8befc` | 40 | 0 | 93.16 → 89.45 | rustc PID 1511719: 130.6 | 339.33 → 391.57 | P2, 1950 MHz, 9501 MHz, 83 %, 15319 MiB → P2, 1950 MHz, 9501 MHz, 80 %, 15319 MiB |
| m05-play-on-p3 | `60e8befc` | 40 | 0 | 89.45 → 100.34 | python PID 3057186: 124.8 | 354.71 → 307.69 | P2, 1950 MHz, 9501 MHz, 84 %, 15319 MiB → P2, 1950 MHz, 9501 MHz, 100 %, 15283 MiB |
| m05-play-on-p4 | `60e8befc` | 40 | 0 | 100.34 → 94.50 | python PID 3057186: 126.5 | 292.64 → 299.30 | P2, 1950 MHz, 9501 MHz, 100 %, 15283 MiB → P2, 1950 MHz, 9501 MHz, 93 %, 15286 MiB |
| m06-paused-off-p0 | `60e8befc` | 40 | 0 | 94.50 → 91.53 | python PID 3057186: 123.9 | 7.62 → 216.38 | P2, 1950 MHz, 9501 MHz, 96 %, 15286 MiB → P2, 1950 MHz, 9501 MHz, 22 %, 15265 MiB |
| m06-paused-on-p0 | `60e8befc` | 40 | 0 | 91.53 → 90.70 | python PID 3057186: 125.5 | 15.06 → 278.35 | P2, 1950 MHz, 9501 MHz, 22 %, 15282 MiB → P2, 1950 MHz, 9501 MHz, 79 %, 15282 MiB |
| m06-paused-off-p1 | `60e8befc` | 40 | 0 | 90.70 → 92.01 | ja_s1_storage-3 PID 3490137: 214.3 | 5.53 → 191.39 | P2, 1950 MHz, 9501 MHz, 83 %, 15282 MiB → P2, 1950 MHz, 9501 MHz, 82 %, 15282 MiB |
| m06-paused-on-p1 | `60e8befc` | 40 | 0 | 92.01 → 94.91 | ja_s1_storage-3 PID 3490137: 286.3 | 13.48 → 217.77 | P2, 1950 MHz, 9501 MHz, 86 %, 15282 MiB → P2, 1950 MHz, 9501 MHz, 97 %, 15286 MiB |
| m06-paused-off-p2 | `60e8befc` | 40 | 0 | 94.91 → 101.52 | ja_s1_storage-3 PID 3490137: 205.1 | 5.44 → 171.70 | P2, 1950 MHz, 9501 MHz, 99 %, 15286 MiB → P2, 1950 MHz, 9501 MHz, 76 %, 15282 MiB |
| m06-paused-on-p2 | `60e8befc` | 40 | 0 | 101.52 → 103.28 | ja_s1_storage-3 PID 3490137: 200.3 | 15.13 → 232.23 | P2, 1950 MHz, 9501 MHz, 76 %, 15282 MiB → P2, 1950 MHz, 9501 MHz, 76 %, 15284 MiB |
| m06-paused-off-p3 | `60e8befc` | 40 | 0 | 103.28 → 99.43 | ja_s1_storage-3 PID 3490137: 133.7 | 7.32 → 165.50 | P2, 1950 MHz, 9501 MHz, 65 %, 15293 MiB → P2, 1950 MHz, 9501 MHz, 78 %, 15293 MiB |
| m06-paused-on-p3 | `60e8befc` | 40 | 0 | 99.43 → 98.49 | ja_s1_storage-3 PID 3490137: 134.5 | 12.87 → 242.02 | P2, 1950 MHz, 9501 MHz, 68 %, 15293 MiB → P2, 1950 MHz, 9501 MHz, 94 %, 15269 MiB |
| m06-paused-off-p4 | `60e8befc` | 40 | 0 | 98.49 → 103.42 | ja_s1_storage-3 PID 3490137: 132.9 | 7.33 → 182.08 | P2, 1950 MHz, 9501 MHz, 90 %, 15293 MiB → P2, 1950 MHz, 9501 MHz, 84 %, 15293 MiB |
| m06-paused-on-p4 | `60e8befc` | 40 | 0 | 103.42 → 102.82 | python PID 3057186: 123.1 | 13.51 → 238.59 | P2, 1950 MHz, 9501 MHz, 74 %, 15293 MiB → P2, 1950 MHz, 9501 MHz, 95 %, 15293 MiB |
| m06-play-off-p0 | `60e8befc` | 40 | 0 | 102.82 → 107.29 | python PID 3057186: 126.4 | 218.02 → 218.05 | P2, 1950 MHz, 9501 MHz, 96 %, 15293 MiB → P2, 1950 MHz, 9501 MHz, 94 %, 15288 MiB |
| m06-play-on-p0 | `60e8befc` | 40 | 0 | 107.29 → 111.20 | python PID 3057186: 125.3 | 260.42 → 273.82 | P2, 1950 MHz, 9501 MHz, 100 %, 15296 MiB → P2, 1950 MHz, 9501 MHz, 94 %, 15284 MiB |
| m06-play-off-p1 | `60e8befc` | 40 | 0 | 111.20 → 113.10 | python PID 3057186: 123.9 | 202.80 → 212.97 | P2, 1950 MHz, 9501 MHz, 78 %, 15293 MiB → P2, 1950 MHz, 9501 MHz, 80 %, 15293 MiB |
| m06-play-on-p1 | `60e8befc` | 40 | 0 | 113.10 → 112.26 | python PID 3057186: 124.8 | 291.25 → 323.88 | P2, 1950 MHz, 9501 MHz, 80 %, 15293 MiB → P2, 1950 MHz, 9501 MHz, 90 %, 15284 MiB |
| m06-play-off-p2 | `60e8befc` | 40 | 0 | 112.26 → 117.29 | ja_s2_rescue-8f PID 665465: 201.7 | 283.48 → 246.84 | P2, 1950 MHz, 9501 MHz, 56 %, 15293 MiB → P2, 1950 MHz, 9501 MHz, 26 %, 15293 MiB |
| m06-play-on-p2 | `60e8befc` | 40 | 0 | 117.29 → 111.79 | ja_s2_rescue-8f PID 665465: 152.2 | 336.70 → 311.10 | P2, 1950 MHz, 9501 MHz, 26 %, 15293 MiB → P2, 1950 MHz, 9501 MHz, 100 %, 15293 MiB |
| m06-play-off-p3 | `60e8befc` | 40 | 0 | 111.79 → 110.31 | python PID 3057186: 127.6 | 235.81 → 283.33 | P2, 1950 MHz, 9501 MHz, 100 %, 15293 MiB → P2, 1950 MHz, 9501 MHz, 87 %, 15297 MiB |
| m06-play-on-p3 | `60e8befc` | 40 | 0 | 110.31 → 113.24 | python PID 3057186: 127.1 | 335.67 → 329.21 | P2, 1950 MHz, 9501 MHz, 80 %, 15297 MiB → P2, 1950 MHz, 9501 MHz, 75 %, 15293 MiB |
| m06-play-off-p4 | `60e8befc` | 40 | 0 | 113.24 → 116.40 | python PID 3057186: 124.9 | 208.27 → 219.35 | P2, 1950 MHz, 9501 MHz, 67 %, 15293 MiB → P2, 1950 MHz, 9501 MHz, 94 %, 15293 MiB |
| m06-play-on-p4 | `60e8befc` | 40 | 0 | 113.40 → 119.18 | python PID 3057186: 122.1 | 281.47 → 267.05 | P2, 1950 MHz, 9501 MHz, 98 %, 15293 MiB → P2, 1950 MHz, 9501 MHz, 75 %, 15288 MiB |
| m07-paused-on-p0 | `60e8befc` | 40 | 0 | 119.18 → 122.05 | python PID 3057186: 126.5 | 14.43 → 337.40 | P2, 1950 MHz, 9501 MHz, 64 %, 15288 MiB → P2, 1950 MHz, 9501 MHz, 53 %, 15397 MiB |
| m07-paused-on-p1 | `60e8befc` | 40 | 0 | 122.05 → 110.84 | python PID 3057186: 126.8 | 14.29 → 339.54 | P2, 1950 MHz, 9501 MHz, 62 %, 15397 MiB → P2, 1950 MHz, 9501 MHz, 99 %, 15403 MiB |
| m07-paused-on-p2 | `60e8befc` | 40 | 0 | 110.84 → 125.65 | rustc PID 1026831: 245.3 | 12.49 → 323.08 | P2, 1950 MHz, 9501 MHz, 71 %, 15403 MiB → P2, 1950 MHz, 9501 MHz, 96 %, 15611 MiB |
| m07-paused-on-p3 | `60e8befc` | 40 | 0 | 125.65 → 122.01 | python PID 3057186: 128.3 | 14.87 → 341.70 | P2, 1950 MHz, 9501 MHz, 99 %, 15611 MiB → P2, 1950 MHz, 9501 MHz, 98 %, 15644 MiB |
| m07-paused-on-p4 | `60e8befc` | 40 | 0 | 122.01 → 115.89 | python PID 3057186: 129.1 | 14.67 → 339.67 | P2, 1950 MHz, 9501 MHz, 100 %, 15671 MiB → P2, 1950 MHz, 9501 MHz, 95 %, 15711 MiB |
| m07-play-on-p0 | `60e8befc` | 40 | 0 | 115.89 → 122.80 | rustc PID 1684988: 267.2 | 293.24 → 363.57 | P2, 1950 MHz, 9501 MHz, 54 %, 15711 MiB → P2, 1950 MHz, 9501 MHz, 76 %, 15774 MiB |
| m07-play-on-p1 | `60e8befc` | 40 | 0 | 122.80 → 123.34 | python PID 3057186: 127.7 | 289.15 → 365.59 | P2, 1950 MHz, 9501 MHz, 18 %, 15774 MiB → P2, 1950 MHz, 9501 MHz, 79 %, 15776 MiB |
| m07-play-on-p2 | `60e8befc` | 40 | 0 | 123.34 → 119.24 | rustc PID 2303406: 211.3 | 351.90 → 371.42 | P2, 1950 MHz, 9501 MHz, 79 %, 15776 MiB → P2, 1950 MHz, 9501 MHz, 96 %, 15611 MiB |
| m07-play-on-p3 | `60e8befc` | 40 | 0; audio output stream failed | 119.24 → 109.06 | kinewright_app- PID 2940595: 289.0 | 290.72 → 343.45 | P2, 1950 MHz, 9501 MHz, 100 %, 15611 MiB → P2, 1950 MHz, 9501 MHz, 94 %, 15820 MiB |
| m07-play-on-p4 | `60e8befc` | 40 | 0 | 109.06 → 121.93 | rustc PID 1086211: 253.1 | 308.55 → 364.77 | P2, 1950 MHz, 9501 MHz, 98 %, 15820 MiB → P2, 1950 MHz, 9501 MHz, 72 %, 15613 MiB |
| m08-paused-on-p0 | `60e8befc` | 40 | 0 | 118.57 → 122.00 | python PID 3057186: 116.8 | 14.51 → 251.32 | P2, 1950 MHz, 9501 MHz, 77 %, 15613 MiB → P2, 1950 MHz, 9501 MHz, 56 %, 15613 MiB |
| m08-paused-on-p1 | `60e8befc` | 40 | 0 | 122.00 → 114.40 | rustc PID 1955087: 197.1 | 14.66 → 312.05 | P2, 1950 MHz, 9501 MHz, 81 %, 15613 MiB → P2, 1950 MHz, 9501 MHz, 100 %, 15629 MiB |
| m08-paused-on-p2 | `60e8befc` | 40 | 0 | 114.40 → 115.22 | python PID 3057186: 120.7 | 14.66 → 404.53 | P2, 1950 MHz, 9501 MHz, 47 %, 15629 MiB → P2, 1950 MHz, 9501 MHz, 100 %, 15629 MiB |
| m08-paused-on-p3 | `60e8befc` | 40 | 0 | 115.22 → 109.05 | python PID 3057186: 121.8 | 14.93 → 407.83 | P2, 1950 MHz, 9501 MHz, 29 %, 15629 MiB → P2, 1950 MHz, 9501 MHz, 100 %, 15674 MiB |
| m08-paused-on-p4 | `60e8befc` | 40 | 0 | 109.05 → 103.83 | python PID 3057186: 116.1 | 16.65 → 292.46 | P2, 1950 MHz, 9501 MHz, 63 %, 15674 MiB → P2, 1950 MHz, 9501 MHz, 100 %, 15669 MiB |
| m08-play-on-p0 | `60e8befc` | 40 | 0; audio output stream failed | 103.83 → 106.90 | python PID 3057186: 117.5 | 331.64 → 302.53 | P2, 1950 MHz, 9501 MHz, 38 %, 15669 MiB → P2, 1950 MHz, 9501 MHz, 20 %, 15670 MiB |
| m08-play-on-p1 | `60e8befc` | 40 | 0 | 106.90 → 105.92 | python PID 3057186: 119.0 | 321.05 → 273.50 | P2, 1950 MHz, 9501 MHz, 79 %, 15670 MiB → P2, 1950 MHz, 9501 MHz, 57 %, 15673 MiB |
| m08-play-on-p2 | `60e8befc` | 40 | 0 | 105.92 → 104.81 | rustc PID 3926873: 203.9 | 303.98 → 316.66 | P2, 1950 MHz, 9501 MHz, 96 %, 15673 MiB → P2, 1950 MHz, 9501 MHz, 40 %, 15669 MiB |
| m08-play-on-p3 | `60e8befc` | 40 | 0 | 104.81 → 101.30 | python PID 3057186: 120.3 | 505.66 → 432.71 | P2, 1950 MHz, 9501 MHz, 80 %, 15669 MiB → P2, 1950 MHz, 9501 MHz, 55 %, 15669 MiB |
| m08-play-on-p4 | `60e8befc` | 40 | 0 | 101.30 → 104.28 | python PID 3057186: 120.1 | 460.10 → 447.27 | P2, 1950 MHz, 9501 MHz, 73 %, 15669 MiB → P2, 1950 MHz, 9501 MHz, 76 %, 15674 MiB |
| m09-paused-on-p0 | `60e8befc` | 40 | 0 | 104.28 → 106.32 | python PID 3057186: 118.5 | 14.27 → 128.10 | P2, 1950 MHz, 9501 MHz, 76 %, 15673 MiB → P2, 1950 MHz, 9501 MHz, 79 %, 15669 MiB |
| m09-paused-on-p1 | `60e8befc` | 40 | 0 | 105.17 → 103.99 | python PID 3057186: 118.1 | 15.24 → 122.98 | P2, 1950 MHz, 9501 MHz, 85 %, 15669 MiB → P2, 1950 MHz, 9501 MHz, 95 %, 15672 MiB |
| m09-paused-on-p2 | `60e8befc` | 40 | 0 | 103.99 → 103.59 | python PID 3057186: 120.5 | 15.63 → 132.87 | P2, 1950 MHz, 9501 MHz, 64 %, 15672 MiB → P2, 1950 MHz, 9501 MHz, 90 %, 15671 MiB |
| m09-paused-on-p3 | `60e8befc` | 40 | 0 | 103.59 → 106.16 | python PID 3057186: 119.5 | 12.29 → 126.78 | P2, 1950 MHz, 9501 MHz, 100 %, 15673 MiB → P2, 1950 MHz, 9501 MHz, 67 %, 15670 MiB |
| m09-paused-on-p4 | `60e8befc` | 40 | 0 | 106.16 → 104.61 | ja_s1_storage-3 PID 2186718: 282.2 | 15.00 → 112.84 | P2, 1950 MHz, 9501 MHz, 100 %, 15670 MiB → P2, 1950 MHz, 9501 MHz, 72 %, 15673 MiB |
| m09-play-on-p0 | `60e8befc` | 40 | 0 | 108.40 → 114.26 | ja_s1_storage-3 PID 2186718: 199.8 | 292.56 → 273.36 | P2, 1950 MHz, 9501 MHz, 100 %, 15673 MiB → P2, 1950 MHz, 9501 MHz, 71 %, 15670 MiB |
| m09-play-on-p1 | `60e8befc` | 40 | 0 | 114.26 → 127.04 | ja_s1_storage-3 PID 2186718: 128.3 | 259.69 → 278.24 | P2, 1950 MHz, 9501 MHz, 87 %, 15670 MiB → P2, 1950 MHz, 9501 MHz, 72 %, 15670 MiB |
| m09-play-on-p2 | `60e8befc` | 40 | 0 | 127.04 → 121.22 | rustc PID 3259650: 209.5 | 227.26 → 270.22 | P2, 1950 MHz, 9501 MHz, 100 %, 15673 MiB → P2, 1950 MHz, 9501 MHz, 92 %, 15670 MiB |
| m09-play-on-p3 | `60e8befc` | 40 | 0 | 121.22 → 116.13 | python PID 3057186: 121.7 | 257.86 → 258.44 | P2, 1950 MHz, 9501 MHz, 100 %, 15670 MiB → P2, 1950 MHz, 9501 MHz, 70 %, 15670 MiB |
| m09-play-on-p4 | `60e8befc` | 40 | 0 | 116.13 → 116.14 | python PID 3057186: 120.2 | 256.60 → 268.47 | P2, 1950 MHz, 9501 MHz, 100 %, 15670 MiB → P2, 1950 MHz, 9501 MHz, 100 %, 15673 MiB |
| m10-paused-on-p0 | `60e8befc` | 40 | 0 | 116.14 → 112.49 | rustc PID 4011439: 152.2 | 14.98 → 329.13 | P2, 1950 MHz, 9501 MHz, 31 %, 15673 MiB → P2, 1950 MHz, 9501 MHz, 87 %, 15690 MiB |
| m10-paused-on-p1 | `60e8befc` | 40 | 0 | 112.49 → 118.58 | python PID 3057186: 120.0 | 14.88 → 418.56 | P2, 1950 MHz, 9501 MHz, 86 %, 15690 MiB → P2, 1950 MHz, 9501 MHz, 100 %, 15693 MiB |
| m10-paused-on-p2 | `60e8befc` | 40 | 0 | 115.25 → 105.32 | python PID 3057186: 119.9 | 14.74 → 346.93 | P2, 1950 MHz, 9501 MHz, 100 %, 15693 MiB → P2, 1950 MHz, 9501 MHz, 84 %, 15690 MiB |
| m10-paused-on-p3 | `60e8befc` | 40 | 0 | 105.32 → 107.98 | python PID 3057186: 118.3 | 14.74 → 304.92 | P2, 1950 MHz, 9501 MHz, 70 %, 15690 MiB → P2, 1950 MHz, 9501 MHz, 98 %, 15694 MiB |
| m10-paused-on-p4 | `60e8befc` | 40 | 0 | 107.98 → 104.68 | python PID 3057186: 118.4 | 13.94 → 311.99 | P2, 1950 MHz, 9501 MHz, 100 %, 15694 MiB → P2, 1950 MHz, 9501 MHz, 29 %, 15693 MiB |
| m10-play-on-p0 | `60e8befc` | 40 | 0 | 104.68 → 106.88 | python PID 3057186: 118.2 | 330.12 → 342.68 | P2, 1950 MHz, 9501 MHz, 29 %, 15693 MiB → P2, 1950 MHz, 9501 MHz, 91 %, 15686 MiB |
| m10-play-on-p1 | `60e8befc` | 40 | 0; audio output stream failed | 106.88 → 109.60 | ja_s1_storage-3 PID 2226161: 212.6 | 314.76 → 307.41 | P2, 1950 MHz, 9501 MHz, 100 %, 15690 MiB → P2, 1950 MHz, 9501 MHz, 92 %, 15694 MiB |
| m10-play-on-p2 | `60e8befc` | 40 | 0 | 109.60 → 121.06 | ja_s1_storage-3 PID 2226161: 264.3 | 257.47 → 315.32 | P2, 1950 MHz, 9501 MHz, 100 %, 15694 MiB → P2, 1950 MHz, 9501 MHz, 38 %, 15668 MiB |
| m10-play-on-p3 | `60e8befc` | 40 | 0 | 121.06 → 117.75 | ja_s1_storage-3 PID 2226161: 134.7 | 270.64 → 299.63 | P2, 1950 MHz, 9501 MHz, 81 %, 15690 MiB → P2, 1950 MHz, 9501 MHz, 100 %, 15694 MiB |
| m10-play-on-p4 | `60e8befc` | 40 | 0 | 117.75 → 117.57 | python PID 3057186: 118.8 | 278.25 → 308.64 | P2, 1950 MHz, 9501 MHz, 100 %, 15694 MiB → P2, 1950 MHz, 9501 MHz, 87 %, 15690 MiB |

## Accepted nvidia-60-snap520

| Run | Source | Reps | Exit / errors | Load 1m start → end | Other CPU peak (one core %) | Own CPU static → active (%) | GPU start → end |
| --- | --- | ---: | --- | --- | --- | --- | --- |
| m01-paused-on-p0-snap520 | `60e8befc` | 40 | 0 | 117.57 → 107.03 | ja_s2_rescue-8f PID 3502683: 141.6 | 1.00 → 59.58 | P2, 1950 MHz, 9501 MHz, 18 %, 15690 MiB → P2, 1950 MHz, 9501 MHz, 69 %, 15690 MiB |
| m01-paused-on-p1-snap520 | `60e8befc` | 40 | 0 | 107.03 → 115.04 | ja_s2_rescue-8f PID 3502683: 184.7 | 1.33 → 62.34 | P2, 1950 MHz, 9501 MHz, 99 %, 15690 MiB → P2, 1950 MHz, 9501 MHz, 100 %, 15774 MiB |
| m01-paused-on-p2-snap520 | `60e8befc` | 40 | 0 | 115.04 → 107.40 | python PID 3057186: 118.8 | 1.67 → 59.61 | P2, 1950 MHz, 9501 MHz, 100 %, 15774 MiB → P2, 1950 MHz, 9501 MHz, 93 %, 15770 MiB |
| m01-paused-on-p3-snap520 | `60e8befc` | 40 | 0 | 107.69 → 102.57 | python PID 3057186: 118.7 | 1.00 → 60.60 | P2, 1950 MHz, 9501 MHz, 66 %, 15770 MiB → P2, 1950 MHz, 9501 MHz, 83 %, 15770 MiB |
| m01-paused-on-p4-snap520 | `60e8befc` | 40 | 0 | 102.57 → 100.80 | python PID 3057186: 119.0 | 1.33 → 59.42 | P2, 1950 MHz, 9501 MHz, 100 %, 15770 MiB → P2, 1950 MHz, 9501 MHz, 95 %, 15774 MiB |
| m01-play-on-p0-snap520 | `60e8befc` | 40 | -6 | 100.80 → 109.64 | python PID 3057186: 117.6 | 128.77 → 126.31 | P2, 1950 MHz, 9501 MHz, 100 %, 15774 MiB → P2, 1950 MHz, 9501 MHz, 59 %, 15770 MiB |
| m01-play-on-p1-snap520 | `60e8befc` | 40 | 0 | 109.64 → 105.35 | python PID 3057186: 119.6 | 125.70 → 126.74 | P2, 1950 MHz, 9501 MHz, 59 %, 15763 MiB → P2, 1950 MHz, 9501 MHz, 62 %, 15770 MiB |
| m01-play-on-p2-snap520 | `60e8befc` | 40 | 0 | 105.35 → 102.52 | python PID 3057186: 119.0 | 120.98 → 130.19 | P2, 1950 MHz, 9501 MHz, 100 %, 15770 MiB → P2, 1950 MHz, 9501 MHz, 63 %, 15773 MiB |
| m01-play-on-p3-snap520 | `60e8befc` | 40 | 0 | 102.52 → 103.71 | python PID 3057186: 119.1 | 176.81 → 133.57 | P2, 1950 MHz, 9501 MHz, 63 %, 15773 MiB → P2, 1950 MHz, 9501 MHz, 63 %, 15770 MiB |
| m01-play-on-p4-snap520 | `60e8befc` | 40 | 0; audio output stream failed | 103.71 → 106.22 | python PID 3057186: 119.9 | 29.97 → 59.91 | P2, 1950 MHz, 9501 MHz, 100 %, 15770 MiB → P2, 1950 MHz, 9501 MHz, 62 %, 15770 MiB |

## Archived original nvidia-60

M3 PF1 G1 is invalid because of the epoch-accounting probe defect. Original partial software M5 is superseded by a complete C3 case. These rows receive no accepted repetition credit.

| Run | Source | Reps | Exit / errors | Load 1m start → end | Other CPU peak (one core %) | Own CPU static → active (%) | GPU start → end |
| --- | --- | ---: | --- | --- | --- | --- | --- |
| m03-paused-on-p0 | `c2cf9ab5` | 40 | 0 | 77.93 → 76.65 | rustc PID 3236301: 274.8 | 2.00 → 99.84 | P2, 1950 MHz, 9501 MHz, 100 %, 13730 MiB → P2, 1950 MHz, 9501 MHz, 77 %, 13786 MiB |
| m03-paused-on-p1 | `c2cf9ab5` | 40 | 0 | 76.65 → 76.24 | kinewright_app- PID 3588149: 261.9 | 1.66 → 97.54 | P2, 1950 MHz, 9501 MHz, 77 %, 13786 MiB → P2, 1950 MHz, 9501 MHz, 99 %, 14010 MiB |
| m03-paused-on-p2 | `c2cf9ab5` | 40 | 0 | 76.24 → 75.22 | kinewright_app- PID 3588149: 250.6 | 2.65 → 101.16 | P2, 1950 MHz, 9501 MHz, 98 %, 14010 MiB → P2, 1950 MHz, 9501 MHz, 45 %, 14169 MiB |
| m03-paused-on-p3 | `c2cf9ab5` | 40 | 0 | 75.22 → 78.12 | kinewright_app- PID 3588149: 213.5 | 1.66 → 96.20 | P2, 1950 MHz, 9501 MHz, 39 %, 14169 MiB → P2, 1950 MHz, 9501 MHz, 78 %, 14201 MiB |
| m03-paused-on-p4 | `c2cf9ab5` | 40 | -6 | 76.67 → 75.91 | kinewright_app- PID 3588149: 228.8 | 1.33 → 100.21 | P2, 1950 MHz, 9501 MHz, 76 %, 14201 MiB → P2, 1950 MHz, 9501 MHz, 83 %, 14157 MiB |
| m03-play-on-p0 | `c2cf9ab5` | 40 | 0 | 75.91 → 83.73 | kinewright_app- PID 3588149: 278.8 | 135.57 → 178.64 | P2, 1950 MHz, 9501 MHz, 80 %, 14156 MiB → P2, 1950 MHz, 9501 MHz, 66 %, 14026 MiB |
| m03-play-on-p1 | `c2cf9ab5` | 40 | 0 | 83.73 → 86.44 | python PID 3145844: 115.4 | 127.65 → 176.82 | P2, 1950 MHz, 9501 MHz, 13 %, 14026 MiB → P2, 1950 MHz, 9501 MHz, 100 %, 14203 MiB |
| m03-play-on-p2 | `c2cf9ab5` | 18 | 0; audio output stream failed | 86.44 → 97.19 | ffmpeg PID 1817344: 539.6 | 130.19 → 103.46 | P2, 1950 MHz, 9501 MHz, 77 %, 14203 MiB → P2, 1950 MHz, 9501 MHz, 100 %, 14203 MiB |
| m03-play-on-p3 | `c2cf9ab5` | 40 | 0; audio output stream failed | 97.19 → 93.66 | kinewright_medi PID 170231: 335.7 | 1.67 → 102.39 | P2, 1950 MHz, 9501 MHz, 100 %, 14203 MiB → P2, 1950 MHz, 9501 MHz, 75 %, 14192 MiB |
| m03-play-on-p4 | `c2cf9ab5` | 40 | 0 | 93.85 → 88.40 | rustc PID 924607: 284.3 | 166.31 → 174.15 | P2, 1950 MHz, 9501 MHz, 75 %, 14192 MiB → P2, 1950 MHz, 9501 MHz, 65 %, 13991 MiB |

## Archived original lavapipe-60

M3 PF1 G1 is invalid because of the epoch-accounting probe defect. Original partial software M5 is superseded by a complete C3 case. These rows receive no accepted repetition credit.

| Run | Source | Reps | Exit / errors | Load 1m start → end | Other CPU peak (one core %) | Own CPU static → active (%) | GPU start → end |
| --- | --- | ---: | --- | --- | --- | --- | --- |
| m03-paused-on-p0 | `c2cf9ab5` | 40 | 0 | 112.59 → 109.92 | python PID 2261117: 114.1 | 19.10 → 202.34 | P2, 1950 MHz, 9501 MHz, 69 %, 8533 MiB → P2, 1950 MHz, 9501 MHz, 72 %, 8499 MiB |
| m03-paused-on-p1 | `c2cf9ab5` | 40 | 0 | 109.92 → 107.55 | python PID 2261117: 114.1 | 21.31 → 228.18 | P2, 1950 MHz, 9501 MHz, 82 %, 8499 MiB → P2, 1950 MHz, 9501 MHz, 69 %, 8498 MiB |
| m03-paused-on-p2 | `c2cf9ab5` | 40 | 0 | 107.55 → 106.10 | python PID 2261117: 113.7 | 22.29 → 227.36 | P2, 1950 MHz, 9501 MHz, 59 %, 8498 MiB → P2, 1950 MHz, 9501 MHz, 92 %, 8507 MiB |
| m03-paused-on-p3 | `c2cf9ab5` | 40 | 0 | 106.10 → 109.38 | python PID 2261117: 112.7 | 19.15 → 222.86 | P2, 1950 MHz, 9501 MHz, 72 %, 8507 MiB → P2, 1950 MHz, 9501 MHz, 89 %, 8503 MiB |
| m03-paused-on-p4 | `c2cf9ab5` | 40 | 0 | 109.38 → 112.40 | python PID 2261117: 114.7 | 19.84 → 228.31 | P2, 1950 MHz, 9501 MHz, 77 %, 8503 MiB → P2, 1950 MHz, 9501 MHz, 76 %, 8500 MiB |
| m03-play-on-p0 | `c2cf9ab5` | 0 | 0; audio output stream failed | 112.40 → 106.71 | rustc PID 1792262: 151.0 | 236.57 → 188.11 | P2, 1950 MHz, 9501 MHz, 71 %, 8500 MiB → P2, 1950 MHz, 9501 MHz, 89 %, 8300 MiB |
| m03-play-on-p1 | `c2cf9ab5` | 40 | 0 | 106.71 → 110.83 | rustc PID 1852595: 259.1 | 199.87 → 271.45 | P2, 1950 MHz, 9501 MHz, 63 %, 8310 MiB → P2, 1950 MHz, 9501 MHz, 63 %, 8253 MiB |
| m03-play-on-p2 | `c2cf9ab5` | 40 | 0 | 110.83 → 110.37 | rustc PID 408204: 252.2 | 192.26 → 302.20 | P2, 1950 MHz, 9501 MHz, 65 %, 8253 MiB → P2, 1950 MHz, 9501 MHz, 91 %, 8479 MiB |
| m03-play-on-p3 | `c2cf9ab5` | 40 | 0 | 110.37 → 103.75 | python PID 2261117: 115.3 | 194.73 → 304.81 | P2, 1950 MHz, 9501 MHz, 68 %, 8479 MiB → P2, 1950 MHz, 9501 MHz, 57 %, 8471 MiB |
| m03-play-on-p4 | `c2cf9ab5` | 40 | 0 | 102.65 → 97.90 | python PID 2261117: 115.0 | 252.55 → 301.00 | P2, 1950 MHz, 9501 MHz, 87 %, 8471 MiB → P2, 1950 MHz, 9501 MHz, 70 %, 8300 MiB |
| m05-paused-on-p0 | `c2cf9ab5` | 40 | 0 | 106.89 → 107.42 | python PID 2261117: 114.7 | 15.09 → 237.96 | P2, 1950 MHz, 9501 MHz, 66 %, 8488 MiB → P2, 1950 MHz, 9501 MHz, 59 %, 8277 MiB |

The deliberately stopped original software M5 paused p1 receives no credit; `target/po0-probe/interrupted-for-r5-fix.json` records that interruption. An absent end/report is not a zero result.

## Separate final captures

These capture processes use C3 and are outside the acceptance matrix.

### m03

| Run | Source | Reps | Exit / errors | Load 1m start → end | Other CPU peak (one core %) | Own CPU static → active (%) | GPU start → end |
| --- | --- | ---: | --- | --- | --- | --- | --- |
| m03-play-on-p0 | `60e8befc` | NOT RUN | 0 | 97.56 → 106.39 | python PID 3057186: 119.6 | NOT RUN | P2, 1950 MHz, 9501 MHz, 100 %, 15733 MiB → P2, 1950 MHz, 9501 MHz, 100 %, 15767 MiB |

### m03-active

| Run | Source | Reps | Exit / errors | Load 1m start → end | Other CPU peak (one core %) | Own CPU static → active (%) | GPU start → end |
| --- | --- | ---: | --- | --- | --- | --- | --- |
| m03-play-on-p0 | `60e8befc` | NOT RUN | -6 | 105.39 → 105.47 | rustc PID 3200438: 205.5 | NOT RUN | P2, 1950 MHz, 9501 MHz, 32 %, 15754 MiB → P2, 1950 MHz, 9501 MHz, 85 %, 15787 MiB |

### m06

| Run | Source | Reps | Exit / errors | Load 1m start → end | Other CPU peak (one core %) | Own CPU static → active (%) | GPU start → end |
| --- | --- | ---: | --- | --- | --- | --- | --- |
| m06-paused-on-p0 | `60e8befc` | NOT RUN | 0 | 107.40 → 104.48 | python PID 3057186: 114.1 | NOT RUN | P2, 1950 MHz, 9501 MHz, 38 %, 15767 MiB → P2, 1950 MHz, 9501 MHz, 88 %, 15769 MiB |

### m10

| Run | Source | Reps | Exit / errors | Load 1m start → end | Other CPU peak (one core %) | Own CPU static → active (%) | GPU start → end |
| --- | --- | ---: | --- | --- | --- | --- | --- |
| m10-play-on-p0 | `60e8befc` | NOT RUN | 0 | 104.48 → 108.30 | python PID 3057186: 117.1 | NOT RUN | P2, 1950 MHz, 9501 MHz, 100 %, 15769 MiB → P2, 1950 MHz, 9501 MHz, 99 %, 15769 MiB |

### panel-primary

| Run | Source | Reps | Exit / errors | Load 1m start → end | Other CPU peak (one core %) | Own CPU static → active (%) | GPU start → end |
| --- | --- | ---: | --- | --- | --- | --- | --- |
| m01-paused-on-p0 | `60e8befc` | 3 | 0 | 108.30 → 111.64 | python PID 3057186: 117.6 | 2.00 → 88.48 | P2, 1950 MHz, 9501 MHz, 62 %, 15769 MiB → P2, 1950 MHz, 9501 MHz, 100 %, 15789 MiB |

### panel-snap520

| Run | Source | Reps | Exit / errors | Load 1m start → end | Other CPU peak (one core %) | Own CPU static → active (%) | GPU start → end |
| --- | --- | ---: | --- | --- | --- | --- | --- |
| m01-paused-on-p0-snap520 | `60e8befc` | 3 | 0 | 111.64 → 111.27 | python PID 3057186: 118.4 | 1.33 → 87.11 | P2, 1950 MHz, 9501 MHz, 39 %, 15789 MiB → P2, 1950 MHz, 9501 MHz, 100 %, 15789 MiB |
