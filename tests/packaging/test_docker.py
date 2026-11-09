import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest


class DockerPackagingTests(unittest.TestCase):
    def test_container_arguments_for_each_format(self):
        script = Path(__file__).resolve().parents[2] / "scripts/packaging/package-docker.sh"
        with tempfile.TemporaryDirectory() as temporary:
            temporary = Path(temporary)
            docker = temporary / "docker"
            docker.write_text('''#!/usr/bin/env python3
import json, os, sys
with open(os.environ["DOCKER_TEST_LOG"], "a") as log:
    log.write(json.dumps(sys.argv[1:]) + "\\n")
if sys.argv[1:3] == ["image", "inspect"]:
    print(os.environ.get("DOCKER_TEST_ARCH", "amd64"))
''')
            docker.chmod(0o755)
            log = temporary / "calls.jsonl"
            env = dict(os.environ, PATH=f"{temporary}:{os.environ['PATH']}",
                       DOCKER_TEST_LOG=str(log), RELEASE_TAG="v0.1.0")
            env.pop("PACKAGING_CACHE_DIR", None)
            env.pop("PACKAGING_SKIP_IMAGE_BUILD", None)
            env.pop("DOCKER_TEST_ARCH", None)
            for format in ("all", "deb", "rpm", "appimage", "flatpak", "test", "binary"):
                with self.subTest(format=format):
                    log.write_text("")
                    subprocess.run(["bash", str(script), format], env=env, check=True)
                    calls = [json.loads(line) for line in log.read_text().splitlines()]
                    self.assertEqual([call[0] for call in calls], ["info", "build", "image", "run"])
                    run = calls[-1]
                    self.assertEqual("--privileged" in run, format in ("all", "flatpak"))
                    self.assertEqual(run[-1], format)
                    self.assertIn("RELEASE_TAG=v0.1.0", run)
                    mounts = [run[i + 1] for i, arg in enumerate(run) if arg == "--mount"]
                    self.assertTrue(any(m.endswith("dst=/source,readonly") for m in mounts))
                    self.assertTrue(any("airpods-driver-package-target-amd64" in m for m in mounts))
                    self.assertFalse(any("docker.sock" in m for m in mounts))

            # CI loads the image via Buildx and persists host directories instead
            # of ephemeral volumes. Exercise this path for both architectures.
            for arch in ("amd64", "arm64"):
                with self.subTest(ci_arch=arch):
                    cache = temporary / f"cache {arch}"
                    log.write_text("")
                    ci_env = dict(env, PACKAGING_CACHE_DIR=str(cache), PACKAGING_SKIP_IMAGE_BUILD="1",
                                  DOCKER_TEST_ARCH=arch)
                    subprocess.run(["bash", str(script), "all"], env=ci_env, check=True)
                    calls = [json.loads(line) for line in log.read_text().splitlines()]
                    self.assertEqual([call[0] for call in calls], ["info", "image", "run"])
                    run = calls[-1]
                    mounts = [run[i + 1] for i, arg in enumerate(run) if arg == "--mount"]
                    for directory, destination in (
                        (f"target-{arch}", "/build-target"),
                        ("registry", "/opt/cargo/registry"),
                        ("git", "/opt/cargo/git"),
                        (f"flatpak-{arch}", "/root/.local/share/flatpak"),
                    ):
                        self.assertTrue((cache / directory).is_dir())
                        self.assertIn(f"type=bind,src={cache / directory},dst={destination}", mounts)
                    self.assertFalse(any(m.startswith("type=volume,") for m in mounts))

    def test_missing_supplied_binary_fails(self):
        script = Path(__file__).resolve().parents[2] / "scripts/packaging/package-docker.sh"
        with tempfile.TemporaryDirectory() as temporary:
            docker = Path(temporary) / "docker"
            docker.write_text("#!/bin/sh\nif [ \"$1\" = image ]; then echo amd64; fi\n")
            docker.chmod(0o755)
            env = dict(os.environ, PATH=f"{temporary}:{os.environ['PATH']}",
                       PACKAGING_SKIP_IMAGE_BUILD="1",
                       AIRPODS_DRIVER_PACKAGE_BINARY=str(Path(temporary) / "missing"))
            result = subprocess.run(["bash", str(script), "deb"], env=env,
                                    capture_output=True, text=True)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("Package binary missing", result.stderr)
