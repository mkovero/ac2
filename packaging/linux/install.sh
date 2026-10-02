#!/bin/sh
# Install ac2 from an unpacked release tarball.
#
#   ./install.sh                 # into ~/.local (no root needed)
#   sudo ./install.sh --prefix /usr/local
#   ./install.sh --uninstall     # remove what an install into the same prefix put there
#
# Installs bin/{ac2,ac2d,ac2-ui}, the desktop entry, the icon and a systemd user unit for
# ac2d. It never enables or starts the daemon; the last lines printed say how.
set -eu

here=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
prefix="$HOME/.local"
uninstall=0
while [ $# -gt 0 ]; do
    case "$1" in
        --prefix) prefix=$2; shift 2 ;;
        --prefix=*) prefix=${1#--prefix=}; shift ;;
        --uninstall) uninstall=1; shift ;;
        -h|--help) sed -n '2,10p' "$0"; exit 0 ;;
        *) echo "install.sh: unknown argument $1" >&2; exit 2 ;;
    esac
done

case "$prefix" in
    "$HOME"/*) unitdir="${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user" ;;
    *) unitdir="$prefix/lib/systemd/user" ;;
esac
bindir="$prefix/bin"
appdir="$prefix/share/applications"
icondir="$prefix/share/icons/hicolor"

files="$bindir/ac2 $bindir/ac2d $bindir/ac2-ui $appdir/ac2.desktop $unitdir/ac2d.service \
$icondir/scalable/apps/ac2.svg"
for s in 16 32 48 64 128 256 512; do
    files="$files $icondir/${s}x${s}/apps/ac2.png"
done

if [ "$uninstall" = 1 ]; then
    for f in $files; do
        [ -e "$f" ] && rm -f "$f" && echo "removed $f"
    done
    exit 0
fi

mkdir -p "$bindir" "$appdir" "$unitdir" "$icondir/scalable/apps"
for b in ac2 ac2d ac2-ui; do
    install -m 0755 "$here/bin/$b" "$bindir/$b"
done
install -m 0644 "$here/share/applications/ac2.desktop" "$appdir/ac2.desktop"
install -m 0644 "$here/share/icons/hicolor/scalable/apps/ac2.svg" "$icondir/scalable/apps/ac2.svg"
for s in 16 32 48 64 128 256 512; do
    mkdir -p "$icondir/${s}x${s}/apps"
    install -m 0644 "$here/share/icons/hicolor/${s}x${s}/apps/ac2.png" "$icondir/${s}x${s}/apps/ac2.png"
done
sed "s|@BINDIR@|$bindir|g" "$here/share/systemd/user/ac2d.service" > "$unitdir/ac2d.service"
chmod 0644 "$unitdir/ac2d.service"
command -v update-desktop-database >/dev/null 2>&1 && update-desktop-database "$appdir" 2>/dev/null || true
command -v gtk-update-icon-cache >/dev/null 2>&1 && gtk-update-icon-cache -q "$icondir" 2>/dev/null || true

echo "installed ac2 into $prefix"
case ":$PATH:" in
    *":$bindir:"*) ;;
    *) echo "note: $bindir is not on PATH; add it, or run $bindir/ac2" ;;
esac
echo "start the daemon now and at login:  systemctl --user daemon-reload && systemctl --user enable --now ac2d"
echo "or just open ac2 from the applications menu (it can host its own daemon)."
