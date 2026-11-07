#!/usr/bin/env python3
"""Build all formats from one private snapshot of the release executable."""

import importlib.util
import os
from pathlib import Path
import platform
import shutil
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[2]


def main():
    spec = importlib.util.spec_from_file_location("build_linux", ROOT / "scripts/packaging/build-linux.py")
    builder = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(builder)
    builder.check_tools(("deb", "rpm", "appimage", "flatpak"))
    with tempfile.TemporaryDirectory(prefix="airpods-driver-release-") as temporary:
        binary = Path(temporary) / "airpods-driver"
        shutil.copyfile(builder.build_binary(platform.machine()), binary)
        binary.chmod(0o755)
        env = dict(os.environ, AIRPODS_DRIVER_PACKAGE_BINARY=str(binary))
        for script, arguments in (
            ("build-deb.py", []),
            ("build-linux.py", ["rpm"]),
            ("build-linux.py", ["appimage"]),
            ("build-linux.py", ["flatpak"]),
        ):
            subprocess.run(["python3", str(ROOT / "scripts/packaging" / script), *arguments],
                           cwd=ROOT, env=env, check=True)


if __name__ == "__main__":
    main()
