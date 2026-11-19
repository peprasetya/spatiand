#!/usr/bin/env bash
# Enter spatial mode, from the desktop, with one click.
#
# The interesting part is self-healing. SteamOS is atomic: an update replaces /usr wholesale,
# which silently removes both the session entry and its launcher. Everything else survives -
# the binary, the calibration, the udev rules - so the only thing lost is registration, and
# the only symptom is that spatial mode quietly stops being offered.
#
# Rather than making that the user's problem, this checks on every launch and re-registers
# when needed. Re-registering needs root, so pkexec is used for a graphical password prompt
# instead of failing with a terminal error nobody will see.
set -uo pipefail

SESSION=/usr/share/wayland-sessions/spatiand.desktop
LAUNCHER=/usr/local/bin/spatiand-session
# An installed release has its programs in bin, beside this script's tools (install-release.sh);
# otherwise this is the development checkout's, and the session runs its build.
HERE="$(cd "$(dirname "$(readlink -f "$0")")" && pwd)"
if [[ -x "$HERE/../bin/spatiand" ]]; then
    RELEASE=true
    TOOLS="$HERE"
    BIN="$(cd "$HERE/../bin" && pwd)/spatiand"
else
    RELEASE=false
    REPO="${SPATIAND_REPO:-$HOME/spatiand}"
    TOOLS="$REPO/tools"
    BIN="$REPO/target/release/spatiand"
fi

fail() {
    if command -v kdialog >/dev/null; then
        kdialog --title "Spatiand" --error "$1"
    else
        echo "$1" >&2
    fi
    exit 1
}

if [[ "$RELEASE" == true ]]; then
    [[ -x "$BIN" ]] || fail "Spatiand is not installed.\n\nExpected: $BIN\n\nInstall it again: run install.sh in the folder it came in."
else
    [[ -x "$BIN" ]] || fail "Spatiand is not built yet.\n\nExpected: $BIN\n\nBuild it with:\ndistrobox enter --name holo -- bash -c 'cd ~/spatiand && cargo build --release'"
fi

# A binary that cannot resolve its libraries dies before it prints anything, and since this
# switches to a session with no desktop underneath, "dies before printing anything" looks
# exactly like a black screen that never comes back. The usual cause is a build container
# whose glibc has drifted ahead of the host's: it compiles and links perfectly and then fails
# at the first instruction.
#
# `ldd` resolves the same way the loader does, so this catches it before the screen goes away.
missing=$(ldd "$BIN" 2>&1 | grep -E 'not found' | head -4)
if [[ -n "$missing" && "$RELEASE" == true ]]; then
    fail "Spatiand cannot run on this system.\n\n$missing\n\nThis release was built for an \
older or newer SteamOS than this one (glibc $(ldd --version | head -1 | grep -oE '[0-9]+\.[0-9]+$') here). \
Install the release made for this SteamOS."
elif [[ -n "$missing" ]]; then
    fail "Spatiand cannot run on this system.\n\n$missing\n\nThis usually means it was built \
against newer libraries than SteamOS has. Rebuild it in a container whose glibc is no newer \
than this machine's ($(ldd --version | head -1 | grep -oE '[0-9]+\.[0-9]+$')):\n\n\
distrobox enter --name holo -- bash -c 'cd ~/spatiand && cargo build --release'"
fi

# Missing, merely out of date, or running another Spatiand than this one.
#
# "Missing" used to be the only case this looked for, which meant an entry that was present
# and stale was left alone forever -- and a stale entry is not a theoretical worry: the one
# before this gained KDE in DesktopNames, without which no Flatpak file chooser opens. The
# revision is written into the entry by install-session.sh and read out of that same script
# here, so there is one number and it lives with the thing it describes. The launcher names the
# program it runs: an installed release's button starts the release, a checkout's its build.
want_revision=$(grep -oE '^ENTRY_REVISION=[0-9]+' "$TOOLS/install-session.sh" |
    head -1 | cut -d= -f2)
have_revision=$(grep -oE '^X-Spatiand-Revision=[0-9]+' "$SESSION" 2>/dev/null |
    head -1 | cut -d= -f2)
if [[ ! -f "$SESSION" || ! -x "$LAUNCHER" ]] || ! grep -qF "\"$BIN\"" "$LAUNCHER" ||
    [[ -n "$want_revision" && ${have_revision:-0} -lt $want_revision ]]; then
    if command -v kdialog >/dev/null; then
        kdialog --title "Spatiand" --msgbox \
"Spatial mode needs to be registered.

Either a SteamOS update replaced the system partition, which removes the
session entry, or this version of Spatiand needs a newer one, or the entry
starts another copy of Spatiand than this one. Your settings, calibration
and the app itself are untouched.

You will be asked for your password."
    fi
    pkexec env SPATIAND_USER="$USER" SPATIAND_BIN="$BIN" bash "$TOOLS/install-session.sh" \
        || fail "Could not register spatial mode.\n\nRun this in a terminal to see why:\nsudo SPATIAND_BIN=$BIN $TOOLS/install-session.sh"
    [[ -f "$SESSION" ]] || fail "Registration reported success but $SESSION is still missing."
fi

# steamosctl owns the SDDM autologin drop-in; writing that file by hand does not work,
# because SteamOS regenerates it and silently ignores hand-edits.
steamosctl set-default-desktop-session spatiand.desktop >/dev/null 2>&1
steamosctl switch-to-desktop-mode spatiand.desktop >/dev/null 2>&1 \
    || fail "Could not switch sessions.\n\nsteamosctl refused. Try again, or reboot."
