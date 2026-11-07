.PHONY: help install-deps install install-desktop build build-debug build-deb deb build-rpm build-appimage build-flatpak build-packages run run-debug check test clean

help:
	@printf '%s\n' \
		'make install-deps  Install build dependencies on Ubuntu' \
		'make install       Install the release build and desktop icon for the current user' \
		'make install-desktop  Install the desktop launcher and dock icon without building' \
		'make build         Build the release binary' \
		'make build-debug   Build the debug binary' \
		'make build-deb     Build a Debian package in Docker into dist/' \
		'make build-rpm     Build an RPM for Fedora in dist/' \
		'make build-appimage  Build a portable AppImage in dist/' \
		'make build-flatpak Build a standalone Flatpak bundle in dist/' \
		'make build-packages  Build all four formats in Docker into dist/' \
		'make run           Build and run release (pass arguments with ARGS="...")' \
		'make run-debug     Build and run debug (pass arguments with ARGS="...")' \
		'make check         Check the Rust project' \
		'make test          Run tests' \
		'make clean         Remove Cargo build artifacts'

install-deps:
	sudo apt update
	sudo apt install cargo rustc build-essential pkg-config libdbus-1-dev libpulse-dev python3 dpkg-dev

build:
	cargo build --release --locked

install: build install-desktop
	install -Dm755 target/release/airpods-driver $(DESTDIR)$(HOME)/.local/bin/airpods-driver

install-desktop:
	install -Dm644 assets/icon.png $(DESTDIR)$(HOME)/.local/share/icons/hicolor/256x256/apps/com.vitalyart.airpods-driver.png
	install -Dm644 packaging/desktop/com.vitalyart.airpods-driver.desktop $(DESTDIR)$(HOME)/.local/share/applications/com.vitalyart.airpods-driver.desktop

build-debug:
	cargo build --locked

build-deb:
	bash scripts/packaging/package-docker.sh deb

build-rpm:
	bash scripts/packaging/package-docker.sh rpm

build-appimage:
	bash scripts/packaging/package-docker.sh appimage

build-flatpak:
	bash scripts/packaging/package-docker.sh flatpak

build-packages:
	bash scripts/packaging/package-docker.sh all

run:
	cargo run --release --locked -- $(ARGS)

run-debug:
	cargo run --locked -- $(ARGS)

check:
	cargo check --locked

test:
	cargo test --locked

clean:
	cargo clean

.PHONY: tarball
tarball:
	@test -n "$(VERSION)" || { echo 'Usage: make tarball VERSION=0.1.0' >&2; exit 2; }
	python3 scripts/packaging/build-deb.py --validate-version
	mkdir -p dist
	@set -eu; stage=$$(mktemp -d); trap 'rm -rf "$$stage"' EXIT; \
	root="$$stage/airpods-driver-v$(VERSION)"; mkdir -p "$$root/.cargo"; \
	test "$(VERSION)" = "$$(python3 -c 'import tomllib; print(tomllib.load(open("Cargo.toml", "rb"))["package"]["version"])')"; \
	cargo vendor --locked "$$root/vendor" > "$$root/.cargo/config.toml"; \
	cp -a Cargo.toml Cargo.lock LICENSE README.md Makefile src assets packaging scripts tests docs "$$root/"; \
	python3 -c 'from pathlib import Path; import sys; p=Path(sys.argv[1]); p.write_text(p.read_text().replace(sys.argv[2], "vendor"))' "$$root/.cargo/config.toml" "$$root/vendor"; \
	tar -czf "dist/airpods-driver-v$(VERSION)-source.tar.gz" -C "$$stage" "airpods-driver-v$(VERSION)"
