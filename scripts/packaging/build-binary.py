#!/usr/bin/env python3
"""Export the native release executable for downstream packaging jobs."""

import importlib.util
from pathlib import Path
import platform
import shutil
import sys


def main():
    spec = importlib.util.spec_from_file_location(
        "build_linux", Path(__file__).with_name("build-linux.py")
    )
    builder = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(builder)
    destination = Path(sys.argv[1])
    destination.parent.mkdir(parents=True, exist_ok=True)
    shutil.copyfile(builder.build_binary(platform.machine()), destination)
    destination.chmod(0o755)


if __name__ == "__main__":
    main()
