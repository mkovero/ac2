# Wine + wineasio + JACK2 (dummy driver) + a Rust toolchain that cross-compiles
# x86_64-pc-windows-msvc with cargo-xwin, so the real cpal ASIO host code path runs on Linux.
#
# The Steinberg ASIO SDK is downloaded here at image build time and never enters the repo.
# Nothing in this image ever opens a real sound device: JACK runs with `-d dummy`.

# debian:trixie-slim as of 2026-10-05
FROM debian@sha256:a29215f6a35e51e22adffa17f89e9d2ef06214e64a2bad10d765c46aea49f11f

ARG WINE_VERSION=10.0~repack-6
ARG JACK_VERSION=1.9.22~dfsg-4
ARG LLVM_VERSION=1:19.0-63
ARG WINEASIO_VERSION=1.3.0
ARG WINEASIO_SHA256=91267e22f2684b27a1d656021c52e1ff05f7cc9241086ae526bff551410b9325
ARG ASIO_SDK_URL=https://download.steinberg.net/sdk_downloads/ASIO-SDK_2.3.4_2025-10-15.zip
ARG ASIO_SDK_SHA256=d5ebf0c20dd2c5f43771fd0c1418f4b361bf52434ee670097cfa6b3a335e2eca
ARG RUST_VERSION=1.95.0
ARG CARGO_XWIN_VERSION=0.23.1

ENV DEBIAN_FRONTEND=noninteractive

RUN apt-get update && apt-get install -y --no-install-recommends \
        ca-certificates curl unzip make gcc libc6-dev procps xvfb xauth \
        wine=${WINE_VERSION} wine64=${WINE_VERSION} libwine=${WINE_VERSION} \
        libwine-dev=${WINE_VERSION} wine64-tools=${WINE_VERSION} \
        jackd2=${JACK_VERSION} libjack-jackd2-dev=${JACK_VERSION} \
        libjack-jackd2-0=${JACK_VERSION} \
        clang=${LLVM_VERSION} lld=${LLVM_VERSION} llvm=${LLVM_VERSION} \
        libclang-dev=${LLVM_VERSION} \
    && rm -rf /var/lib/apt/lists/*

# wineasio from source, 64-bit only: PE side (wineasio64.dll) plus unix side
# (wineasio64.dll.so, which links libjack).
RUN curl -fsSL -o /tmp/wineasio.tar.gz \
        https://github.com/wineasio/wineasio/archive/refs/tags/v${WINEASIO_VERSION}.tar.gz \
    && echo "${WINEASIO_SHA256}  /tmp/wineasio.tar.gz" | sha256sum -c - \
    && mkdir /opt/wineasio-src && tar xzf /tmp/wineasio.tar.gz -C /opt/wineasio-src --strip-components=1 \
    && make -C /opt/wineasio-src 64 WINEBUILD_INCLUDEDIR=/usr/include/wine/wine/windows \
    && install -m 644 /opt/wineasio-src/build64/wineasio64.dll \
        /usr/lib/x86_64-linux-gnu/wine/x86_64-windows/wineasio64.dll \
    && install -m 644 /opt/wineasio-src/build64/wineasio64.dll.so \
        /usr/lib/x86_64-linux-gnu/wine/x86_64-unix/wineasio64.dll.so \
    && rm /tmp/wineasio.tar.gz

# ASIO SDK, fetched from Steinberg's server (license: not redistributable in the repo).
RUN curl -fsSL -o /tmp/asiosdk.zip "${ASIO_SDK_URL}" \
    && echo "${ASIO_SDK_SHA256}  /tmp/asiosdk.zip" | sha256sum -c - \
    && unzip -q /tmp/asiosdk.zip -d /opt \
    && mv /opt/ASIOSDK /opt/asiosdk \
    && rm /tmp/asiosdk.zip
ENV CPAL_ASIO_DIR=/opt/asiosdk

# Rust + cargo-xwin (prebuilt static binary, checksummed).
ENV RUSTUP_HOME=/opt/rustup CARGO_HOME=/opt/cargo PATH=/opt/cargo/bin:$PATH
RUN curl -fsSL https://sh.rustup.rs | sh -s -- -y --profile minimal \
        --default-toolchain ${RUST_VERSION} --component rustfmt --target x86_64-pc-windows-msvc \
    && curl -fsSL -o /tmp/xwin.tgz \
        https://github.com/rust-cross/cargo-xwin/releases/download/v${CARGO_XWIN_VERSION}/cargo-xwin-v${CARGO_XWIN_VERSION}.x86_64-unknown-linux-musl.tar.gz \
    && curl -fsSL -o /tmp/xwin.tgz.sha256 \
        https://github.com/rust-cross/cargo-xwin/releases/download/v${CARGO_XWIN_VERSION}/cargo-xwin-v${CARGO_XWIN_VERSION}.x86_64-unknown-linux-musl.tar.gz.sha256 \
    && echo "$(cut -d' ' -f1 /tmp/xwin.tgz.sha256)  /tmp/xwin.tgz" | sha256sum -c - \
    && tar xzf /tmp/xwin.tgz -C /opt/cargo/bin \
    && rm /tmp/xwin.tgz /tmp/xwin.tgz.sha256

# cargo-xwin drives clang-cl / lld-link / llvm-lib; Debian ships them versioned.
RUN for t in clang-cl lld-link llvm-lib llvm-rc; do \
        ln -sf /usr/bin/${t}-19 /usr/local/bin/${t} 2>/dev/null || true; \
        [ -e /usr/local/bin/${t} ] || ln -sf /usr/lib/llvm-19/bin/${t} /usr/local/bin/${t}; \
    done
# asio-sys runs vcvarsall.bat unless VCINSTALLDIR is set; cargo-xwin already provides the
# MSVC CRT and Windows SDK, so the variable only has to exist.
ENV VCINSTALLDIR=/opt/xwin-cache \
    XWIN_CACHE_DIR=/opt/xwin-cache \
    XWIN_ACCEPT_LICENSE=1 \
    CARGO_BUILD_JOBS=4

# 64-bit Wine prefix with wineasio registered. wineasio reads its settings from
# HKCU\Software\Wine\WineASIO: 2 in / 2 out, and no autoconnect to JACK's (dummy) system
# ports, so nothing the probe writes leaves the wineasio client unless run.sh connects it.
ENV WINEARCH=win64 WINEPREFIX=/opt/wineprefix WINEDEBUG=-all
RUN wineboot -i && wineserver -w \
    && cp /usr/lib/x86_64-linux-gnu/wine/x86_64-windows/wineasio64.dll \
        ${WINEPREFIX}/drive_c/windows/system32/ \
    && wine regsvr32 wineasio64.dll \
    && wine reg add 'HKCU\Software\Wine\WineASIO' /v 'Number of inputs' /t REG_DWORD /d 2 /f \
    && wine reg add 'HKCU\Software\Wine\WineASIO' /v 'Number of outputs' /t REG_DWORD /d 2 /f \
    && wine reg add 'HKCU\Software\Wine\WineASIO' /v 'Connect to hardware' /t REG_DWORD /d 0 /f \
    && wine reg add 'HKCU\Software\Wine\WineASIO' /v 'Autostart server' /t REG_DWORD /d 0 /f \
    && wineserver -w

# Second target, to check that azo (no SDK, no C++) also builds with MinGW.
ARG MINGW_VERSION=14.2.0-19+27+b1
RUN apt-get update && apt-get install -y --no-install-recommends \
        gcc-mingw-w64-x86-64-posix=${MINGW_VERSION} \
    && rm -rf /var/lib/apt/lists/* \
    && rustup target add x86_64-pc-windows-gnu
ENV CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER=x86_64-w64-mingw32-gcc-posix

# cargo-xwin fetches and unpacks the MSVC CRT and Windows SDK (slow, ~1 GB) on first use;
# its own layer so probe changes do not repeat it.
RUN cargo new --quiet --bin /tmp/warm && cd /tmp/warm \
    && cargo xwin build --release --target x86_64-pc-windows-msvc \
    && rm -rf /tmp/warm

# Pre-build the probe's dependencies, so run.sh builds offline and quickly. The probe source
# itself is mounted at run time.
COPY probe/Cargo.toml probe/Cargo.lock /opt/probe-seed/
RUN mkdir -p /opt/probe-seed/src/bin && cd /opt/probe-seed \
    && echo 'fn main() {}' > src/bin/azo_probe.rs && cp src/bin/azo_probe.rs src/bin/cpal_probe.rs \
    && cargo fetch --target x86_64-pc-windows-msvc && cargo fetch --target x86_64-pc-windows-gnu \
    && cargo xwin build --release --target x86_64-pc-windows-msvc --target-dir /tmp/seed-target \
    && rm -rf /opt/probe-seed /tmp/seed-target

# jack_lsp / jack_connect for run.sh (loopback connection inside the dummy graph).
ARG JACK_TOOLS_VERSION=4-4
RUN apt-get update && apt-get install -y --no-install-recommends \
        jack-example-tools=${JACK_TOOLS_VERSION} \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /work

# jq reads ac2's --json replies in ac2.sh.
ARG JQ_VERSION=1.7.1-6+deb13u4
RUN apt-get update && apt-get install -y --no-install-recommends jq=${JQ_VERSION} \
    && rm -rf /var/lib/apt/lists/*
