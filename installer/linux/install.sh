#!/bin/sh
# Install PinkDown for the current user (default) or system-wide (--system).
#
# Usage:
#   ./install.sh              per-user install into ~/.local (no root needed)
#   ./install.sh --system     system-wide into /usr/local (uses sudo if needed)
#   ./install.sh --prefix DIR install under DIR/{bin,share}
#
# Expects the release layout (pinkdown, pinkdown.desktop,
# pinkdown-linux-icon.png, install.sh in one directory). When run from a source
# checkout it falls back to target/release/pinkdown and assets/.
set -eu

system=0
prefix=""
while [ $# -gt 0 ]; do
    case "$1" in
        --system) system=1 ;;
        --prefix)
            shift
            prefix="${1:-}"
            if [ -z "$prefix" ]; then
                echo "install.sh: --prefix needs a directory" >&2
                exit 2
            fi
            ;;
        -h|--help)
            sed -n '2,12p' "$0" | sed 's/^# \{0,1\}//'
            exit 0
            ;;
        *)
            echo "install.sh: unknown option '$1' (try --help)" >&2
            exit 2
            ;;
    esac
    shift
done

script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)

binary=""
for candidate in "$script_dir/pinkdown" "$script_dir/../../target/release/pinkdown"; do
    if [ -x "$candidate" ]; then
        binary=$candidate
        break
    fi
done
if [ -z "$binary" ]; then
    echo "install.sh: no 'pinkdown' binary next to install.sh (or in target/release)" >&2
    exit 1
fi

icon=""
for candidate in "$script_dir/pinkdown-linux-icon.png" "$script_dir/../../assets/pinkdown-linux-icon.png"; do
    if [ -f "$candidate" ]; then
        icon=$candidate
        break
    fi
done
if [ -z "$icon" ]; then
    echo "install.sh: pinkdown-linux-icon.png not found next to install.sh (or in assets/)" >&2
    exit 1
fi

desktop_file="$script_dir/pinkdown.desktop"
if [ ! -f "$desktop_file" ]; then
    desktop_file="$script_dir/../../installer/linux/pinkdown.desktop"
fi
if [ ! -f "$desktop_file" ]; then
    echo "install.sh: pinkdown.desktop not found next to install.sh (or in installer/linux/)" >&2
    exit 1
fi

if [ -n "$prefix" ]; then
    bindir="$prefix/bin"
    datadir="$prefix/share"
elif [ "$system" -eq 1 ]; then
    bindir="/usr/local/bin"
    datadir="/usr/local/share"
else
    bindir="${HOME}/.local/bin"
    datadir="${HOME}/.local/share"
fi

appsdir="$datadir/applications"
iconsdir="$datadir/icons/hicolor/512x512/apps"

# Use sudo only when the destination is not writable by the current user.
as_root() {
    if "$@" 2>/dev/null; then
        return 0
    fi
    command -v sudo >/dev/null 2>&1 || {
        echo "install.sh: cannot write to $1 and sudo is unavailable" >&2
        exit 1
    }
    sudo "$@"
}

as_root mkdir -p "$bindir" "$appsdir" "$iconsdir"
as_root install -m 755 "$binary" "$bindir/pinkdown"
as_root install -m 644 "$icon" "$iconsdir/pinkdown.png"

# Rewrite Exec to the installed absolute path so the launcher works even when
# bindir is not on PATH.
installed_desktop=$(mktemp "${TMPDIR:-/tmp}/pinkdown-desktop.XXXXXX")
trap 'rm -f "$installed_desktop"' EXIT
sed "s|^Exec=.*|Exec=$bindir/pinkdown %f|" "$desktop_file" >"$installed_desktop"
as_root install -m 644 "$installed_desktop" "$appsdir/pinkdown.desktop"

command -v update-desktop-database >/dev/null 2>&1 && \
    update-desktop-database "$appsdir" >/dev/null 2>&1 || true
command -v gtk-update-icon-cache >/dev/null 2>&1 && \
    gtk-update-icon-cache -q -t "$datadir/icons/hicolor" 2>/dev/null || true

# Claim .md for PinkDown in the current user's mimeapps (user installs only).
if [ "$system" -eq 0 ] && [ -z "$prefix" ] && command -v xdg-mime >/dev/null 2>&1; then
    xdg-mime default pinkdown.desktop text/markdown 2>/dev/null || true
    xdg-mime default pinkdown.desktop text/x-markdown 2>/dev/null || true
fi

echo "PinkDown installed:"
echo "  binary : $bindir/pinkdown"
echo "  launcher: $appsdir/pinkdown.desktop"
echo "  icon   : $iconsdir/pinkdown.png"
case ":$PATH:" in
    *":$bindir:"*) ;;
    *) echo "note: $bindir is not on PATH; add it to launch 'pinkdown' from a shell." ;;
esac
