#!/bin/sh
# hermir-capture <flatpak-id> <settle-seconds> <out-dir> [args...]
#
# Installs <flatpak-id> from Flathub into the user installation ($FLATPAK_USER_DIR), starts it once
# under a virtual display with a fresh home, and stops it once every path in $WAIT_FOR (relative
# to ~/.var/app/<id>, space-separated) exists and five more seconds passed, or after
# <settle-seconds>. With $CLOSE_AFTER set, from that many seconds on its windows are asked to
# close every ten seconds (close-windows.py), for emulators that write their settings only on a
# clean quit. Leaves in <out-dir>: `home/` (what it wrote under ~/.var/app), `info`
# (`flatpak info`), `log` and `exit` (124 or 143: still running when stopped; 0: it quit).
#
# Runs the same inside the capture image (ci/capture/Dockerfile, --privileged: bubblewrap cannot
# make its namespaces otherwise) and on a Linux host with flatpak, xvfb-run and dbus-run-session.
set -eu
here=$(cd "$(dirname "$0")" && pwd)
app=$1 settle=$2 out=$3
shift 3
mkdir -p "$out"
# The installation stays where it is when HOME changes below.
export FLATPAK_USER_DIR="${FLATPAK_USER_DIR:-$HOME/.local/share/flatpak}"

flatpak remote-add --user --if-not-exists flathub https://dl.flathub.org/repo/flathub.flatpakrepo
flatpak install --user -y --noninteractive flathub "$app" >&2
flatpak info --user "$app" >"$out/info"

# `flatpak run` talks to the system bus; a bare container has none, and a stopped one leaves
# its socket and pid file behind.
if ! dbus-send --system --dest=org.freedesktop.DBus --print-reply /org/freedesktop/DBus \
    org.freedesktop.DBus.GetId >/dev/null 2>&1; then
    mkdir -p /run/dbus
    rm -f /run/dbus/pid /run/dbus/system_bus_socket
    dbus-daemon --system --fork
fi

# A fresh home, and a runtime directory of our own with a short path: unix socket paths are
# limited to 108 bytes, and the sockets Flatpak's helpers make under it fail beyond that.
home=$(mktemp -d)
rt=$(mktemp -d /tmp/hc.XXXXXX)
chmod 700 "$rt"

status=0
HOME=$home XDG_RUNTIME_DIR=$rt APP=$app SETTLE=$settle WAIT_FOR=${WAIT_FOR:-} \
    CLOSE_AFTER=${CLOSE_AFTER:-} CLOSE="$here/close-windows.py" \
    dbus-run-session -- xvfb-run -a -s "-screen 0 1280x720x24" sh -c '
        timeout -s TERM "$SETTLE" flatpak run --user "$APP" "$@" &
        pid=$!
        elapsed=0
        while kill -0 "$pid" 2>/dev/null; do
            if [ -n "$WAIT_FOR" ]; then
                all=1
                for f in $WAIT_FOR; do
                    [ -e "$HOME/.var/app/$APP/$f" ] || all=0
                done
                if [ "$all" = 1 ]; then
                    sleep 5
                    kill -TERM "$pid" 2>/dev/null || true
                    break
                fi
            fi
            if [ -n "$CLOSE_AFTER" ] && [ "$elapsed" -ge "$CLOSE_AFTER" ] &&
                [ $(((elapsed - CLOSE_AFTER) % 10)) = 0 ]; then
                python3 "$CLOSE" || true
            fi
            sleep 1
            elapsed=$((elapsed + 1))
        done
        wait "$pid"
    ' sh "$@" >"$out/log" 2>&1 || status=$?
echo "$status" >"$out/exit"

rm -rf "$out/home"
mkdir -p "$out/home"
if [ -d "$home/.var" ]; then
    cp -a "$home/.var" "$out/home/"
fi
rm -rf "$home" "$rt"
