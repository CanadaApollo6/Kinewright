#!/usr/bin/env python3
"""Source admission must fail before extraction, configure, or install replacement."""
import importlib.util
import hashlib
import io
from pathlib import Path
import struct
import tarfile
import tempfile
import textwrap
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location('ffmpeg_builder', Path(__file__).with_name('build-ffmpeg.py'))
builder = importlib.util.module_from_spec(spec)
spec.loader.exec_module(builder)

inspect_spec = importlib.util.spec_from_file_location('ffmpeg_inspector', Path(__file__).with_name('inspect-ffmpeg.py'))
inspector = importlib.util.module_from_spec(inspect_spec)
inspect_spec.loader.exec_module(inspector)


def profile_component_sources(omit=()):
    """Minimal upstream component lists declaring every name all profiles enable."""
    files = {}
    for profile in ['libraries', 'recorder', 'testgen']:
        for system in ['Linux', 'Windows']:
            for argument in builder.configure(profile, system):
                kind, _, names = argument.removeprefix('--enable-').partition('=')
                if not names or kind not in [*builder.COMPONENT_SOURCES, 'filter']:
                    continue
                for name in names.split(','):
                    if (kind, name) in omit:
                        continue
                    if kind == 'filter':
                        path, line = 'libavfilter/allfilters.c', f'extern const FFFilter ff_vf_{name};'
                    else:
                        path, suffix = builder.COMPONENT_SOURCES[kind]
                        line = f'extern const FFCodec ff_{name}_{suffix};'
                    files.setdefault(path, set()).add(line)
    for path, _ in builder.COMPONENT_SOURCES.values():
        files.setdefault(path, set())
    files.setdefault('libavfilter/allfilters.c', set())
    return {path: '\n'.join(sorted(lines)) + '\n' for path, lines in files.items()}


def add_source_files(source, files):
    for path, text in files.items():
        entry = tarfile.TarInfo(f'ffmpeg-{builder.PROFILE["version"]}/{path}')
        entry.size = len(text.encode())
        source.addfile(entry, io.BytesIO(text.encode()))


class SourceAdmission(unittest.TestCase):
    @staticmethod
    def lavformat_cbs_state(args):
        # FFmpeg 8.0.3 built libavformat cbs.o for MOV (also selected by HLS) with an
        # empty type table unless AV1/APV was selected. 9.0.2's mov_muxer selects
        # cbs_av1_lavf and cbs_apv_lavf itself; libavformat/cbs.h still masks every
        # other CBS type, so this stays the conservative check.
        def enabled(kind):
            names = set()
            for arg in args:
                if arg == f'--enable-{kind}s':
                    names.add('*')
                elif arg.startswith(f'--enable-{kind}='):
                    names.update(arg.split('=', 1)[1].split(','))
            return names

        muxers = enabled('muxer')
        needed = bool(muxers & {'*', 'mov', 'mp4', 'ipod', 'avif', 'f4v', 'hls', 'ismv', 'psp', 'tg2', 'tgp'})
        # configure *_select dependencies that can populate that restricted table.
        selectors = {
            'av1': {'decoder': {'av1'}, 'parser': {'av1'},
                    'encoder': {'av1_vaapi', 'av1_vulkan'},
                    'bsf': {'av1_frame_merge', 'av1_frame_split', 'av1_metadata', 'dovi_rpu'}},
            'apv': {'decoder': {'apv'}, 'parser': {'apv'}},
        }
        types = {codec for codec, kinds in selectors.items()
                 if any(enabled(kind) & (names | {'*'}) for kind, names in kinds.items())}
        return needed, types

    def test_profiles_supply_a_type_for_libavformat_cbs(self):
        for system in ['Linux', 'Windows']:
            with patch.object(builder.platform, 'system', return_value=system):
                for profile in ['libraries', 'recorder', 'testgen']:
                    with self.subTest(system=system, profile=profile):
                        needed, types = self.lavformat_cbs_state(builder.configure(profile))
                        self.assertTrue(not needed or types,
                                        f'{profile}: MOV requires an AV1/APV CBS type; other CBS types are masked')

    def test_cbs_check_catches_missing_av1_even_with_vp9_enabled(self):
        args = builder.configure('libraries')
        args = [arg.replace('h264,hevc,av1,vp9', 'h264,hevc,vp9') for arg in args]
        self.assertEqual(self.lavformat_cbs_state(args), (True, set()))
        self.assertEqual(self.lavformat_cbs_state(['--enable-muxer=mp4', '--enable-bsf=h264_metadata']),
                         (True, set()))
        self.assertEqual(self.lavformat_cbs_state(['--enable-muxer=hls']), (True, set()))
        self.assertEqual(self.lavformat_cbs_state(['--enable-muxer=mov', '--enable-parser=apv']),
                         (True, {'apv'}))
        self.assertEqual(self.lavformat_cbs_state(['--enable-muxer=wav']), (False, set()))

    def test_profile_naming_an_absent_component_is_refused(self):
        files = profile_component_sources()
        builder.check_components(files.__getitem__)
        files = profile_component_sources(omit={('decoder', 'h264'), ('indev', 'gdigrab')})
        with self.assertRaises(SystemExit) as refused:
            builder.check_components(files.__getitem__)
        message = str(refused.exception)
        for entry in ['libraries: decoder h264', 'testgen: decoder h264', 'recorder: indev gdigrab']:
            self.assertIn(entry, message)

    def test_unconditional_buffer_filters_count_by_their_registered_names(self):
        files = profile_component_sources(omit={('filter', name) for name in
                                                ['buffer', 'buffersink', 'abuffer', 'abuffersink']})
        with self.assertRaisesRegex(SystemExit, 'filter buffersink'):
            builder.check_components(files.__getitem__)
        files['libavfilter/allfilters.c'] += (
            'extern  const FFFilter ff_asrc_abuffer;\nextern  const FFFilter ff_vsrc_buffer;\n'
            'extern  const FFFilter ff_asink_abuffer;\nextern  const FFFilter ff_vsink_buffer;\n')
        builder.check_components(files.__getitem__)

    def test_build_refuses_a_missing_component_before_configure(self):
        with tempfile.TemporaryDirectory(dir=builder.ROOT/'target') as directory:
            root = Path(directory)
            archive = root/'source.tar.xz'
            with tarfile.open(archive, 'w:xz') as source:
                add_source_files(source, profile_component_sources(omit={('demuxer', 'mov')}))
            profile = dict(builder.PROFILE, sha256=builder.file_digest(archive))
            with patch.object(builder, 'ROOT', root), \
                 patch.object(builder, 'PROFILE', profile), \
                 patch.object(builder, 'ensure_nasm'), \
                 patch.object(builder, 'fingerprint', return_value=('test-key', {})), \
                 patch.object(builder, 'verified_source', return_value=archive), \
                 patch.object(builder, 'run') as run:
                with self.assertRaisesRegex(SystemExit, 'libraries: demuxer mov'):
                    builder.build('libraries')
                run.assert_not_called()
            self.assertFalse((root/'third_party/ffmpeg').exists())

    def test_pinned_source_has_every_profile_component(self):
        archive = builder.ROOT/'target/ffmpeg-source'/builder.PROFILE['sha256']/'source.tar.xz'
        self.assertTrue(archive.is_file(), 'run scripts/build-ffmpeg.py --fetch-source first')
        builder.verify_source(archive)
        source, read = builder.archive_reader(archive)
        with source:
            builder.check_components(read)
            components = builder.source_components(read)
        # A vacuous reader would pass every profile; the pin has the full upstream lists.
        self.assertGreater(len(components['decoder']), 500)
        self.assertGreater(len(components['demuxer']), 300)

    def test_bad_source_never_executes_or_replaces_an_install(self):
        scratch = builder.ROOT / 'target'
        scratch.mkdir(parents=True, exist_ok=True)
        with tempfile.TemporaryDirectory(dir=scratch) as directory:
            root = Path(directory)
            install = root / 'third_party/ffmpeg'
            install.mkdir(parents=True)
            sentinel = install / 'keep'
            sentinel.write_bytes(b'previous verified install')
            with patch.object(builder, 'ROOT', root), \
                 patch.object(builder, 'ensure_nasm'), \
                 patch.object(builder, 'fingerprint', return_value=('test-key', {})), \
                 patch.object(builder.urllib.request, 'urlopen', return_value=io.BytesIO(b'unverified input')), \
                 patch.object(builder, 'run') as run, \
                 patch.object(builder.tarfile, 'open') as extract:
                with self.assertRaisesRegex(SystemExit, 'checksum mismatch'):
                    builder.build('libraries')
                run.assert_not_called()
                extract.assert_not_called()
            self.assertEqual(sentinel.read_bytes(), b'previous verified install')
            self.assertEqual(list((root / 'target/ffmpeg-build').iterdir()), [])

    def test_restored_bad_source_never_extracts_or_replaces_an_install(self):
        with tempfile.TemporaryDirectory(dir=builder.ROOT/'target') as directory:
            root = Path(directory)
            install = root/'third_party/ffmpeg'
            install.mkdir(parents=True)
            sentinel = install/'keep'
            sentinel.write_bytes(b'previous verified install')
            cache = root/'target/ffmpeg-source'/builder.PROFILE['sha256']/'source.tar.xz'
            cache.parent.mkdir(parents=True)
            cache.write_bytes(b'unverified restored input')
            with patch.object(builder, 'ROOT', root), \
                 patch.object(builder, 'ensure_nasm'), \
                 patch.object(builder, 'fingerprint', return_value=('test-key', {})), \
                 patch.object(builder.urllib.request, 'urlopen') as download, \
                 patch.object(builder, 'run') as run, \
                 patch.object(builder.tarfile, 'open') as extract:
                with self.assertRaisesRegex(SystemExit, 'checksum mismatch'):
                    builder.build('libraries')
                download.assert_not_called()
                run.assert_not_called()
                extract.assert_not_called()
            self.assertEqual(sentinel.read_bytes(), b'previous verified install')

    def test_recorder_has_no_file_demuxers_or_test_generators(self):
        args = builder.configure('recorder')
        self.assertFalse(any(a.startswith('--enable-demuxer') for a in args))
        self.assertFalse(any('lavfi' in a for a in args))
        self.assertFalse(any(a.startswith('--enable-parser') for a in args))
        self.assertIn('--disable-network', args)
        self.assertIn('--disable-autodetect', args)
        self.assertIn('--disable-ffprobe', args)
        self.assertIn('--enable-protocol=pipe', builder.configure('testgen'))
        self.assertNotIn('--enable-protocol=pipe', args)
        with patch.object(builder.platform, 'system', return_value='Windows'):
            windows = builder.configure('recorder')
        enabled = next(a for a in windows if a.startswith('--enable-decoder='))
        self.assertIn('bmp', enabled.split('=', 1)[1].split(','))


class SourceDownload(unittest.TestCase):
    def setUp(self):
        directory = tempfile.TemporaryDirectory(dir=builder.ROOT/'target')
        self.addCleanup(directory.cleanup)
        self.root = Path(directory.name)
        self.payload = b'verified source fixture'
        profile = dict(builder.PROFILE, sha256=hashlib.sha256(self.payload).hexdigest())
        self.enterContext(patch.object(builder, 'ROOT', self.root))
        self.enterContext(patch.object(builder, 'PROFILE', profile))

    def test_timeouts_retry_three_attempts_then_reuse_the_verified_cache(self):
        with patch.object(builder.urllib.request, 'urlopen', side_effect=[
                builder.urllib.error.URLError(TimeoutError('connection timed out')),
                TimeoutError('socket timed out'), io.BytesIO(self.payload)]) as download, \
             patch.object(builder.time, 'sleep') as sleep:
            archive = builder.verified_source()
            self.assertEqual(archive.read_bytes(), self.payload)
            self.assertEqual(archive.relative_to(self.root).as_posix(),
                             f'target/ffmpeg-source/{builder.PROFILE["sha256"]}/source.tar.xz')
            self.assertEqual(builder.verified_source(), archive)
            archive.write_bytes(b'poisoned restored cache')
            with self.assertRaisesRegex(SystemExit, 'checksum mismatch'):
                builder.verified_source()
        self.assertEqual(download.call_count, 3)
        self.assertEqual([call.args for call in sleep.call_args_list], [(2,), (4,)])
        for call in download.call_args_list:
            self.assertEqual(call.args, ('https://ffmpeg.org/releases/ffmpeg-9.0.2.tar.xz',))
            self.assertEqual(call.kwargs, {'timeout': 30})
        self.assertEqual(list((self.root/'target/ffmpeg-source').iterdir()), [archive.parent])

    def test_interrupted_download_is_discarded_before_retry(self):
        class Interrupted(io.BytesIO):
            def read(self, size=-1):
                if self.tell():
                    raise TimeoutError('interrupted body')
                return super().read(size)
        with patch.object(builder.urllib.request, 'urlopen', side_effect=[
                Interrupted(b'partial response'), io.BytesIO(self.payload)]) as download, \
             patch.object(builder.time, 'sleep') as sleep:
            archive = builder.verified_source()
        self.assertEqual(archive.read_bytes(), self.payload)
        self.assertEqual(download.call_count, 2)
        sleep.assert_called_once_with(2)
        self.assertEqual(list((self.root/'target/ffmpeg-source').iterdir()), [archive.parent])

    def test_retry_exhaustion_publishes_no_partial_cache(self):
        with patch.object(builder.urllib.request, 'urlopen', side_effect=builder.urllib.error.URLError('timeout')) as download, \
             patch.object(builder.time, 'sleep') as sleep:
            with self.assertRaises(builder.urllib.error.URLError):
                builder.verified_source()
        self.assertEqual(download.call_count, 3)
        self.assertEqual([call.args for call in sleep.call_args_list], [(2,), (4,)])
        self.assertEqual(list((self.root/'target/ffmpeg-source').iterdir()), [])


class BuildIdentity(unittest.TestCase):
    def test_windows_shared_import_libraries_match_setup_and_survive_recorder_install(self):
        with tempfile.TemporaryDirectory(dir=builder.ROOT/'target') as directory:
            root = Path(directory)
            archive = root/'source.tar.xz'
            license_name = f'ffmpeg-{builder.PROFILE["version"]}/COPYING.GPLv3'
            with tarfile.open(archive, 'w:xz') as source:
                entry = tarfile.TarInfo(license_name)
                entry.size = len(b'test license')
                source.addfile(entry, io.BytesIO(b'test license'))
                add_source_files(source, profile_component_sources())
            profile = dict(builder.PROFILE, sha256=builder.file_digest(archive))
            runtime = root/'third_party/ffmpeg'
            recorder = root/'third_party/ffmpeg-recorder'
            names = ['avcodec', 'avformat', 'avutil', 'avfilter', 'swscale', 'swresample']
            def install(command, _cwd, heavy=False):
                if command[0] != 'make':
                    return
                self.assertTrue(heavy)
                if command[-1] == 'install':
                    (runtime/'bin').mkdir(parents=True)
                    # FFmpeg's MSVC install rule puts import libraries in SHLIBDIR=bin.
                    for name in names:
                        (runtime/'bin'/f'{name}.lib').write_bytes(name.encode())
                    (runtime/'bin/avcodec-63.dll').write_bytes(b'runtime DLL')
                else:
                    self.assertEqual(command[-1], 'install-progs')
                    (recorder/'bin').mkdir(parents=True)
                    (recorder/'bin/ffmpeg.exe').write_bytes(b'static recorder')
            with patch.object(builder, 'ROOT', root), \
                 patch.object(builder, 'PROFILE', profile), \
                 patch.object(builder.platform, 'system', return_value='Windows'), \
                 patch.dict(builder.os.environ), \
                 patch.object(builder, 'ensure_nasm'), \
                 patch.object(builder, 'fingerprint', return_value=('test-key', {})), \
                 patch.object(builder, 'verified_source', return_value=archive), \
                 patch.object(builder, 'run', side_effect=install):
                builder.build('libraries')
                builder.build('recorder')
            for name in names:
                self.assertTrue((runtime/'lib'/f'{name}.lib').is_file(), f'Required import library missing: {name}.lib')
                self.assertEqual((runtime/'lib'/f'{name}.lib').read_bytes(), name.encode())
                self.assertFalse((runtime/'bin'/f'{name}.lib').exists())
            self.assertEqual((runtime/'bin/avcodec-63.dll').read_bytes(), b'runtime DLL')
            self.assertEqual((recorder/'bin/ffmpeg.exe').read_bytes(), b'static recorder')
            self.assertFalse((recorder/'lib').exists())

    def test_linker_query_nonce_is_not_part_of_the_tool_identity(self):
        from types import SimpleNamespace
        with patch.object(builder.shutil, 'which', return_value='/tool/gcc'), \
             patch.object(builder, 'file_digest', return_value='compiler-digest'), \
             patch.object(builder.subprocess, 'run') as query:
            query.return_value = SimpleNamespace(stdout='gcc 15.2 -plugin-opt=-fresolution=/tmp/ccAAA.res GNU ld 2.45')
            first = builder.tool_identity(['gcc'], ['-Wl,--version'])
            query.return_value = SimpleNamespace(stdout='gcc 15.2 -plugin-opt=-fresolution=/tmp/ccBBB.res GNU ld 2.45')
            self.assertEqual(builder.tool_identity(['gcc'], ['-Wl,--version']), first)
            query.return_value = SimpleNamespace(stdout='gcc 15.2 -plugin-opt=-fresolution=/tmp/ccBBB.res GNU ld 2.46')
            self.assertNotEqual(builder.tool_identity(['gcc'], ['-Wl,--version']), first)

    def test_ci_and_local_use_the_same_fingerprint_and_source_only_ignores_it(self):
        with tempfile.TemporaryDirectory(dir=builder.ROOT/'target') as directory:
            root = Path(directory)
            prefix = root/'third_party/ffmpeg'
            prefix.mkdir(parents=True)
            (prefix/'.build-key').write_text('shared-fingerprint')
            with patch.object(builder, 'ROOT', root), \
                 patch.object(builder, 'fingerprint', return_value=('shared-fingerprint', {})), \
                 patch.object(builder.urllib.request, 'urlopen', side_effect=RuntimeError('fresh verified-source download')) as download:
                builder.build('libraries')
                download.assert_not_called()
                with self.assertRaisesRegex(RuntimeError, 'fresh verified-source download'):
                    builder.build('libraries', no_binary_cache=True)
                download.assert_called_once()

    def test_ci_consumer_reuses_only_an_exactly_restored_producer_cache(self):
        import os
        with tempfile.TemporaryDirectory(dir=builder.ROOT/'target') as directory:
            root = Path(directory)
            prefix = root/'third_party/ffmpeg'
            prefix.mkdir(parents=True)
            (prefix/'.build-key').write_text('key-written-by-the-producer-build')
            with patch.object(builder, 'ROOT', root), \
                 patch.object(builder, 'fingerprint', return_value=('newer-image-key', {})), \
                 patch.object(builder.urllib.request, 'urlopen', side_effect=RuntimeError('fresh verified-source download')) as download:
                with patch.dict(os.environ, {'KINEWRIGHT_FFMPEG_RESTORED_PRODUCER_CACHE': 'true'}):
                    builder.build('libraries')
                    download.assert_not_called()
                    with self.assertRaisesRegex(RuntimeError, 'fresh verified-source download'):
                        builder.build('libraries', no_binary_cache=True)
                    (prefix/'.build-key').write_text('')
                    with self.assertRaisesRegex(RuntimeError, 'fresh verified-source download'):
                        builder.build('libraries')
                    (prefix/'.build-key').write_text('key-written-by-the-producer-build')
                for other in ['false', '', 'True', '1']:
                    with patch.dict(os.environ, {'KINEWRIGHT_FFMPEG_RESTORED_PRODUCER_CACHE': other}):
                        with self.assertRaisesRegex(RuntimeError, 'fresh verified-source download'):
                            builder.build('libraries')

    def test_ci_consumers_take_the_restore_flag_from_the_exact_cache_hit_for_the_whole_job(self):
        action = (builder.ROOT/'.github/actions/ffmpeg/action.yml').read_text()
        mark = action[action.index('    - name: Mark the exactly restored producer binaries'):action.index('    - name: Consumer Windows dependencies')]
        self.assertIn("if: inputs.producer != 'true' && steps.cache.outputs.cache-hit == 'true'", mark)
        self.assertIn('''echo 'KINEWRIGHT_FFMPEG_RESTORED_PRODUCER_CACHE=true' >> "$GITHUB_ENV"''', mark)
        # Later steps call the setup scripts again. A step-level value would be lost
        # there, and a consumer on a newer runner image would rebuild.
        self.assertEqual(action.count('KINEWRIGHT_FFMPEG_RESTORED_PRODUCER_CACHE'), 1)
        self.assertLess(action.index('    - name: Restore trusted same-repository binaries'), action.index(mark))
        for name in ['ci.yml', 'ci-tests.yml', 'release.yml', 'ffmpeg.yml']:
            self.assertNotIn('KINEWRIGHT_FFMPEG_RESTORED_PRODUCER_CACHE', (builder.ROOT/'.github/workflows'/name).read_text())

    def test_fingerprint_changes_with_tool_dependency_flag_or_setup_changes(self):
        from types import SimpleNamespace
        import os
        def query(command, **_kwargs):
            return SimpleNamespace(returncode=0, stdout='1.0')
        with patch.object(builder, 'ensure_nasm'), \
             patch.object(builder, 'tool_identity', return_value={'version': 'tool-1'}) as tool, \
             patch.object(builder.subprocess, 'check_output', return_value='ld'), \
             patch.object(builder.subprocess, 'run', side_effect=query) as dependency:
            baseline, _ = builder.fingerprint()
            tool.return_value = {'version': 'tool-2'}
            self.assertNotEqual(builder.fingerprint()[0], baseline)
            tool.return_value = {'version': 'tool-1'}
            dependency.side_effect = lambda command, **kwargs: SimpleNamespace(returncode=0, stdout='2.0' if '--modversion' in command and command[-1]=='x264' else '1.0')
            self.assertNotEqual(builder.fingerprint()[0], baseline)
            dependency.side_effect = query
            with patch.dict(os.environ, {'CFLAGS': '-DMS1_FINGERPRINT_WITNESS'}):
                self.assertNotEqual(builder.fingerprint()[0], baseline)
            actual_digest = builder.file_digest
            with patch.object(builder, 'file_digest', side_effect=lambda path: 'changed' if str(path).endswith('setup-ffmpeg.sh') else actual_digest(path)):
                self.assertNotEqual(builder.fingerprint()[0], baseline)

    def test_windows_pins_msys_make_and_shell_even_when_host_make_is_first(self):
        import os
        # The default root is the subject here; the CI root has its own witness below.
        with patch.object(builder.platform, 'system', return_value='Windows'), \
             patch.dict(os.environ), \
             patch.object(builder.subprocess, 'run') as run:
            os.environ.pop('KINEWRIGHT_MSYS_ROOT', None)
            builder.run(['make', '-j4', 'install'], builder.ROOT)
        command = run.call_args.args[0]
        self.assertEqual(command[0].replace('\\', '/'), 'C:/msys64/usr/bin/make.exe')
        self.assertEqual(command[1:], ['SHELL=/usr/bin/sh', '-j4', 'install'])
        self.assertNotIn('shell', run.call_args.kwargs)

    def test_ci_uses_the_action_installation_instead_of_the_runner_preinstall(self):
        import os
        with patch.object(builder.platform, 'system', return_value='Windows'), \
             patch.dict(os.environ, {'KINEWRIGHT_MSYS_ROOT': 'D:/a/_temp/msys64'}):
            commands = builder.tool_commands()
            self.assertEqual(commands['make'][0].replace('\\', '/'), 'D:/a/_temp/msys64/usr/bin/make.exe')
            self.assertEqual(commands['shell'][0].replace('\\', '/'), 'D:/a/_temp/msys64/usr/bin/sh.exe')
        workflow = (builder.ROOT/'.github/actions/ffmpeg/action.yml').read_text()
        self.assertIn('MSYS_ROOT: ${{ steps.msys.outputs.msys2-location }}', workflow)
        self.assertIn('cache: false', workflow)

    def test_depfile_backslash_round_trip(self):
        builder.depfile_shell_witness()

    def test_release_workflows_and_packagers_require_source_only_flag(self):
        release = (builder.ROOT/'.github/workflows/release.yml').read_text()
        self.assertNotIn('actions/cache', release)
        self.assertEqual(release.count("no-binary-cache: 'true'"), 2)
        self.assertIn('-NoBinaryCache', release)
        self.assertIn('--no-binary-cache', release)
        package = builder.ROOT/'scripts/package-linux.sh'
        # The Linux packager runs only on Linux; on Windows `bash` can resolve to the WSL launcher.
        if builder.platform.system() == 'Linux':
            result = builder.subprocess.run(['bash', package.as_posix(), '--version', '0.1.0', '--stage-only'],
                                            capture_output=True, text=True, check=False)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn('requires --no-binary-cache', result.stderr)
        else:
            self.assertIn('requires --no-binary-cache', package.read_text())
        windows = (builder.ROOT/'scripts/package-windows.ps1').read_text()
        self.assertIn('if (-not $NoBinaryCache)', windows)
        self.assertIn('-NoBinaryCache -SkipBuildCachePrune', windows)

    def test_native_cache_is_push_write_only_and_forks_do_not_restore(self):
        workflow = (builder.ROOT/'.github/actions/ffmpeg/action.yml').read_text()
        restore = workflow[workflow.index('    - name: Restore trusted'):workflow.index('    - name: Consumer Windows')]
        save = workflow[workflow.index('    - name: Save binaries'):]
        self.assertIn("github.event.pull_request.head.repo.full_name == github.repository", restore)
        self.assertIn("github.repository == 'CanadaApollo6/Kinewright'", restore)
        self.assertIn("github.event_name == 'push'", save)
        self.assertIn("inputs.no-binary-cache != 'true'", save)
        self.assertIn('ms1-${{ steps.fingerprint.outputs.key }}', save)
        self.assertNotIn('restore-keys', restore)
        self.assertIn("key: ms1-${{ inputs.producer == 'true' && steps.fingerprint.outputs.key || inputs.cache-key }}", restore)

    def test_vcpkg_package_cache_is_push_write_only_and_the_only_fallback_key(self):
        workflow = (builder.ROOT/'.github/actions/ffmpeg/action.yml').read_text()
        restore = workflow[workflow.index('    - name: Restore vcpkg packages'):workflow.index('    - name: Producer Windows dependencies')]
        save = workflow[workflow.index('    - name: Save vcpkg packages'):workflow.index('    - name: Producer fingerprint')]
        key = "vcpkg-archives-${{ hashFiles('scripts/prepare-ffmpeg-windows.ps1') }}-"
        for step in [restore, save]:
            self.assertIn("runner.os == 'Windows' && inputs.producer == 'true' && inputs.no-binary-cache != 'true'", step)
            self.assertIn("github.repository == 'CanadaApollo6/Kinewright'", step)
            self.assertIn('path: third_party/vcpkg-archives', step)
            self.assertIn('key: ' + key + '${{ steps.vcpkg-image.outputs.image }}', step)
        self.assertIn("github.event.pull_request.head.repo.full_name == github.repository", restore)
        self.assertIn("github.event_name == 'push'", save)
        self.assertNotIn('restore-keys', save)
        # vcpkg checks every archive against its own hash of compiler, triplet and
        # port, so an older image's set may be restored. Nothing else falls back.
        self.assertEqual(workflow.count('restore-keys'), 1)
        self.assertIn('restore-keys: |\n          ' + key + '\n', restore)
        script = (builder.ROOT/'scripts/prepare-ffmpeg-windows.ps1').read_text()
        self.assertIn("$env:VCPKG_DEFAULT_BINARY_CACHE = Join-Path $root 'third_party/vcpkg-archives'", script)

    def test_source_cache_uses_the_pin_and_verifies_restores_before_building(self):
        workflow = (builder.ROOT/'.github/actions/ffmpeg/action.yml').read_text()
        source = workflow[workflow.index('    - name: Read the pinned source digest'):workflow.index('    - name: MSYS build tools')]
        self.assertIn("profile['sha256']", source)
        code = source.split("python - <<'PY'\n", 1)[1].split('\n        PY', 1)[0]
        with tempfile.TemporaryDirectory(dir=builder.ROOT/'target') as directory:
            output = Path(directory)/'github-output'
            with patch.dict(builder.os.environ, {'GITHUB_OUTPUT': str(output)}):
                exec(compile(textwrap.dedent(code), '<source-cache-key>', 'exec'), {})
            self.assertEqual(output.read_text(), f'sha256={builder.PROFILE["sha256"]}\n')
        self.assertEqual(source.count('key: ffmpeg-source-${{ steps.source.outputs.sha256 }}'), 2)
        self.assertEqual(source.count('path: target/ffmpeg-source/${{ steps.source.outputs.sha256 }}/source.tar.xz'), 2)
        self.assertLess(source.index('actions/cache/restore@v4'), source.index('--fetch-source'))
        self.assertLess(source.index('--fetch-source'), source.index('actions/cache/save@v4'))
        save = source[source.index('    - name: Save verified source'):]
        self.assertIn("github.event_name == 'push'", save)
        self.assertNotIn('no-binary-cache', source)


def _minimal_elf64(sonames, *, include_phdrs=True, include_shdrs=True, dynamic_sh_type=6,
                   include_dynamic=True, include_load=True):
    ehdr_size = 64
    phnum = (1 if include_load else 0) + (1 if include_dynamic else 0) if include_phdrs else 0
    phoff = ehdr_size if phnum else 0
    str_off = ehdr_size + phnum * 56
    base_vaddr = 0x400000

    strtab = bytearray(b'\x00')
    offsets = []
    for name in sonames:
        offsets.append(len(strtab))
        strtab.extend(name.encode('utf-8') + b'\x00')
    dyn = bytearray()
    if include_dynamic:
        dyn.extend(struct.pack('<qQ', 5, base_vaddr + str_off))  # DT_STRTAB
        dyn.extend(struct.pack('<qQ', 10, len(strtab)))  # DT_STRSZ
        for off in offsets:
            dyn.extend(struct.pack('<qQ', 1, off))  # DT_NEEDED
        dyn.extend(struct.pack('<qQ', 0, 0))  # DT_NULL

    dyn_off = str_off + len(strtab)
    payload_end = dyn_off + len(dyn)
    shnum = 0
    shoff = 0
    shdrs = bytearray()
    if include_shdrs:
        shoff = payload_end
        shdrs.extend(struct.pack('<IIQQQQIIQQ', 0, 0, 0, 0, 0, 0, 0, 0, 0, 0))
        shdrs.extend(struct.pack('<IIQQQQIIQQ', 0, 3, 0, 0, str_off, len(strtab), 0, 0, 1, 0))
        shnum = 2
        if include_dynamic:
            shdrs.extend(struct.pack(
                '<IIQQQQIIQQ', 0, dynamic_sh_type, 0, base_vaddr + dyn_off, dyn_off, len(dyn), 1, 0, 8, 16
            ))
            shnum = 3

    phdrs = bytearray()
    if include_phdrs:
        if include_load:
            phdrs.extend(struct.pack('<IIQQQQQQ', 1, 5, 0, base_vaddr, base_vaddr, payload_end, payload_end, 0x1000))
        if include_dynamic:
            phdrs.extend(struct.pack(
                '<IIQQQQQQ', 2, 6, dyn_off, base_vaddr + dyn_off, base_vaddr + dyn_off, len(dyn), len(dyn), 8
            ))

    ehdr = bytearray(ehdr_size)
    ehdr[0:16] = b'\x7fELF\x02\x01\x01' + b'\x00' * 9
    struct.pack_into('<QQ', ehdr, 32, phoff, shoff)
    struct.pack_into('<HHHH', ehdr, 54, 56 if phnum else 0, phnum, 64 if shnum else 0, shnum)
    return bytes(ehdr + phdrs + strtab + dyn + shdrs)


def _minimal_pe64(dll_names, delay_names=(), *, image_base=0x10000000, delay_attrs=1, delay_raw_addrs=None):
    pe_offset = 0x80
    opt_size = 240
    sec_offset = pe_offset + 24 + opt_size
    rdata_raw = 0x200
    rdata_rva = 0x1000

    rdata = bytearray()
    imp_rva = rdata_rva
    imp_count = len(dll_names)
    imp_table_size = (imp_count + 1) * 20
    delay_rva = rdata_rva + imp_table_size if delay_names else 0
    delay_table_size = (len(delay_names) + 1) * 32 if delay_names else 0
    strings_rva = rdata_rva + imp_table_size + delay_table_size

    strings = bytearray()
    imp_name_rvas = []
    for name in dll_names:
        imp_name_rvas.append(strings_rva + len(strings))
        strings.extend(name.encode('ascii') + b'\x00')
    delay_name_rvas = []
    for name in delay_names:
        delay_name_rvas.append(strings_rva + len(strings))
        strings.extend(name.encode('ascii') + b'\x00')

    for name_rva in imp_name_rvas:
        rdata.extend(struct.pack('<IIIII', 0, 0, 0, name_rva, 0))
    rdata.extend(struct.pack('<IIIII', 0, 0, 0, 0, 0))
    if delay_names:
        for index, name_rva in enumerate(delay_name_rvas):
            if delay_raw_addrs is not None:
                addr = delay_raw_addrs[index]
            elif delay_attrs == 1:
                addr = name_rva
            else:
                addr = image_base + name_rva
            rdata.extend(struct.pack('<IIIIIIII', delay_attrs, addr, 0, 0, 0, 0, 0, 0))
        rdata.extend(struct.pack('<IIIIIIII', 0, 0, 0, 0, 0, 0, 0, 0))
    rdata.extend(strings)

    image = bytearray(rdata_raw + len(rdata))
    image[0:2] = b'MZ'
    struct.pack_into('<I', image, 0x3C, pe_offset)
    image[pe_offset:pe_offset + 4] = b'PE\x00\x00'
    struct.pack_into('<HHIIIHH', image, pe_offset + 4, 0x8664, 1, 0, 0, 0, opt_size, 0x2022)
    opt_offset = pe_offset + 24
    struct.pack_into('<H', image, opt_offset, 0x20B)  # PE32+
    struct.pack_into('<Q', image, opt_offset + 24, image_base)
    struct.pack_into('<I', image, opt_offset + 108, 16)  # NumberOfRvaAndSizes
    dd_offset = opt_offset + 112
    struct.pack_into('<II', image, dd_offset + 1 * 8, imp_rva, imp_table_size)
    if delay_names:
        struct.pack_into('<II', image, dd_offset + 13 * 8, delay_rva, delay_table_size)
    image[sec_offset:sec_offset + 8] = b'.rdata\x00\x00'
    struct.pack_into('<IIII', image, sec_offset + 8, len(rdata), rdata_rva, len(rdata), rdata_raw)
    image[rdata_raw:rdata_raw + len(rdata)] = rdata
    return bytes(image)


class SurfaceInspection(unittest.TestCase):
    @staticmethod
    def _matching_report(expected):
        return {
            'decoders': sorted(expected['decoders']),
            'encoders': sorted(expected['encoders']),
            'demuxers': sorted(expected['demuxers']),
            'parsers': [list(entry) for entry in sorted(expected['parsers'])],
            'muxers': sorted(expected['muxers']),
            'filters': sorted(expected['filters']),
            'bsfs': sorted(expected['bsfs']),
            'hwdevices': sorted(expected['hwdevices']),
            'hwaccels': sorted(expected['hwaccels']),
            'protocols': {
                'input': sorted(expected['protocols']['input']),
                'output': sorted(expected['protocols']['output']),
            },
            'external_libraries': sorted(expected['external_libraries']),
            'external_library_sources': {
                'linked': sorted(expected['linked_external_libraries']),
                'pkgconfig': sorted(expected['external_libraries']),
                'configured': sorted(expected['external_libraries']),
            },
            'libraries': {f'lib{stem}.so.1.0.0': 1 for stem in sorted(expected['built_libraries'])},
        }

    def test_expected_surface_declares_protocols_hwaccels_and_external_libraries(self):
        linux = inspector.expected_surface(system='Linux')
        windows = inspector.expected_surface(system='Windows')
        for surface in (linux, windows):
            self.assertEqual(surface['protocols'], {'input': {'file'}, 'output': {'file'}})
            self.assertEqual(surface['hwdevices'], set())
            self.assertEqual(surface['hwaccels'], set())
            self.assertEqual(surface['external_libraries'], {'libdav1d', 'libx264', 'libx265', 'zlib'})
            self.assertEqual(
                surface['built_libraries'],
                {'avcodec', 'avformat', 'avfilter', 'avutil', 'swscale', 'swresample'},
            )
        self.assertEqual(linux['linked_external_libraries'], {'libdav1d', 'libx264', 'libx265', 'zlib'})
        self.assertEqual(windows['linked_external_libraries'], set())

    def test_exact_set_check_names_unexpected_and_missing_in_every_category(self):
        expected = inspector.expected_surface(system='Linux')
        inspector.check(self._matching_report(expected), expected)

        unexpected_cases = [
            ('decoders', lambda r: r['decoders'].append('vp8'), 'vp8'),
            ('encoders', lambda r: r['encoders'].append('libvpx_vp9'), 'libvpx_vp9'),
            ('demuxers', lambda r: r['demuxers'].append('hls'), 'hls'),
            ('parsers', lambda r: r['parsers'].append(['hevc_extra']), 'hevc_extra'),
            ('muxers', lambda r: r['muxers'].append('mpegts'), 'mpegts'),
            ('filters', lambda r: r['filters'].append('yadif'), 'yadif'),
            ('bsfs', lambda r: r['bsfs'].append('h264_mp4toannexb'), 'h264_mp4toannexb'),
            ('hwdevices', lambda r: r['hwdevices'].append('vaapi'), 'vaapi'),
            ('hwaccels', lambda r: r['hwaccels'].append('h264:vaapi:vaapi'), 'h264:vaapi:vaapi'),
            ('protocols.input', lambda r: r['protocols']['input'].append('http'), 'http'),
            ('protocols.output', lambda r: r['protocols']['output'].append('tcp'), 'tcp'),
            ('external_libraries', lambda r: r['external_libraries'].append('openssl'), 'openssl'),
            ('external_libraries.linked', lambda r: r['external_library_sources']['linked'].append('libva'), 'libva'),
            ('external_libraries.pkgconfig', lambda r: r['external_library_sources']['pkgconfig'].append('lzma'), 'lzma'),
            ('external_libraries.configured', lambda r: r['external_library_sources']['configured'].append('iconv'), 'iconv'),
            ('libraries', lambda r: r['libraries'].__setitem__('libavdevice.so.63.1.102', 1), 'avdevice'),
        ]
        for category, mutate, token in unexpected_cases:
            with self.subTest(kind='unexpected', category=category):
                report = self._matching_report(expected)
                mutate(report)
                with self.assertRaises(SystemExit) as refused:
                    inspector.check(report, expected)
                message = str(refused.exception)
                self.assertIn(f'{category}:', message)
                self.assertIn(f'unexpected={token}', message)

        missing_cases = [
            ('decoders', lambda r: r['decoders'].remove('h264'), 'h264'),
            ('encoders', lambda r: r['encoders'].remove('libx265'), 'libx265'),
            ('demuxers', lambda r: r['demuxers'].remove('wav'), 'wav'),
            ('parsers', lambda r: r['parsers'].remove(['ac3', 'eac3']), 'ac3,eac3'),
            ('muxers', lambda r: r['muxers'].remove('mp4'), 'mp4'),
            ('filters', lambda r: r['filters'].remove('scale'), 'scale'),
            ('bsfs', lambda r: r['bsfs'].remove('extract_extradata'), 'extract_extradata'),
            ('protocols.input', lambda r: r['protocols']['input'].remove('file'), 'file'),
            ('protocols.output', lambda r: r['protocols']['output'].remove('file'), 'file'),
            ('external_libraries', lambda r: r['external_libraries'].remove('zlib'), 'zlib'),
            ('external_libraries.linked', lambda r: r['external_library_sources']['linked'].remove('libx264'), 'libx264'),
            ('external_libraries.pkgconfig', lambda r: r['external_library_sources']['pkgconfig'].remove('libdav1d'), 'libdav1d'),
            ('external_libraries.configured', lambda r: r['external_library_sources']['configured'].remove('libx265'), 'libx265'),
            ('libraries', lambda r: r['libraries'].pop('libswscale.so.1.0.0'), 'swscale'),
        ]
        for category, mutate, token in missing_cases:
            with self.subTest(kind='missing', category=category):
                report = self._matching_report(expected)
                mutate(report)
                with self.assertRaises(SystemExit) as refused:
                    inspector.check(report, expected)
                message = str(refused.exception)
                self.assertIn(f'{category}:', message)
                self.assertIn(f'missing={token}', message)

    def test_elf_pe_and_pkgconfig_external_library_enumeration(self):
        with tempfile.TemporaryDirectory(dir=builder.ROOT / 'target') as directory:
            root = Path(directory)
            so_path = root / 'libavcodec.so.63.1.102'
            so_path.write_bytes(_minimal_elf64([
                'libc.so.6', 'libm.so.6', 'libavutil.so.61',
                'libx264.so.165', 'libz.so.1', 'libssl.so.3',
            ]))
            self.assertEqual(
                inspector.binary_external_libraries([so_path]),
                ['libssl', 'libx264', 'zlib'],
            )

            dll_path = root / 'avcodec-63.dll'
            dll_path.write_bytes(_minimal_pe64(
                ['KERNEL32.dll', 'bcrypt.dll', 'avutil-61.dll', 'api-ms-win-crt-runtime-l1-1-0.dll', 'WS2_32.dll'],
                delay_names=['d3d11.dll'],
            ))
            self.assertEqual(
                inspector.binary_external_libraries([dll_path]),
                ['d3d11', 'ws2_32'],
            )

            pc_linux = root / 'linux-prefix/lib/pkgconfig'
            pc_linux.mkdir(parents=True)
            (pc_linux / 'libavcodec.pc').write_text(
                'Libs: -L${libdir} -lavcodec\n'
                'Libs.private: -pthread -lm -latomic -ldav1d -lx264 -lx265 -lz -lssl /usr/lib/libva.so.2\n',
                encoding='utf-8',
            )
            self.assertEqual(
                inspector.pkgconfig_external_libraries(root / 'linux-prefix'),
                ['libdav1d', 'libx264', 'libx265', 'ssl', 'va', 'zlib'],
            )

            pc_win = root / 'win-prefix/lib/pkgconfig'
            pc_win.mkdir(parents=True)
            (pc_win / 'libavcodec.pc').write_text(
                'Libs: -L${libdir} avcodec.lib\n'
                'Libs.private: -libpath:C:\\vcpkg\\lib dav1d.lib libx264.lib x265-static.lib zlib.lib bcrypt.lib user32.lib ws2_32.lib\n',
                encoding='utf-8',
            )
            self.assertEqual(
                inspector.pkgconfig_external_libraries(root / 'win-prefix'),
                ['libdav1d', 'libx264', 'libx265', 'ws2_32', 'zlib'],
            )

    def test_elf_dynamic_section_removed_or_retyped_still_reports_dependencies(self):
        deps = ['libc.so.6', 'libavutil.so.61', 'libx264.so.165', 'libssl.so.3']
        with tempfile.TemporaryDirectory(dir=builder.ROOT / 'target') as directory:
            root = Path(directory)

            with self.subTest(case='retyped_sht_dynamic'):
                retyped = root / 'retyped.so.1'
                retyped.write_bytes(_minimal_elf64(deps, dynamic_sh_type=1))  # SHT_PROGBITS
                self.assertEqual(inspector.elf_needed(retyped), sorted(deps))
                self.assertEqual(inspector.binary_external_libraries([retyped]), ['libssl', 'libx264'])

            with self.subTest(case='stripped_section_headers'):
                stripped = root / 'stripped.so.1'
                stripped.write_bytes(_minimal_elf64(deps, include_shdrs=False))
                self.assertEqual(inspector.elf_needed(stripped), sorted(deps))
                self.assertEqual(inspector.binary_external_libraries([stripped]), ['libssl', 'libx264'])

            with self.subTest(case='unmapped_pt_dynamic'):
                unmapped = root / 'unmapped.so.1'
                unmapped.write_bytes(_minimal_elf64(deps, include_shdrs=False, include_load=False))
                with self.assertRaisesRegex(ValueError, 'unmapped DT_STRTAB'):
                    inspector.elf_needed(unmapped)

            with self.subTest(case='neither_pt_nor_sht_dynamic'):
                no_dynamic = root / 'static.so.1'
                no_dynamic.write_bytes(_minimal_elf64([], include_dynamic=False))
                self.assertEqual(inspector.elf_needed(no_dynamic), [])

    def test_pkgconfig_parses_wl_and_xlinker_and_rejects_unknown_options(self):
        with tempfile.TemporaryDirectory(dir=builder.ROOT / 'target') as directory:
            root = Path(directory)
            pc_dir = root / 'lib/pkgconfig'
            pc_dir.mkdir(parents=True)
            pc_file = pc_dir / 'libavcodec.pc'

            for libs_private in [
                '-Wl,/x/libfoo.a',
                '-Wl,-lfoo',
                '-Wl,-Bstatic,-lfoo,-Bdynamic',
                '-Xlinker -lfoo',
                '-Xlinker /x/libfoo.a',
            ]:
                with self.subTest(case=libs_private):
                    pc_file.write_text(
                        f'Libs: -L${{libdir}} -lavcodec\nLibs.private: {libs_private}\n',
                        encoding='utf-8',
                    )
                    self.assertEqual(inspector.pkgconfig_external_libraries(root), ['foo'])

            pc_file.write_text(
                'Libs: -L${libdir} -lavcodec\n'
                'Libs.private: -Wl,--as-needed -Wl,-rpath,/x -Wl,-z,relro -lm\n',
                encoding='utf-8',
            )
            self.assertEqual(inspector.pkgconfig_external_libraries(root), [])

            for bad_line, token in [
                ('-Wl,--unknown-linker-opt', '--unknown-linker-opt'),
                ('-Xlinker --unknown-xlinker-opt', '--unknown-xlinker-opt'),
                ('--unknown-top-level-opt', '--unknown-top-level-opt'),
                ('-Wl,-rpath,-lfoo', '-rpath'),
                ('-Xlinker', '-Xlinker'),
            ]:
                with self.subTest(bad=bad_line):
                    pc_file.write_text(
                        f'Libs: -L${{libdir}} -lavcodec\nLibs.private: {bad_line}\n',
                        encoding='utf-8',
                    )
                    with self.assertRaises(ValueError) as refused:
                        inspector.pkgconfig_external_libraries(root)
                    self.assertIn(token, str(refused.exception))

    def test_pe_delay_import_without_rva_bit_reports_or_raises(self):
        with tempfile.TemporaryDirectory(dir=builder.ROOT / 'target') as directory:
            root = Path(directory)

            va_dll = root / 'va-delay.dll'
            va_dll.write_bytes(_minimal_pe64(
                ['KERNEL32.dll'], delay_names=['d3d11.dll'],
                image_base=0x10000000, delay_attrs=0,
            ))
            self.assertEqual(inspector.pe_imports(va_dll), ['KERNEL32.dll', 'd3d11.dll'])
            self.assertEqual(inspector.binary_external_libraries([va_dll]), ['d3d11'])

            bad_va = root / 'bad-va-delay.dll'
            bad_va.write_bytes(_minimal_pe64(
                ['KERNEL32.dll'], delay_names=['d3d11.dll'],
                image_base=0x180000000, delay_attrs=0, delay_raw_addrs=[0x1050],
            ))
            with self.assertRaisesRegex(ValueError, 'unsupported PE delay import'):
                inspector.pe_imports(bad_va)

            bad_attrs = root / 'bad-attrs-delay.dll'
            bad_attrs.write_bytes(_minimal_pe64(
                ['KERNEL32.dll'], delay_names=['d3d11.dll'],
                image_base=0x10000000, delay_attrs=2, delay_raw_addrs=[0x1050],
            ))
            with self.assertRaisesRegex(ValueError, 'unsupported PE delay import'):
                inspector.pe_imports(bad_attrs)

    def test_configured_hwaccel_refuses_as_unimplemented_while_unexpected_actual_still_fails(self):
        base_args = builder.configure('libraries', 'Linux')
        fake_builder = type('FakeBuilder', (), {
            'PROFILE': builder.PROFILE,
            'configure': staticmethod(lambda profile, system=None: [*base_args, '--enable-hwaccel=h264_vaapi']),
        })
        with patch.object(inspector, '_load_builder', return_value=fake_builder):
            with self.assertRaises(SystemExit) as refused:
                inspector.expected_surface(system='Linux')
        self.assertIn('not implemented', str(refused.exception))
        self.assertIn('h264_vaapi', str(refused.exception))

        expected = inspector.expected_surface(system='Linux')
        report = self._matching_report(expected)
        with self.assertRaises(SystemExit) as refused:
            inspector.check(report, {**expected, 'hwaccels': {'h264_vaapi'}})
        self.assertIn('not implemented', str(refused.exception))
        self.assertIn('h264_vaapi', str(refused.exception))

        report['hwaccels'].append('h264:vaapi:vaapi')
        with self.assertRaises(SystemExit) as refused:
            inspector.check(report, expected)
        self.assertIn('hwaccels:', str(refused.exception))
        self.assertIn('unexpected=h264:vaapi:vaapi', str(refused.exception))

    def test_built_ffmpeg_matches_exact_profile_surface(self):
        prefix = builder.ROOT / 'third_party/ffmpeg'
        self.assertTrue(prefix.is_dir(), 'run scripts/setup-ffmpeg.sh first')
        report = inspector.inspect(prefix)
        inspector.check(report)
        self.assertEqual(report['protocols'], {'input': ['file'], 'output': ['file']})
        self.assertEqual(report['hwdevices'], [])
        self.assertEqual(report['hwaccels'], [])
        self.assertEqual(report['external_libraries'], ['libdav1d', 'libx264', 'libx265', 'zlib'])


if __name__ == '__main__':
    unittest.main()
