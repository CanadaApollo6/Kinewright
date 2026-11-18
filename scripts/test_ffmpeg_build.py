#!/usr/bin/env python3
"""Source admission must fail before extraction, configure, or install replacement."""
import importlib.util
import hashlib
import io
from pathlib import Path
import tarfile
import tempfile
import textwrap
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location('ffmpeg_builder', Path(__file__).with_name('build-ffmpeg.py'))
builder = importlib.util.module_from_spec(spec)
spec.loader.exec_module(builder)


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

    def test_ci_consumers_take_the_restore_flag_from_the_exact_cache_hit(self):
        action = (builder.ROOT/'.github/actions/ffmpeg/action.yml').read_text()
        flag = "KINEWRIGHT_FFMPEG_RESTORED_PRODUCER_CACHE: ${{ inputs.producer != 'true' && steps.cache.outputs.cache-hit == 'true' }}"
        self.assertEqual(action.count(flag), 2)

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


if __name__ == '__main__':
    unittest.main()
