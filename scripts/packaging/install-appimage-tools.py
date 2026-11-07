#!/usr/bin/env python3
"""Download upstream AppImage build tools, checking GitHub asset SHA256 digests."""
import hashlib
from pathlib import Path
import platform
import subprocess
import tempfile

destination = Path("/usr/local/bin")
arch = platform.machine()
# SHA256 values are from the official GitHub release asset digests. Use versioned
# releases: continuous assets are overwritten and cannot be pinned reliably.
for repo, tag, name, digests in (
    ("linuxdeploy/linuxdeploy", "1-alpha-20251107-1", "linuxdeploy", {
        "x86_64": "c20cd71e3a4e3b80c3483cef793cda3f4e990aca14014d23c544ca3ce1270b4d",
        "aarch64": "620095110d693282b8ebeb244a95b5e911cf8f65f76c88b4b47d16ae6346fcff",
    }),
    ("AppImage/appimagetool", "1.9.1", "appimagetool", {
        "x86_64": "ed4ce84f0d9caff66f50bcca6ff6f35aae54ce8135408b3fa33abfc3cb384eb0",
        "aarch64": "f0837e7448a0c1e4e650a93bb3e85802546e60654ef287576f46c71c126a9158",
    }),
):
    if arch not in digests:
        raise SystemExit(f"Unsupported architecture: {arch}")
    digest = digests[arch]
    asset = f"{name}-{arch}.AppImage"
    target = destination / name
    if target.exists() and hashlib.sha256(target.read_bytes()).hexdigest() == digest:
        print(f"Already installed: {name} {tag}", flush=True)
        continue
    print(f"Downloading {asset} ({tag})", flush=True)
    with tempfile.TemporaryDirectory(prefix="airpods-driver-appimage-tools-") as temporary:
        download = Path(temporary) / asset
        subprocess.run(
            ["curl", "--http1.1", "--fail", "--location", "--silent", "--show-error", "--retry", "3",
             "--connect-timeout", "20", "--max-time", "900", "--speed-time", "60",
             "--speed-limit", "1024", "--output", str(download),
             f"https://github.com/{repo}/releases/download/{tag}/{asset}"],
            check=True,
        )
        data = download.read_bytes()
        if hashlib.sha256(data).hexdigest() != digest:
            raise SystemExit(f"SHA256 mismatch: {asset}")
        target.write_bytes(data)
    target.chmod(0o755)
    print(f"Installed {name}: {digest}", flush=True)
