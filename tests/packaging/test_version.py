import importlib.util
from pathlib import Path
import subprocess
import unittest

script = Path(__file__).resolve().parents[2] / "scripts/packaging/build-deb.py"
spec = importlib.util.spec_from_file_location("build_deb", script)
build_deb = importlib.util.module_from_spec(spec)
spec.loader.exec_module(build_deb)


class ReleaseVersionTests(unittest.TestCase):
    def test_prerelease_sorts_before_stable_in_debian(self):
        prerelease = build_deb.debian_version("1.2.3-rc.1", "v1.2.3-rc.1")
        stable = build_deb.debian_version("1.2.3", "v1.2.3")
        subprocess.run(["dpkg", "--compare-versions", prerelease, "lt", stable], check=True)

    def test_tag_must_match_binary_version(self):
        for tag in ("v1.2.4", "1.2.3", "v1.2.3/extra"):
            with self.subTest(tag=tag), self.assertRaises(ValueError):
                build_deb.debian_version("1.2.3", tag)

    def test_preserves_build_metadata(self):
        self.assertEqual(build_deb.debian_version("1.2.3+build-test"), "1.2.3+build-test")
        self.assertEqual(build_deb.debian_version("1.2.3-rc.1+build-test"), "1.2.3~rc.1+build-test")

    def test_rejects_unsafe_versions(self):
        for version in ("1.2.3\nDepends: bad", "../1.2.3", "latest"):
            with self.subTest(version=version), self.assertRaises(ValueError):
                build_deb.debian_version(version)


if __name__ == "__main__":
    unittest.main()
