import importlib.util
from pathlib import Path
import tempfile
import tomllib
import unittest

script = Path(__file__).resolve().parents[2] / "scripts/packaging/prepare-release.py"
spec = importlib.util.spec_from_file_location("prepare_release", script)
release = importlib.util.module_from_spec(spec)
spec.loader.exec_module(release)


class ReleasePreparationTests(unittest.TestCase):
    def test_tag_versions(self):
        for version in ("1.0.0", "1.1.0-rc.1", "1.1.0-rc.1+build.02"):
            with self.subTest(version=version):
                self.assertEqual(release.version_from_tag(f"v{version}"), version)

    def test_rejects_invalid_tags(self):
        for tag in ("1.0.0", "v1.0", "v01.0.0", "v1.0.0-rc.01", "v1.0.0\n", "v1.0.0/extra"):
            with self.subTest(tag=tag), self.assertRaises(ValueError):
                release.version_from_tag(tag)

    def test_snapshot_versions_and_dependencies(self):
        manifest = '[package]\nname = "app"\nversion = "0.1.0"\n\n[dependencies]\ndep = "2.0"\n'
        lock = ('version = 4\n\n[[package]]\nname = "app"\nversion = "0.1.0"\n'
                'dependencies = ["dep"]\n\n[[package]]\nname = "dep"\nversion = "2.0.0"\n'
                'source = "registry+https://github.com/rust-lang/crates.io-index"\n')
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "Cargo.toml").write_text(manifest)
            (root / "Cargo.lock").write_text(lock)
            # Branch builds and early CI validation must not modify either file.
            self.assertEqual(release.prepare(root, ""), "0.1.0")
            self.assertEqual(release.prepare(root, "v1.0.0", check_only=True), "1.0.0")
            self.assertEqual((root / "Cargo.toml").read_text(), manifest)
            self.assertEqual((root / "Cargo.lock").read_text(), lock)
            self.assertEqual(release.prepare(root, "v1.0.0"), "1.0.0")
            updated_manifest = tomllib.loads((root / "Cargo.toml").read_text())
            updated_lock = tomllib.loads((root / "Cargo.lock").read_text())
            self.assertEqual(updated_manifest["package"]["version"], "1.0.0")
            self.assertEqual(updated_lock["package"][0]["version"], "1.0.0")
            self.assertEqual(updated_manifest["dependencies"], {"dep": "2.0"})
            self.assertEqual(updated_lock["package"][1], tomllib.loads(lock)["package"][1])
            # Reapplying the same tag is safe.
            self.assertEqual(release.prepare(root, "v1.0.0"), "1.0.0")

    def test_invalid_lock_leaves_manifest_unchanged(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            manifest = '[package]\nname = "app"\nversion = "0.1.0"\n'
            (root / "Cargo.toml").write_text(manifest)
            (root / "Cargo.lock").write_text('version = 4\n')
            with self.assertRaisesRegex(ValueError, "Expected one local package"):
                release.prepare(root, "v1.0.0")
            self.assertEqual((root / "Cargo.toml").read_text(), manifest)
