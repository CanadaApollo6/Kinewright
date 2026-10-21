# S0 evidence archives

The five raw-log archives listed in `evidence-index.json` (`S0-2/3/4/5-evidence.tar.gz` and
`verification-logs.tar.gz`, about 8 MB together) are kept out of Git so the repository does not
carry build and test logs. The lead keeps them locally under `target/review/ja/s0-evidence/`,
with a `SHA256SUMS` file. The byte lengths and SHA-256 values in `evidence-index.json` are the
authority, so any copy can be checked against them.
