FROM ubuntu:24.04
ENV DEBIAN_FRONTEND=noninteractive
RUN apt-get update && apt-get install -y --no-install-recommends \
    build-essential pkg-config libdbus-1-dev libpulse-dev python3 dpkg-dev \
    rpm curl ca-certificates file desktop-file-utils patchelf binutils \
    flatpak dbus-x11 xz-utils zstd git && rm -rf /var/lib/apt/lists/*
ENV CARGO_HOME=/opt/cargo RUSTUP_HOME=/opt/rustup
ENV PATH=/opt/cargo/bin:$PATH
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs -o /tmp/rustup.sh \
    && sh /tmp/rustup.sh -y --profile minimal --default-toolchain 1.93.1 \
    && rm /tmp/rustup.sh
RUN apt-get update && apt-get install -y --no-install-recommends \
    libgl1 libegl1 libxkbcommon0 libxkbcommon-x11-0 libwayland-client0 \
    libx11-6 libx11-xcb1 libxcursor1 libxi6 libxrandr2 xvfb x11-utils libgl1-mesa-dri && rm -rf /var/lib/apt/lists/*
COPY scripts/packaging/install-appimage-tools.py /tmp/install-appimage-tools.py
RUN python3 /tmp/install-appimage-tools.py && rm /tmp/install-appimage-tools.py
WORKDIR /work
