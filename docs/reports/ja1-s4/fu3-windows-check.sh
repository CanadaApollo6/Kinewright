#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/../../.."
source scripts/setup-ffmpeg.sh >/dev/null
nice -n 19 python3 -I - <<'PY'
from pathlib import Path
import tomllib
root = Path.cwd()
check = root / 'target/fu3-windows-check'
media = check / 'media'
media.mkdir(parents=True, exist_ok=True)
versions = tomllib.loads((root / 'Cargo.toml').read_text())['workspace']['dependencies']

def dependency(name):
    value = versions[name]
    if isinstance(value, str):
        return f'{name} = "{value}"\n'
    features = ', '.join(f'"{feature}"' for feature in value.get('features', []))
    return f'{name} = {{ version = "{value["version"]}", features = [{features}] }}\n'

manifest = '''[package]
name = "kinewright-project"
version = "0.1.0"
edition = "2024"
[workspace]
members = ["media"]
[features]
test-util = ["kinewright-media/test-util"]
storage-heap-profile = []
[dependencies]
'''
manifest += ''.join(dependency(name) for name in ['crossbeam-channel', 'fs2', 'gethostname', 'getrandom', 'serde', 'serde_json'])
manifest += f'kinewright-core = {{ path = "{root}/crates/kinewright-core" }}\n'
manifest += 'kinewright-media = { path = "media" }\n'
manifest += '''[target.'cfg(windows)'.dependencies]
winapi-util = "=0.1.11"
[dev-dependencies]
dhat = "=0.3.3"
'''
manifest += f'[lib]\npath = "{root}/crates/kinewright-project/src/lib.rs"\n'
for test in sorted((root / 'crates/kinewright-project/tests').glob('*.rs')):
    manifest += f'[[test]]\nname = "{test.stem}"\npath = "{test}"\n'
for example in sorted((root / 'crates/kinewright-project/examples').glob('*.rs')):
    if example.stem in {'m4_verify', 'performance_workloads'}:
        continue
    manifest += f'[[example]]\nname = "{example.stem}"\npath = "{example}"\n'
    if example.stem == 'ja_s1_storage_budget':
        manifest += 'required-features = ["test-util"]\n'
manifest += '''[lints.rust]
unsafe_code = "forbid"
unexpected_cfgs = { level = "warn", check-cfg = ['cfg(kani)'] }
[lints.clippy]
all = { level = "warn", priority = -1 }
pedantic = { level = "warn", priority = -1 }
assert_is_empty = "allow"
'''
(check / 'Cargo.toml').write_text(manifest)
(media / 'Cargo.toml').write_text(f'''[package]
name = "kinewright-media"
version = "0.1.0"
edition = "2024"
[features]
test-util = ["kinewright-core/test-util"]
[dependencies]
kinewright-core = {{ path = "{root}/crates/kinewright-core" }}
kinewright-hash = {{ path = "{root}/crates/kinewright-hash" }}
''')
(media / 'src').mkdir(exist_ok=True)
library = ''
for module in ['lut', 'builtin_looks', 'lut_store', 'sha256']:
    library += f'#[path = "{root}/crates/kinewright-media/src/{module}.rs"]\npub mod {module};\n'
library += 'pub use lut_store::*;\npub use sha256::{sha256_bytes, sha256_file, sha256_reader};\npub mod test_support;\n'
(media / 'src/lib.rs').write_text(library)
support = (root / 'crates/kinewright-media/src/test_support.rs').read_text()
start = support.index('pub struct TempDirectory')
end = support.index('pub struct GeneratedMedia')
unique_start = support.index('fn unique_stem(')
unique_end = support.index('\n}\n', unique_start) + 3
pure_support = 'use std::{fs, path::{Path, PathBuf}, time::{SystemTime, UNIX_EPOCH}};\n'
pure_support += support[start:end] + support[unique_start:unique_end]
(media / 'src/test_support.rs').write_text(pure_support)
(check / 'Cargo.lock').write_bytes((root / 'Cargo.lock').read_bytes())
PY
test "$(awk '/MemAvailable:/ {print $2}' /proc/meminfo)" -ge 6291456
flock -o /tmp/kinewright-cargo-heavy.lock env CARGO_BUILD_JOBS=4 nice -n 10 cargo clippy --manifest-path "$PWD/target/fu3-windows-check/Cargo.toml" --all-targets --features test-util --target x86_64-pc-windows-msvc -- -D warnings
