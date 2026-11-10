#!/usr/bin/env python3
"""B2b-0 sealed stdin and launch-shape negative mutations."""
import importlib.util, subprocess, os
from pathlib import Path
ROOT=Path(__file__).resolve().parent.parent
spec=importlib.util.spec_from_file_location('b2b0_pairs',ROOT/'scripts/te15-b2a-mutations.py')
engine=importlib.util.module_from_spec(spec);spec.loader.exec_module(engine)
engine.OUT=Path(os.environ.get('KINEWRIGHT_TE15_B2B0_OUT',ROOT/'target/te1-5/b2b-0/new-pairs'))
INPUT='crates/kinewright-agent/src/job_guardian/input.rs'
PROTO='crates/kinewright-agent/src/job_guardian/protocol.rs'
LAUNCH='crates/kinewright-agent/src/orchestration/native_launcher.rs'
GUARD='crates/kinewright-agent/src/job_guardian/linux.rs'
UNIT='job_guardian::input::tests::te15_b2b0_sealed_readonly_exact_and_gone'
LONG='writer_host::hooks_tests::tests::te15_b2b0_long_feedback_exact_stdin'
LIVE='writer_host::hooks_tests::tests::te15_b2b0_guardian_long_input_and_stop'
engine.PAIRS=[
 ('digest',INPUT,'kinewright_hash::sha256_bytes(&bytes) != binding.sha256','false',UNIT,'B2B0_DIGEST_REFUSED'),
 ('length',INPUT,[('file.metadata()?.len() != binding.byte_length','false'),('bytes.len() as u64 != binding.byte_length','false')],None,UNIT,'B2B0_TRUNCATED_REFUSED'),
 ('seals',INPUT,'!rustix::fs::fcntl_get_seals(&file)?.contains(SEALS)','false',UNIT,'B2B0_UNSEALED_REFUSED'),
 ('read-only',INPUT,'let mut file = File::open(path)?;','let mut file = std::fs::OpenOptions::new().read(true).write(true).open(path)?;',UNIT,'B2B0_READONLY'),
 ('offset',INPUT,'    file.rewind()?;','    // mutant omits rewind',UNIT,'B2B0_ZERO_OFFSET'),
 ('input-budget',PROTO,'pub const INPUT_BYTES: usize = 256 * 1024;','pub const INPUT_BYTES: usize = 512 * 1024;',UNIT,'B2B0_INPUT_LIMIT'),
 ('leaked-input',INPUT,'    open(&source(&file), binding)','    let result=open(&source(&file), binding); std::mem::forget(file); result',UNIT,'B2B0_NO_INPUT_HANDLES'),
 ('metadata',LAUNCH,'"s4b-codex".into(),\n            "-".into(),','"s4b-codex".into(),\n            prompt.into(),',LONG,'provider effective launch context changed'),
 ('codex-mode',LAUNCH,'arguments.push("-".into());','arguments.push("fixed positional prompt".into());','te15_b2a_case1_scripted_studio_end_to_end','B2B0_CODEX_STDIN_MODE'),
 ('claude-mode',LAUNCH,'    Ok(arguments)','    arguments.extend(["--".into(), "changed prompt".into()]); Ok(arguments)','writer_host::hooks_tests::tests::s4b_c6_claude_argv_and_environment_are_isolated','C6_STDIN_NO_POSITIONAL_PROMPT'),
 ('capacity-words','crates/kinewright-agent/src/writer_host/provider_readiness.rs','detail.code == \"feedback_too_long\"','detail.code == \"mutant_not_capacity\"','writer_host::hooks_tests::tests::te15_b2b0_oversize_pump_never_launches_or_retries','B2B0_PUMP_TRUE_WORDS'),
 ('guardian-input',GUARD,'    command.stdin(stdin.unwrap_or_else(Stdio::null));','    drop(stdin);',LIVE,'B2B0_CHILD_EXACT_BYTES'),
]
original_run=engine.run
def run(label,witness,marker,red):
    if witness==LIVE:
        with (engine.OUT/(label+'-build.log')).open('w') as log:
            subprocess.run(['bash','-c','source scripts/setup-ffmpeg.sh && exec flock -o /tmp/kinewright-cargo-heavy.lock env CARGO_BUILD_JOBS=4 RUST_TEST_THREADS=4 nice -n19 cargo build -p kinewright-agent --bin kinewright-job-guardian'],cwd=ROOT,stdout=log,stderr=subprocess.STDOUT,check=True)
    return original_run(label,witness,marker,red)
engine.run=run
if __name__=='__main__':engine.main()
