# Linux packages and releases

The four downloadable formats are DEB, RPM, AppImage and a single-file Flatpak bundle.
All are distributed as GitHub release files; no application is submitted to a store.

## Application updates

Official packages check this repository's GitHub Releases once a day while the GUI
is running. Preferences → Updates also provides a manual check, preview releases,
and an Update button. Update downloads and verifies the package. When the download
finishes, a separate Install button appears; only clicking Install starts the
system installer and any authorization prompt. The verified file remains cached
while the app is running and is checked again before installation. The main-window
update card renders the GitHub release notes as Markdown, with a bounded scroll
area for long descriptions and an Update button below the notes. The button opens settings
and scrolls to the update controls. Failed
automatic checks are retried no more than once an hour. After successful installation,
the application automatically restarts into the new version and opens its window.
If restarting fails, the installed update is retained and a Restart button allows
another attempt. Headless mode does not check for updates.

The updater selects the current package format and CPU architecture, requires a
newer SemVer version, downloads over HTTPS, and checks the size and SHA256 against
the release's `SHA256SUMS` (and GitHub asset digest when present). These checks
protect download integrity; they are not independent release signatures. The
GitHub repository and its release publishing credentials remain the trust source.

- DEB and RPM use PackageKit over system D-Bus and wait for the transaction's
  completion. PackageKit, a suitable distribution backend, and a desktop PolicyKit
  authentication agent must be available. No APT or RPM repository is needed.
  The app itself runs without root privileges; authorization belongs to PackageKit.
  If a transaction times out, its installer is retained in the update cache until
  its state can be checked in the system package manager.
- AppImage replaces the original file atomically after verification, keeping the
  previous executable as `<original filename>.previous`. The directory must be
  writable. The process then re-executes the original AppImage path with its old
  mount and library environment cleared, so the updated image is mounted afresh.
- Flatpak downloads a bundle into its persistent cache and invokes the host's
  `flatpak install --bundle --or-update` through `flatpak-spawn --host`. It retains
  the current default system or standard per-user installation. Custom Flatpak
  installation paths are currently unsupported. No Flatpak repository is required
  for application updates; runtime dependencies can still require Flathub.
  `--talk-name=org.freedesktop.Flatpak` deliberately permits host command execution
  for this installer, which weakens sandbox isolation. No general host filesystem
  permission is added. Downloads are removed after the installer finishes.
  A host helper waits for the old Flatpak instance to exit before launching the
  updated deployment in the same installation. It intentionally runs without
  `--watch-bus` so it survives the old sandbox closing.

Source builds and `make install` builds do not replace themselves with packages.
The first release containing the updater must be installed manually; earlier
versions cannot acquire this feature on their own. Future package formats require
an explicit installation backend rather than guessing from the Linux distribution.

## Build all formats

```bash
make build-packages
```

All package targets build inside the same Ubuntu 24.04 Docker image used by CI.
The host needs Docker Engine with a running daemon accessible to your user, Bash,
and Make; Rust, Python, RPM, AppImage and Flatpak tools are installed in the image.
No host package installation is needed. `make install-deps` is only for native
application development.

Individual targets are `make build-deb`, `make build-rpm`, `make build-appimage` and
`make build-flatpak`. `make deb` remains an alias. Without Make, run
`bash scripts/packaging/package-docker.sh all` (or `deb`, `rpm`, `appimage`, `flatpak`).
Outputs go into `dist/` and belong to the invoking user. Packages are built for the
Docker engine's native architecture: x86_64 or aarch64.

The image is built automatically with Docker's layer cache and pins Rust to 1.93.1.
Update the Dockerfile explicitly to upgrade the compiler. AppImage tools are
installed at image build time from fixed releases with SHA256 verification.
Named Docker volumes preserve Cargo registry, Git dependency, compiled artifact and
Flatpak runtime caches between runs. Compiled artifacts and runtimes have separate
caches for each architecture. The first build downloads the toolchain, dependencies
and (for Flatpak) the Freedesktop runtime; later builds reuse them.

CI exports the packaging image's Buildx layers to the GitHub Actions cache, with
separate scopes for amd64 and arm64. It loads that image into Docker and sets
`PACKAGING_SKIP_IMAGE_BUILD=1` so both test and packaging steps reuse it.
`PACKAGING_CACHE_DIR` selects host directories for Cargo registry, Git dependencies,
compiled artifacts and Flatpak runtimes instead of named volumes. CI restores and
saves those directories with `actions/cache`; they are not included in packages.
Cargo cache keys include the architecture and exact packaging image ID (including
the compiler and system libraries), then `Cargo.lock` and the commit SHA. Fallback
keys reuse older artifacts only from the same image and architecture. Flatpak has
a separate cache keyed by architecture, runtime configuration and commit SHA.
The first CI run is cold; later runs can reuse caches accessible to their branch.

The source checkout is mounted read-only and selected build inputs are copied into
the container. Host `target/`, Cargo configuration and Rust toolchains are not used.
Output and cache directories are mounted writable. `RELEASE_TAG` is forwarded for release validation;
`PACKAGING_IMAGE` optionally overrides the local image tag.

Flatpak builds and verification use a dedicated privileged container with
`apparmor=unconfined` to allow nested sandbox namespaces. DEB, RPM, AppImage and test
commands do not request these privileges. The host Docker setup must allow the
Flatpak container's nested sandbox; no host Flatpak or D-Bus configuration is needed.
The Docker socket and host Bluetooth/audio devices are not mounted into the container.

RPMs target Fedora; other RPM distributions may use different runtime package names.
The Ubuntu 24.04 environment produces AppImages requiring glibc 2.39 or newer;
they use the desktop host's graphics drivers and core X11/Wayland libraries.
Flatpak packages the same native executable as the other formats. Before export,
the executable is run inside the Freedesktop runtime to check ABI compatibility.
Runtime and permissions are in `packaging/flatpak/package.json`. The resulting bundle is
installed and launched inside the build container for verification, including GUI
startup under Xvfb. No application is published to Flathub.

To run Rust and packaging tests in the same environment:

```bash
bash scripts/packaging/package-docker.sh test
```

## Installation checks

CI builds all four formats on amd64 and arm64, then:

- Installs the actual DEB in clean Ubuntu 24.04 using `apt`, checks its executable,
  shared libraries, launcher and icon, and removes it.
- Installs the actual RPM in clean Fedora 43 using `dnf`, performs the same checks,
  and removes it.
- Executes the AppImage in clean Ubuntu without FUSE, with only base display libraries
  supplied by the system; also checks its extracted launcher and icon. GPU drivers and
  core X11/Wayland libraries are intentionally supplied by the desktop host.
- Installs the Flatpak from its bundle, runs `--version` inside the sandbox,
  checks its exported launcher and icon, and removes it.

The checks also open the actual application window under Xvfb with software rendering.
This catches missing dynamically loaded GUI libraries that a `--version` check would miss.

The commands live in `scripts/verify/verify-installed.sh`, `scripts/verify/verify-appimage.sh`, and
`scripts/verify/verify-flatpak.sh`, with GUI startup in `scripts/verify/verify-gui.sh`.
Flatpak builds also run `scripts/verify/verify-flatpak-update.sh`, which installs
two fixture bundles and verifies that a sandboxed application can update itself
through the host installer and launch the new version. This requires the same
dedicated container and runtime as the other Flatpak checks.
These are installation and startup checks, not Bluetooth
hardware tests. Do not run native package installation checks on a development machine
unless you intend to install the application there.

The workflow follows the build → artifact → tagged GitHub release approach used by
[Aula F75 Max Driver](https://github.com/VitalyArt/Aula-F75-Max-Driver/blob/main/.github/workflows/build.yml).

## Local package

Build the DEB using Docker:

```bash
make build-deb
```

The scripts `build-deb.py`, `build-linux.py` and `build-packages.py` are internal
container entry points; invoking them directly requires all native build tools.

The package is written to `dist/airpods-driver_<version>_<architecture>.deb`.
The version comes from `Cargo.toml`. CI builds amd64 and arm64 separately using
the same Ubuntu 24.04 image. Runtime dependency versions are calculated inside
that image, independently of the developer's host distribution.

The package includes the executable, desktop launcher, dock icon, license, and
user guide. Linked library dependencies are calculated with `dpkg-shlibdeps`;
Bluetooth and dynamically loaded window libraries are declared explicitly.
It does not change Bluetooth or audio configuration, or install an autostart entry.

To inspect a package:

```bash
dpkg-deb --info dist/*.deb
dpkg-deb --contents dist/*.deb
```

## CI and releases

Pushes and pull requests run tests, build all four formats for both architectures, verify package installation,
and upload workflow artifacts. The workflow can also be run manually.
Publishing occurs only for tags beginning with `v` and after both builds succeed.

1. Commit and push the source and workflow files to GitHub.
2. Create and push a release tag:

```bash
git tag v1.0.0
git push origin v1.0.0
```

The tag supplies the release version automatically: `v1.0.0` builds version `1.0.0`.
CI validates the tag before building the image. Inside the private container snapshot,
`prepare-release.py` updates the package version in both `Cargo.toml` and `Cargo.lock`
before tests or compilation. Dependency versions and the host checkout stay unchanged.
The executable's `--version` and all package versions therefore agree with the tag.
Untagged builds use the version committed in `Cargo.toml`. Local release builds can
use the same mechanism: `RELEASE_TAG=v1.0.0 make build-packages`.
A version such as `1.1.0-rc.1` uses tag `v1.1.0-rc.1` and becomes Debian version
`1.1.0~rc.1`, which sorts before `1.1.0`.
Prerelease tags create GitHub prereleases; stable tags create regular releases.
Releases contain all eight packages, `SHA256SUMS`, and automatically generated release notes.
Rerunning the workflow replaces assets on an existing release.

The release job uses GitHub's built-in token with `contents: write`; no personal
access token is needed. GitHub Actions must be enabled for the repository.

Checks inside the packaging container:

```bash
bash scripts/packaging/package-docker.sh test
RELEASE_TAG=v0.1.0 make build-packages
```

## Source archive

Run `make tarball VERSION=0.1.0` with the version from Cargo.toml to create a
source archive in `dist/`, including vendored Rust dependencies. Cargo may need
network access to fetch dependencies. Normal build commands use Makefile.
