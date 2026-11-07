#!/usr/bin/env python3
"""Apply RELEASE_TAG to the private build snapshot without changing dependencies."""

import argparse
import os
from pathlib import Path
import re
import tomllib

ROOT = Path(__file__).resolve().parents[2]


def version_from_tag(tag):
    match = re.fullmatch(
        r"v((?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)"
        r"(?:-([0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*))?"
        r"(?:\+[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?)", tag,
    )
    if not match or any(
        identifier.isdigit() and len(identifier) > 1 and identifier.startswith("0")
        for identifier in (match.group(2) or "").split(".")
    ):
        raise ValueError(f"Release tag must be v<SemVer>, for example v1.0.0 or v1.1.0-rc.1: {tag!r}")
    return match.group(1)


def replace_package_version(document, name, version, *, lockfile=False):
    header = r"\[\[package\]\]" if lockfile else r"\[package\]"
    pattern = rf"(?ms)^{header}\s*\n.*?(?=^\[|\Z)"
    matches = []
    for match in re.finditer(pattern, document):
        package = tomllib.loads(match.group())["package"]
        if lockfile:
            package = package[0]
        if package["name"] == name and "source" not in package:
            matches.append(match)
    if len(matches) != 1:
        raise ValueError(f"Expected one local package {name!r} in {'Cargo.lock' if lockfile else 'Cargo.toml'}")
    match = matches[0]
    replacement, count = re.subn(r'(?m)^version\s*=\s*"[^"\n]+"',
                                  f'version = "{version}"', match.group())
    if count != 1:
        raise ValueError(f"Expected one version field for {name!r}")
    return document[:match.start()] + replacement + document[match.end():]


def prepare(root, tag, *, check_only=False):
    manifest_path = root / "Cargo.toml"
    manifest = manifest_path.read_text()
    package = tomllib.loads(manifest)["package"]
    if not tag:
        return package["version"]
    version = version_from_tag(tag)
    lock_path = root / "Cargo.lock"
    lock = lock_path.read_text()
    manifest = replace_package_version(manifest, package["name"], version)
    lock = replace_package_version(lock, package["name"], version, lockfile=True)
    if not check_only:
        manifest_path.write_text(manifest)
        lock_path.write_text(lock)
    return version


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check-only", action="store_true")
    args = parser.parse_args()
    print(f"Build version: {prepare(ROOT, os.environ.get('RELEASE_TAG', ''), check_only=args.check_only)}")


if __name__ == "__main__":
    try:
        main()
    except ValueError as error:
        raise SystemExit(str(error)) from error
