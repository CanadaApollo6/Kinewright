#!/usr/bin/env python3
"""Positive and mutation witnesses for the CI-run MS2 client boundary."""

from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

SCRIPT = Path(__file__).with_name("ms2_client_boundary.py")


class Boundary(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="kinewright-ms2-boundary-")
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        (self.root / "Cargo.toml").write_text('''[workspace]
members = ["crates/*"]
[workspace.dependencies]
zbus = "=5.19.0"
bus = { package = "zbus", version = "=5.19.0" }
''')
        (self.root / "Cargo.lock").write_text('[[package]]\nname = "zbus"\nversion = "5.19.0"\n')
        self.app = self.member("kinewright-app")
        self.member("kinewright-media-supervisor", '[dependencies]\nzbus.workspace = true\n')

    def member(self, name, dependencies=""):
        directory = self.root / "crates" / name
        (directory / "src").mkdir(parents=True)
        (directory / "src/main.rs").write_text("fn main() {}\n")
        manifest = directory / "Cargo.toml"
        manifest.write_text(f'[package]\nname = "{name}"\nversion = "0.1.0"\n' + dependencies)
        return manifest

    def run_gate(self):
        return subprocess.run([sys.executable, str(SCRIPT), "--root", str(self.root)],
                              capture_output=True, text=True, check=False)

    def test_positive_supervisor_and_accessibility_transitive(self):
        result = self.run_gate()
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_red_direct_zbus_added_to_app(self):
        self.app.write_text(self.app.read_text() + '[dependencies]\nzbus.workspace = true\n')
        result = self.run_gate()
        self.assertEqual(result.returncode, 1)
        self.assertIn("kinewright-app declares direct zbus", result.stderr)

    def test_dependency_alias_and_target_dev_build_cannot_bypass(self):
        for section in ["dependencies", "dev-dependencies", "build-dependencies",
                        'target.\'cfg(target_os = "linux")\'.dependencies']:
            for declaration in ['bus.workspace = true', 'renamed = { package = "zbus", version = "5" }']:
                with self.subTest(section=section, declaration=declaration):
                    original = self.app.read_text()
                    self.app.write_text(original + f'[{section}]\n{declaration}\n')
                    result = self.run_gate()
                    self.assertEqual(result.returncode, 1, result.stdout)
                    self.assertIn("declares direct zbus", result.stderr)
                    self.app.write_text(original)

    def test_non_app_workspace_crate_cannot_declare_zbus(self):
        self.member("kinewright-media", '[dependencies]\nzbus = "5"\n')
        self.assertEqual(self.run_gate().returncode, 1)

    def test_implicit_path_member_cannot_bypass_boundary(self):
        manifest = self.root / "Cargo.toml"
        manifest.write_text(manifest.read_text().replace('members = ["crates/*"]', 'members = ["crates/kinewright-app", "crates/kinewright-media-supervisor"]'))
        self.member("implicit", '[dependencies]\nzbus = "5"\n')
        self.app.write_text(self.app.read_text() + '[dependencies]\nimplicit = { path = "../implicit" }\n')
        result = self.run_gate()
        self.assertEqual(result.returncode, 1)
        self.assertIn("implicit declares direct zbus", result.stderr)

    def test_app_source_cannot_mention_systemd_manager(self):
        (self.app.parent / "src/main.rs").write_text('// org.freedesktop.systemd1\nfn main() {}\n')
        result = self.run_gate()
        self.assertEqual(result.returncode, 1)
        self.assertIn("src/main.rs:1: app mentions", result.stderr)

    def test_two_zbus_versions_fail(self):
        lock = self.root / "Cargo.lock"
        lock.write_text(lock.read_text() + '[[package]]\nname = "zbus"\nversion = "5.13.2"\n')
        self.assertEqual(self.run_gate().returncode, 1)


if __name__ == "__main__":
    unittest.main()
