# M39 token regression report

`artifact.json` is the canonical report manifest and bounded data snapshot.
`report.html` is the validated, self-contained reader artifact.

The portable reader bundled with Data Analytics 0.2.8 uses `100vw` for its
top bar, which overflows by the vertical scrollbar width on long reports. The
generated HTML includes a report-local `.analytics-top-bar { width: 100% }`
override. Both 1440-pixel desktop and 390-pixel mobile verification passed
after that mechanical packaging correction.

The underlying measurements and trace identities are checked in at
[`benchmarks/auto-edit/v4/token-regression.json`](../../../benchmarks/auto-edit/v4/token-regression.json).
