import importlib.util
from pathlib import Path
import unittest
from unittest.mock import patch


scripts = Path(__file__).resolve().parents[2] / "scripts/packaging"


def load(name):
    spec = importlib.util.spec_from_file_location(name.replace("-", "_"), scripts / f"{name}.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


class PackagingToolTests(unittest.TestCase):
    def test_reports_all_missing_tools(self):
        builder = load("build-linux")
        with patch.object(builder.shutil, "which", side_effect=lambda tool: None if tool in (
            "rpmbuild", "linuxdeploy", "flatpak"
        ) else f"/usr/bin/{tool}"):
            with self.assertRaisesRegex(SystemExit, "rpmbuild, linuxdeploy, flatpak"):
                builder.check_tools(("deb", "rpm", "appimage", "flatpak"))

    def test_rpm_does_not_require_other_format_tools(self):
        builder = load("build-linux")
        with patch.object(builder.shutil, "which", side_effect=lambda tool: (
            f"/usr/bin/{tool}" if tool in ("cargo", "rustc", "rpmbuild") else None
        )):
            builder.check_tools(("rpm",))

    def test_all_formats_fail_before_compilation(self):
        builder = load("build-packages")
        with patch("shutil.which", return_value=None), patch("subprocess.run") as run:
            with self.assertRaisesRegex(SystemExit, "Missing build tools"):
                builder.main()
            run.assert_not_called()
