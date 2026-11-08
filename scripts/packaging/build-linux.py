#!/usr/bin/env python3
"""Build downloadable Linux packages; no application store publication."""

import argparse
import json
import os
from pathlib import Path
import platform
import shutil
import subprocess
import tempfile
import tomllib

import importlib.util

ROOT = Path(__file__).resolve().parents[2]
APP_ID = "com.vitalyart.airpods-driver"
DIST = ROOT / "dist"


def check_tools(formats):
    required = ["cargo", "rustc"]
    tools = {
        "deb": ("dpkg", "dpkg-deb", "dpkg-shlibdeps"),
        "rpm": ("rpmbuild",),
        "appimage": ("linuxdeploy", "appimagetool", "patchelf", "ldconfig"),
        "flatpak": ("flatpak",),
    }
    for package_format in formats:
        required.extend(tools[package_format])
    missing = [tool for tool in dict.fromkeys(required) if not shutil.which(tool)]
    if missing:
        raise SystemExit(
            f"Missing build tools: {', '.join(missing)}.\n"
            "On Ubuntu/Debian, rpmbuild is provided by the rpm package (sudo apt install rpm).\n"
            "See docs/packaging.md for all packaging dependencies and container build instructions."
        )


def run(*args, **kwargs):
    subprocess.run([str(arg) for arg in args], check=True, cwd=ROOT, **kwargs)


def install(source, destination, mode=0o644):
    destination.parent.mkdir(parents=True, exist_ok=True)
    shutil.copyfile(source, destination)
    destination.chmod(mode)


def stage_files(stage, binary, prefix="usr"):
    install(binary, stage / prefix / "bin/airpods-driver", 0o755)
    install(ROOT / f"packaging/desktop/{APP_ID}.desktop", stage / prefix / f"share/applications/{APP_ID}.desktop")
    install(ROOT / "assets/icon.png", stage / prefix / f"share/icons/hicolor/256x256/apps/{APP_ID}.png")
    install(ROOT / "LICENSE", stage / prefix / "share/licenses/airpods-driver/LICENSE")


def build_binary(arch):
    supplied = os.environ.get("AIRPODS_DRIVER_PACKAGE_BINARY")
    if supplied:
        binary = Path(supplied)
    else:
        result = subprocess.run(
            ["cargo", "build", "--release", "--locked", "--message-format=json-render-diagnostics"],
            check=True, cwd=ROOT, stdout=subprocess.PIPE, text=True,
        )
        artifacts = [json.loads(line) for line in result.stdout.splitlines()]
        binary = Path(next(item["executable"] for item in artifacts
                           if item.get("reason") == "compiler-artifact"
                           and item["target"]["name"] == "airpods-driver" and item.get("executable")))
    with binary.open("rb") as executable:
        header = executable.read(20)
    expected = {"x86_64": 62, "aarch64": 183}[arch]
    if len(header) != 20 or header[:4] != b"\x7fELF" or int.from_bytes(
        header[18:20], "little" if header[5] == 1 else "big"
    ) != expected:
        raise RuntimeError(f"Executable does not match packaging architecture {arch}")
    return binary


def rpm(version, binary, work):
    # Native RPM dependency generation records the actual ELF/GLIBC requirements.
    top = work / "rpm"
    for directory in ("BUILD", "BUILDROOT", "RPMS", "SOURCES", "SPECS", "SRPMS"):
        (top / directory).mkdir(parents=True)
    stage_files(top / "SOURCES/payload", binary)
    marker = top / "SOURCES/payload/usr/share/airpods-driver/package-format"
    marker.parent.mkdir(parents=True, exist_ok=True)
    marker.write_text("rpm\n")
    base, _, metadata = version.partition("+")
    base = base.replace("-", "~", 1)
    release = "1" + ("." + metadata.replace("-", ".") if metadata else "")
    spec = top / "SPECS/airpods-driver.spec"
    spec.write_text(f"""Name: airpods-driver
Version: {base}
Release: {release}
Summary: AirPods controls for the Linux desktop
License: AGPL-3.0-only
Requires: bluez, hicolor-icon-theme, libglvnd-glx, libglvnd-egl
Recommends: PackageKit, polkit
Requires: libxkbcommon, libxkbcommon-x11, libwayland-client, libX11-xcb, libXcursor, libXi, libXrandr

%description
View battery levels and control supported AirPods features.

%prep
%build
%install
mkdir -p %{{buildroot}}
cp -a %{{_sourcedir}}/payload/. %{{buildroot}}/

%files
/usr/bin/airpods-driver
/usr/share/airpods-driver/package-format
/usr/share/applications/{APP_ID}.desktop
/usr/share/icons/hicolor/256x256/apps/{APP_ID}.png
%license /usr/share/licenses/airpods-driver/LICENSE
""")
    run("rpmbuild", "-bb", "--define", f"_topdir {top}", "--define", "_build_id_links none", spec)
    packages = list((top / "RPMS").rglob("*.rpm"))
    if len(packages) != 1:
        raise RuntimeError(f"Expected one RPM, found {packages}")
    install(packages[0], DIST / packages[0].name)


def appimage(version, binary, work, arch):
    stage = work / "AppDir"
    stage_files(stage, binary)
    install(ROOT / f"packaging/desktop/{APP_ID}.desktop", stage / f"{APP_ID}.desktop")
    install(ROOT / "assets/icon.png", stage / f"{APP_ID}.png")
    apprun = stage / "AppRun"
    apprun.write_text('''#!/bin/sh
HERE="$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)"
export LD_LIBRARY_PATH="$HERE/usr/lib:$HERE/usr/lib/pulseaudio${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
exec "$HERE/usr/bin/airpods-driver" "$@"
''')
    apprun.chmod(0o755)
    env = dict(os.environ, APPIMAGE_EXTRACT_AND_RUN="1", ARCH=arch,
               OUTPUT=str(DIST / f"airpods-driver-{version}-{arch}.AppImage"),
               LINUXDEPLOY_OUTPUT_VERSION=version)
    # linuxdeploy and appimagetool must be installed in PATH.
    # PulseAudio loads libpulsecommon from a private directory; deploy it explicitly.
    common = list(Path("/usr/lib").glob("**/pulseaudio/libpulsecommon-*.so"))
    extra = [arg for library in common for arg in ("--library", str(library))]
    libraries = subprocess.check_output(["ldconfig", "-p"], text=True)
    # GPU drivers and core display libraries must come from the desktop host.
    for name in ("libxkbcommon.so.0", "libxkbcommon-x11.so.0",
                 "libXcursor.so.1", "libXi.so.6", "libXrandr.so.2"):
        matches = [line.split(" => ")[-1] for line in libraries.splitlines() if line.strip().startswith(name + " ")]
        if not matches:
            raise RuntimeError(f"Missing AppImage runtime library: {name}")
        extra.extend(("--library", matches[0]))
    run("linuxdeploy", "--appdir", stage, "--executable", stage / "usr/bin/airpods-driver",
        "--desktop-file", stage / f"{APP_ID}.desktop", "--icon-file", stage / f"{APP_ID}.png",
        *extra, env=env)
    run("appimagetool", stage, DIST / f"airpods-driver-{version}-{arch}.AppImage", env=env)


def flatpak(version, work):
    # All four formats package the same native release executable. The runtime is
    # also the build environment: we only copy files, so no separate SDK is needed.
    binary = build_binary(platform.machine())
    config = json.loads((ROOT / "packaging/flatpak/package.json").read_text())
    runtime, branch = config["runtime"], config["runtime-version"]
    run("flatpak", "install", "--user", "--noninteractive", "--assumeyes",
        "flathub", f"{runtime}//{branch}")
    stage = work / "flatpak"
    run("flatpak", "build-init", stage, APP_ID, runtime, runtime, branch)
    stage_files(stage, binary, prefix="files")
    install(ROOT / f"packaging/flatpak/{APP_ID}.metainfo.xml",
            stage / f"files/share/metainfo/{APP_ID}.metainfo.xml")
    run("flatpak", "build-finish", "--command=airpods-driver", *config["finish-args"], stage)
    # Fail before publishing a bundle if the build host's libc/library ABI is too
    # new for the runtime. Release builds use the Ubuntu 24.04 environment.
    run("flatpak", "build", stage, "/app/bin/airpods-driver", "--version")
    repo = work / "repo"
    run("flatpak", "build-export", repo, stage)
    run("flatpak", "build-bundle", repo, DIST / f"airpods-driver-{version}-{platform.machine()}.flatpak",
        APP_ID, "--runtime-repo=https://dl.flathub.org/repo/flathub.flatpakrepo")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("format", choices=("rpm", "appimage", "flatpak"))
    args = parser.parse_args()
    check_tools((args.format,))
    version = tomllib.loads((ROOT / "Cargo.toml").read_text())["package"]["version"]
    spec = importlib.util.spec_from_file_location("build_deb", ROOT / "scripts/packaging/build-deb.py")
    deb = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(deb)
    deb.debian_version(version, os.environ.get("RELEASE_TAG", ""))
    arch = platform.machine()
    if arch not in ("x86_64", "aarch64"):
        raise SystemExit(f"Unsupported architecture: {arch}")
    DIST.mkdir(exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="airpods-driver-package-") as temporary:
        work = Path(temporary)
        if args.format == "flatpak":
            flatpak(version, work)
        else:
            binary = build_binary(arch)
            if args.format == "rpm":
                rpm(version, binary, work)
            else:
                appimage(version, binary, work, arch)


if __name__ == "__main__":
    main()
