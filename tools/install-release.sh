#!/usr/bin/env bash
# Install Spatiand from an unpacked release folder (tools/make-release.sh makes it), or update the
# one installed: run it again from a newer release.
#
#   ./install.sh [--yes] [--no-system]
#
#   --yes        do the steps that need your password without asking first, the glasses' rules too
#   --no-system  only what goes in your home: nothing that needs your password
#
# Everything lands in your home but what a system keeps for all its users, because SteamOS replaces
# /usr wholesale on every update. The programs, the browser and the menu entries go to
# ~/.local/share/spatiand/release and the menus; the session entry ("Spatial Mode" at the login
# screen) has to be on /usr, needs your password once, and is the one thing an update takes away
# -- "Go To Spatiand" puts it back when it is gone. Rules that let Spatiand open the glasses outside
# a session (tools/install-udev-rules.sh says when that matters) are offered, never assumed.
#
# DESTDIR=<folder> writes those system files under that folder instead, with no password: to see
# what would be installed, or to try the installer on a machine whose own session must not change.
set -uo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
FROM="$HERE/spatiand"
DEST="$HOME/.local/share/spatiand/release"
ROOT="${DESTDIR:-}"

yes=false
system=true
for arg in "$@"; do
    case "$arg" in
        --yes) yes=true ;;
        --no-system) system=false ;;
        -h|--help) sed -n '2,19p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
        *) echo "!! unknown option: $arg (see --help)" >&2; exit 2 ;;
    esac
done

say() { printf '\n== %s ==\n' "$1"; }
die() { printf '\n!! %s\n' "$1" >&2; exit 1; }
# A question answered yes by --yes, and no when there is nobody at a terminal to answer it.
ask() {
    $yes && return 0
    [ -t 0 ] || return 1
    local answer
    read -rp "$1 [y/N] " answer
    [[ "$answer" == [yY]* ]]
}
# The processes running a program from under a folder: "pid program arguments", one a line, the
# program as the system knows it (not as it was called).
running_under() {
    local p exe
    for p in /proc/[0-9]*; do
        exe=$(readlink "$p/exe" 2>/dev/null) || continue
        [[ "$exe" == "$1/"* ]] || continue
        printf '%s %s %s\n' "${p#/proc/}" "${exe% (deleted)}" "$(tr '\0' ' ' <"$p/cmdline" 2>/dev/null | cut -d' ' -f2-)"
    done
}
# What make-release.sh wrote of the release: version, commit, glibc.
built() { sed -n "s/^$1=//p" "$2/VERSION" 2>/dev/null; }

[ -x "$FROM/bin/spatiand" ] || die "no spatiand folder beside this script: unpack the whole release, then run its install.sh"

say "checking this machine"
[ "$(uname -m)" = x86_64 ] || die "this release is for a Steam Deck (x86_64), and this machine is $(uname -m)"
[ -e /dev/dri/renderD128 ] || [ -e /dev/dri/card0 ] || die "no graphics device in /dev/dri"
if ! command -v steamosctl >/dev/null; then
    printf 'note: this does not look like SteamOS. Spatiand may still run, but "Go To Spatiand"\n'
    printf '      will not know how to switch sessions.\n'
fi
# Every program's libraries, as the loader will find them, before anything is changed: a program
# that cannot load dies before it says anything, which in a session with no desktop under it
# looks like a black screen that never comes back.
missing=$(for f in "$FROM"/bin/* "$FROM"/lib/spatimesh/spatimesh; do
    ldd "$f" 2>/dev/null | grep 'not found' | sed "s|^[[:space:]]*|$(basename "$f"): |"
done | sort -u)
[ -z "$missing" ] || die "this release needs libraries this system does not have:
$missing
It is built for SteamOS $(built steamos "$FROM") (glibc $(built glibc "$FROM")); this one has glibc $(ldd --version | head -1 | grep -oE '[0-9]+\.[0-9]+$')."

# The room itself, from what is about to be replaced (its services are the same program, with
# --spatikey-service or --dictation-service): it would lose its programs under it.
while read -r pid exe args; do
    if [ "$exe" = "$DEST/bin/spatiand" ] && [[ "$args" != *-service* ]]; then
        die "Spatial Mode is running from the copy installed here (pid $pid): leave it first (hold the exit row on the Deck's screen), then run this again"
    fi
done < <(running_under "$DEST/bin")

say "installing to $DEST"
# Services running from an earlier install would go on running the old programs until the next
# login: stopped here, and started again from the new ones below.
dictation_was_on=false
while read -r pid exe args; do
    case "$exe $args" in
        *--dictation-service*) dictation_was_on=true ;;
        *--spatikey-service*|*/spatiand-storage*) ;;
        *) continue ;;
    esac
    echo "stopping $(basename "$exe") $args(pid $pid), to start the new one"
    kill "$pid" 2>/dev/null
done < <(running_under "$DEST/bin")
if $dictation_was_on; then systemctl --user stop spatiand-dictation 2>/dev/null; fi

# Copied whole beside the old one and then swapped in, so a failure halfway leaves the old one
# whole, and windows open from it keep the programs they were started from.
rm -rf "$DEST.new" "$DEST.old"
install -d "$(dirname "$DEST")"
cp -a "$FROM" "$DEST.new" || { rm -rf "$DEST.new"; die "could not copy the programs to $DEST.new (is the disk full?)"; }
if [ -e "$DEST" ]; then mv "$DEST" "$DEST.old" || die "could not move the old $DEST aside"; fi
mv "$DEST.new" "$DEST" || die "could not put the new programs at $DEST"
rm -rf "$DEST.old"
# What earlier releases left in the folder above: the room's program and icon on their own
# (2026-09), and SpatiMesh's WPE WebKit (until 2026-11-10).
data="$(dirname "$DEST")"
[ -f "$data/spatiand" ] && rm -f "$data/spatiand" "$data/spatiand.svg"
rm -rf "$data/wpe" "$data/spatimesh-wpe"
echo "installed Spatiand $(built version "$DEST") ($(built commit "$DEST"))"

say "adding Spatiand's applications to the menu"
bash "$DEST/tools/install-desktop-apps.sh" "$DEST/bin" || die "could not add the menu entries"
if $dictation_was_on; then "$HOME/.local/share/spatiand/dictation.sh" start || true; fi

say "adding Go To Spatiand to the desktop and the menu"
DESKTOP_DIR="${XDG_DESKTOP_DIR:-$(xdg-user-dir DESKTOP 2>/dev/null || echo "$HOME/Desktop")}"
# The September release's own launcher, which switched sessions without checking the entry.
old="$DESKTOP_DIR/Go To Spatiand.desktop"
if [ -f "$old" ] && grep -q 'switch-to-desktop-mode spatiand.desktop' "$old"; then rm -f "$old"; fi
XDG_DESKTOP_DIR="$DESKTOP_DIR" bash "$DEST/tools/install-desktop-link.sh" >/dev/null || die "could not add Go To Spatiand"
echo "on the desktop and in the menu, under System"

# The session entry: written when it is not there, is out of date, or starts another copy.
LAUNCHER="$ROOT/usr/local/bin/spatiand-session"
SESSION="$ROOT/usr/share/wayland-sessions/spatiand.desktop"
RULES="$ROOT/etc/udev/rules.d/99-spatiand.rules"
want_revision=$(grep -oE '^ENTRY_REVISION=[0-9]+' "$DEST/tools/install-session.sh" | head -1 | cut -d= -f2)
have_revision=$(grep -oE '^X-Spatiand-Revision=[0-9]+' "$SESSION" 2>/dev/null | head -1 | cut -d= -f2)
session=true
if [ -x "$LAUNCHER" ] && grep -qF "\"$DEST/bin/spatiand\"" "$LAUNCHER" && [ "${have_revision:-0}" -ge "${want_revision:-0}" ]; then
    session=false
fi
udev=false

if ! $system; then
    say "nothing outside your home (--no-system)"
    echo "Spatial Mode is not at the login screen until this is run again without --no-system,"
    echo "or \"Go To Spatiand\" is clicked (it asks for your password and registers it)."
else
    say "what needs your password"
    if $session; then
        cat <<'EOF'
"Spatial Mode", the session Spatiand runs in, is registered outside your home:
/usr/local/bin/spatiand-session and /usr/share/wayland-sessions/spatiand.desktop.
A SteamOS update removes them; "Go To Spatiand" puts them back when they are gone.
EOF
    else
        echo "Spatial Mode is registered already, for this copy."
    fi
    if [ -f "$RULES" ]; then
        echo "The glasses' udev rules are installed already ($RULES)."
    else
        cat <<'EOF'

Optional: udev rules (/etc/udev/rules.d/99-spatiand.rules) that let Spatiand open the glasses
and the Deck's controller, and set the Deck's brightness from its panel, also outside a login
session -- when it is started over ssh, say. Spatial Mode works without them.
EOF
        ask "Install the udev rules too?" && udev=true
    fi
    if $session || $udev; then
        # One script, so that one password does both.
        me="${USER:-$(id -un)}"
        script="set -e"
        $session && script+=$'\n'"$(printf 'env SPATIAND_USER=%q SPATIAND_BIN=%q bash %q' "$me" "$DEST/bin/spatiand" "$DEST/tools/install-session.sh")"
        $udev && script+=$'\n'"$(printf 'env SPATIAND_USER=%q bash %q' "$me" "$DEST/tools/install-udev-rules.sh")"
        if [ -n "$ROOT" ]; then
            echo "(under $ROOT, with no password: DESTDIR)"
            DESTDIR="$ROOT" bash -c "$script"
        else
            echo
            echo "Your password is asked for now. (A Steam Deck has none until one is set: if you never"
            echo "set one, cancel, run \"passwd\" in Konsole, and run this installer again.)"
            status=127
            command -v pkexec >/dev/null && { pkexec bash -c "$script"; status=$?; }
            # 127: no agent to ask with (no desktop, as over ssh); a terminal can still ask.
            if [ "$status" -eq 127 ] && [ -t 0 ]; then sudo bash -c "$script"; status=$?; fi
            if [ "$status" -ne 0 ]; then
                printf '\n!! The steps that need your password were not done (cancelled, or failed: see above).\n'
                printf '   Everything else is installed. "Go To Spatiand" registers Spatial Mode when clicked,\n'
                printf '   or run this installer again.\n'
            fi
        fi
    fi
fi

say "done"
cat <<EOF
Plug the glasses into the Deck's USB-C port first, then either:

  * double-click "Go To Spatiand" on the desktop, or
  * log out, and pick "Spatial Mode" at the login screen.

To come back to the desktop, hold any button on the exit row of the Deck's screen, or from a
terminal:  steamosctl switch-to-desktop-mode plasma.desktop

Spatiand's applications are in the menu, under Spatiand, to run on the desktop too.
What the session did is in ~/.local/share/spatiand-session.log (the one before: .log.1).
To remove it all: $DEST/uninstall.sh
EOF
