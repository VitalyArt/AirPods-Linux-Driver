#!/usr/bin/env python3
"""Build a native Debian package with dependencies resolved from the executable."""

import argparse
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import tempfile
import tomllib

ROOT = Path(__file__).resolve().parents[2]
APP_ID = "com.vitalyart.airpods-driver"


def debian_version(version, tag=""):
    if not re.fullmatch(
        r"[0-9]+\.[0-9]+\.[0-9]+(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?",
        version,
    ):
        raise ValueError(f"Unsupported release version: {version}")
    if tag and tag != f"v{version}":
        raise ValueError(f"Release tag {tag!r} must match Cargo.toml: v{version}")
    # Debian sorts prereleases before the corresponding stable release.
    base, separator, metadata = version.partition("+")
    return base.replace("-", "~", 1) + separator + metadata


def output(*command, cwd=ROOT):
    return subprocess.check_output(command, cwd=cwd, text=True).strip()


def install(source, destination, mode=0o644):
    destination.parent.mkdir(parents=True, exist_ok=True)
    shutil.copyfile(source, destination)
    destination.chmod(mode)


def main():
    os.umask(0o022)
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--validate-version", action="store_true")
    args = parser.parse_args()
    manifest = tomllib.loads((ROOT / "Cargo.toml").read_text())
    version = debian_version(manifest["package"]["version"], os.environ.get("RELEASE_TAG", ""))
    if args.validate_version:
        print(version)
        return

    for tool in ("cargo", "rustc", "dpkg", "dpkg-deb", "dpkg-shlibdeps"):
        if not shutil.which(tool):
            raise SystemExit(f"Missing build tool: {tool}. See docs/packaging.md.")

    arch = output("dpkg", "--print-architecture")
    machines = {"amd64": 62, "arm64": 183}
    if arch not in machines:
        raise SystemExit("Supported DEB architectures: amd64 and arm64.")
    supplied = os.environ.get("AIRPODS_DRIVER_PACKAGE_BINARY")
    if supplied:
        binary = Path(supplied)
    else:
        built = subprocess.run(
            ["cargo", "build", "--release", "--locked", "--message-format=json-render-diagnostics"],
            cwd=ROOT,
            check=True,
            stdout=subprocess.PIPE,
            text=True,
        )
        artifacts = [json.loads(line) for line in built.stdout.splitlines()]
        binary = Path(next(
            artifact["executable"]
            for artifact in artifacts
            if artifact.get("reason") == "compiler-artifact"
            and artifact["target"]["name"] == "airpods-driver"
            and artifact.get("executable")
        ))
    # Do not label a cross-compiled executable with the packaging host's architecture.
    with binary.open("rb") as executable:
        header = executable.read(20)
    if len(header) != 20 or header[:4] != b"\x7fELF":
        raise SystemExit("Expected a Linux ELF executable.")
    byteorder = "little" if header[5] == 1 else "big"
    if int.from_bytes(header[18:20], byteorder) != machines[arch]:
        raise SystemExit(f"Executable architecture does not match Debian host {arch}.")
    dist = ROOT / "dist"
    dist.mkdir(exist_ok=True)
    package = dist / f"airpods-driver_{version}_{arch}.deb"

    with tempfile.TemporaryDirectory(prefix="airpods-driver-deb-") as temporary:
        work = Path(temporary)
        stage = work / "package"
        (stage / "DEBIAN").mkdir(parents=True)
        install(binary, stage / "usr/bin/airpods-driver", 0o755)
        (stage / "usr/share/airpods-driver").mkdir(parents=True, exist_ok=True)
        (stage / "usr/share/airpods-driver/package-format").write_text("deb\n")
        install(ROOT / f"packaging/desktop/{APP_ID}.desktop", stage / f"usr/share/applications/{APP_ID}.desktop")
        install(ROOT / "assets/icon.png", stage / f"usr/share/icons/hicolor/256x256/apps/{APP_ID}.png")
        install(ROOT / "LICENSE", stage / "usr/share/doc/airpods-driver/copyright")
        install(ROOT / "README.md", stage / "usr/share/doc/airpods-driver/README.md")

        # dpkg-shlibdeps needs source package metadata even for a standalone binary.
        (work / "debian").mkdir()
        (work / "debian/control").write_text(
            "Source: airpods-driver\nSection: sound\nPriority: optional\n"
            "Maintainer: VitalyArt <noreply@vitaly.art>\n\n"
            "Package: airpods-driver\nArchitecture: any\nDescription: AirPods controls for Linux\n"
        )
        linked = output("dpkg-shlibdeps", "-O", f"-e{stage / 'usr/bin/airpods-driver'}", cwd=work)
        dependencies = next(
            line.removeprefix("shlibs:Depends=")
            for line in linked.splitlines()
            if line.startswith("shlibs:Depends=")
        )
        # Window backends load these libraries dynamically, outside ELF dependencies.
        runtime = (
            "bluez, hicolor-icon-theme, libgl1, libegl1, libxkbcommon0, libxkbcommon-x11-0, "
            "libwayland-client0, libx11-6, libx11-xcb1, libxcursor1, libxi6, libxrandr2"
        )
        installed_size = int(output("du", "-sk", str(stage)).split()[0])
        control = stage / "DEBIAN/control"
        control.write_text(
            f"Package: airpods-driver\nVersion: {version}\nArchitecture: {arch}\n"
            "Section: sound\nPriority: optional\n"
            "Maintainer: VitalyArt <noreply@vitaly.art>\n"
            f"Installed-Size: {installed_size}\nDepends: {dependencies}, {runtime}\n"
            "Recommends: pulseaudio | pipewire-pulse, packagekit, policykit-1\n"
            "Description: AirPods controls for the Linux desktop\n"
            " View headphone and case battery levels, switch listening modes,\n"
            " and control supported AirPods features from a window or system tray.\n"
        )
        # Publish the file only after dpkg-deb has finished successfully.
        with tempfile.TemporaryDirectory(prefix=".deb-", dir=dist) as package_temporary:
            pending = Path(package_temporary) / package.name
            subprocess.run(
                ["dpkg-deb", "--build", "--root-owner-group", str(stage), str(pending)],
                check=True,
            )
            pending.replace(package)
    print(f"Built {package}")


if __name__ == "__main__":
    try:
        main()
    except (ValueError, subprocess.CalledProcessError) as error:
        raise SystemExit(str(error)) from error
