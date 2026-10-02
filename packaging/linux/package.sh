#!/usr/bin/env bash
# Linux release artifacts from built binaries:
#   ac2-<version>-linux-x86_64.tar.gz   ac2, ac2d, ac2-ui + desktop entry, icons, systemd
#                                       user unit, install.sh, README, LICENSE, docs
#   ac2-ui-<version>-x86_64.AppImage    the desktop app (with its embedded daemon)
#
#   packaging/linux/package.sh <version> <dir with release binaries> <out dir>
#
# The AppImage bundles no shared libraries: the binaries need glibc and libstdc++, which
# every desktop has, and libjack.so.0, which must be the system's (JACK2's, or PipeWire's
# from pipewire-jack) to reach the system's server. No ALSA: nothing links libasound. GPU
# and windowing libraries are opened at run time.
set -euo pipefail

version=$1
bins=$2
out=$3
root=$(cd "$(dirname "$0")/../.." && pwd)
arch=x86_64
mkdir -p "$out"
out=$(cd "$out" && pwd)
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

# --- tarball --------------------------------------------------------------------------
name="ac2-$version-linux-$arch"
d="$work/$name"
mkdir -p "$d/bin" "$d/share/applications" "$d/share/systemd/user" \
    "$d/share/icons/hicolor/scalable/apps" "$d/docs"
for b in ac2 ac2d ac2-ui; do
    install -m 0755 "$bins/$b" "$d/bin/$b"
done
install -m 0644 "$root/packaging/linux/ac2.desktop" "$d/share/applications/"
install -m 0644 "$root/packaging/linux/ac2d.service" "$d/share/systemd/user/"
install -m 0644 "$root/packaging/icon/ac2.svg" "$d/share/icons/hicolor/scalable/apps/"
for s in 16 32 48 64 128 256 512; do
    mkdir -p "$d/share/icons/hicolor/${s}x${s}/apps"
    install -m 0644 "$root/packaging/icon/generated/ac2-$s.png" \
        "$d/share/icons/hicolor/${s}x${s}/apps/ac2.png"
done
install -m 0755 "$root/packaging/linux/install.sh" "$d/"
install -m 0644 "$root/README.md" "$root/LICENSE" "$d/"
install -m 0644 "$root/docs/install.md" "$root/docs/user-guide.md" "$root/docs/protocol.md" "$d/docs/"
tar -C "$work" --owner=0 --group=0 --sort=name -czf "$out/$name.tar.gz" "$name"
echo "wrote $out/$name.tar.gz"

# --- AppImage -------------------------------------------------------------------------
tools=${AC2_TOOLS_DIR:-"$root/target/packaging-tools"}
mkdir -p "$tools"
fetch() { # url sha256 dest
    if [ ! -f "$3" ] || ! echo "$2  $3" | sha256sum -c --quiet - 2>/dev/null; then
        curl -fsSL --retry 5 -o "$3.part" "$1"
        echo "$2  $3.part" | sha256sum -c --quiet -
        mv "$3.part" "$3"
    fi
}
fetch https://github.com/AppImage/appimagetool/releases/download/1.9.1/appimagetool-x86_64.AppImage \
    ed4ce84f0d9caff66f50bcca6ff6f35aae54ce8135408b3fa33abfc3cb384eb0 "$tools/appimagetool"
fetch https://github.com/AppImage/type2-runtime/releases/download/20251108/runtime-x86_64 \
    2fca8b443c92510f1483a883f60061ad09b46b978b2631c807cd873a47ec260d "$tools/runtime-x86_64"
chmod +x "$tools/appimagetool"

app="$work/ac2.AppDir"
mkdir -p "$app/usr/bin" "$app/usr/share/applications" "$app/usr/share/icons/hicolor/256x256/apps"
install -m 0755 "$bins/ac2-ui" "$app/usr/bin/ac2-ui"
install -m 0644 "$root/packaging/linux/ac2.desktop" "$app/usr/share/applications/ac2.desktop"
install -m 0644 "$root/packaging/linux/ac2.desktop" "$app/ac2.desktop"
install -m 0644 "$root/packaging/icon/generated/ac2-256.png" "$app/usr/share/icons/hicolor/256x256/apps/ac2.png"
install -m 0644 "$root/packaging/icon/generated/ac2-256.png" "$app/ac2.png"
ln -s usr/bin/ac2-ui "$app/AppRun"
# No FUSE on CI runners: run the tool from its extracted image.
APPIMAGE_EXTRACT_AND_RUN=1 ARCH=$arch VERSION=$version "$tools/appimagetool" \
    --no-appstream --runtime-file "$tools/runtime-x86_64" \
    "$app" "$out/ac2-ui-$version-$arch.AppImage"
echo "wrote $out/ac2-ui-$version-$arch.AppImage"
