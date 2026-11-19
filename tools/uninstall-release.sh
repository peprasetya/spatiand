#!/usr/bin/env bash
# Remove what a release's install.sh installed (tools/install-release.sh): the programs at
# ~/.local/share/spatiand/release, their menu and desktop entries, and -- with your password --
# the session entry and the glasses' udev rules.
#
#   ./uninstall.sh [--yes] [--no-system]
#
#   --yes        remove what needs your password without asking first, the udev rules too
#   --no-system  only what is in your home
#
# What you made with it stays, as it does when any program is removed: your settings, SpatiKey's
# passwords, SpatiMesh's profile, the environments and applications added to the room. Where they
# are is said at the end.
#
# An entry is removed only where it starts this install's programs: a development checkout's
# (tools/install-desktop-apps.sh run from it) is left alone. DESTDIR=<folder> takes the system
# files from under that folder instead, with no password, as install.sh's does.
set -uo pipefail

DEST="$HOME/.local/share/spatiand/release"
ROOT="${DESTDIR:-}"

yes=false
system=true
for arg in "$@"; do
    case "$arg" in
        --yes) yes=true ;;
        --no-system) system=false ;;
        -h|--help) sed -n '2,18p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
        *) echo "!! unknown option: $arg (see --help)" >&2; exit 2 ;;
    esac
done

say() { printf '\n== %s ==\n' "$1"; }
die() { printf '\n!! %s\n' "$1" >&2; exit 1; }
ask() {
    $yes && return 0
    [ -t 0 ] || return 1
    local answer
    read -rp "$1 [y/N] " answer
    [[ "$answer" == [yY]* ]]
}
running_under() {
    local p exe
    for p in /proc/[0-9]*; do
        exe=$(readlink "$p/exe" 2>/dev/null) || continue
        [[ "$exe" == "$1/"* ]] || continue
        printf '%s %s %s\n' "${p#/proc/}" "${exe% (deleted)}" "$(tr '\0' ' ' <"$p/cmdline" 2>/dev/null | cut -d' ' -f2-)"
    done
}
# A file of ours: it names a program of this install.
ours() { [ -f "$1" ] && grep -qF "$DEST/" "$1"; }

# All of it is read before any of it runs: the copy of this script in $DEST is removed below.
main() {
    while read -r pid exe args; do
        if [ "$exe" = "$DEST/bin/spatiand" ] && [[ "$args" != *-service* ]]; then
            die "Spatial Mode is running from this install (pid $pid): leave it first, then run this again"
        fi
    done < <(running_under "$DEST")

    say "stopping what runs from $DEST"
    # By what they run, not by their units' names: a checkout's services have the same ones.
    local pid exe args stopped=0
    while read -r pid exe args; do
        echo "stopping $(basename "$exe") $args(pid $pid)"
        kill "$pid" 2>/dev/null
        stopped=$((stopped + 1))
    done < <(running_under "$DEST")
    for _ in 1 2 3 4 5 6 7 8 9 10; do
        [ -z "$(running_under "$DEST")" ] && break
        sleep 0.5
    done
    while read -r pid exe args; do kill -KILL "$pid" 2>/dev/null; done < <(running_under "$DEST")
    [ "$stopped" -gt 0 ] || echo "nothing was running"

    say "removing the menu and desktop entries"
    local apps="$HOME/.local/share/applications" desktop f
    desktop="${XDG_DESKTOP_DIR:-$(xdg-user-dir DESKTOP 2>/dev/null || echo "$HOME/Desktop")}"
    for f in "$apps"/id.prasetya.spatiand.*.desktop "$apps/spatiand-go.desktop" "$desktop/spatiand-go.desktop" \
        "$HOME/.config/autostart/spatikey-desktop.desktop" "$HOME/.config/autostart/spatiand-dictation.desktop" \
        "$HOME/.local/share/spatiand/dictation.sh"; do
        if ours "$f"; then rm -f "$f" && echo "removed $f"; fi
    done
    # The dictation switch's entry names the switch, not a program.
    f="$apps/id.prasetya.spatiand.dictation.desktop"
    if [ -f "$f" ] && [ ! -e "$HOME/.local/share/spatiand/dictation.sh" ]; then rm -f "$f" && echo "removed $f"; fi
    # Shared with a checkout's entries, if there are any: removed when nothing of Spatiand's is left.
    if ! ls "$apps"/id.prasetya.spatiand.*.desktop >/dev/null 2>&1; then
        rm -rf "$HOME/.local/share/spatiand/desktop-icons"
        rm -f "$HOME/.local/share/desktop-directories/spatiand.directory" "$HOME/.config/menus/applications-merged/spatiand.menu"
    fi
    [ -e "$apps/spatiand-go.desktop" ] || rm -f "$HOME/.local/share/icons/hicolor/scalable/apps/spatiand.svg"
    # The menu, read again in Plasma's own environment (install-desktop-apps.sh says why).
    local plasma setting
    plasma="$(pgrep -x plasmashell | head -1 || true)"
    if [ -n "$plasma" ] && [ -r "/proc/$plasma/environ" ]; then
        while IFS= read -r -d '' setting; do
            case "$setting" in
                XDG_MENU_PREFIX=*|XDG_CONFIG_DIRS=*|XDG_DATA_DIRS=*|XDG_CURRENT_DESKTOP=*|LANG=*|LC_ALL=*) export "$setting" ;;
            esac
        done < "/proc/$plasma/environ"
    fi
    command -v kbuildsycoca6 >/dev/null && { kbuildsycoca6 >/dev/null 2>&1 || true; }

    local launcher="$ROOT/usr/local/bin/spatiand-session" session="$ROOT/usr/share/wayland-sessions/spatiand.desktop"
    local rules="$ROOT/etc/udev/rules.d/99-spatiand.rules" script=""
    if $system; then
        say "what needs your password"
        if ours "$launcher"; then
            echo "Spatial Mode at the login screen starts this install: $launcher, $session."
            script+="rm -f '$launcher' '$session'"$'\n'
            # A login that goes straight to the session it names would find none.
            if [ -z "$ROOT" ] && [ "$(steamosctl get-default-desktop-session 2>/dev/null)" = spatiand.desktop ]; then
                steamosctl set-default-desktop-session plasma.desktop >/dev/null 2>&1 &&
                    echo "the desktop session to log into is Plasma again"
            fi
        elif [ -f "$launcher" ]; then
            echo "Spatial Mode at the login screen starts another copy of Spatiand ($launcher): left as it is."
        fi
        if [ -f "$rules" ]; then
            echo "The glasses' udev rules are installed ($rules); a development checkout uses them too."
            if ask "Remove the udev rules?"; then
                script+="rm -f '$rules'"$'\n'
                [ -n "$ROOT" ] || script+="udevadm control --reload-rules || true"$'\n'
            fi
        fi
        if [ -n "$script" ] && [ -n "$ROOT" ]; then
            bash -c "set -e"$'\n'"$script"
        elif [ -n "$script" ]; then
            # /usr is read-only on SteamOS until told otherwise, and is put back so after.
            script="set -e
relock=false
if command -v steamos-readonly >/dev/null && [ \"\$(steamos-readonly status 2>/dev/null)\" = enabled ]; then steamos-readonly disable; relock=true; fi
$script
if \$relock; then steamos-readonly enable || true; fi"
            local status=127
            command -v pkexec >/dev/null && { pkexec bash -c "$script"; status=$?; }
            if [ "$status" -eq 127 ] && [ -t 0 ]; then sudo bash -c "$script"; status=$?; fi
            [ "$status" -eq 0 ] || printf '!! Not removed (cancelled, or failed: see above). Run this again to try once more.\n'
        fi
        [ -n "$script" ] || echo "nothing to remove there"
    else
        ours "$launcher" && echo "left as asked (--no-system): $launcher and $session, which start the programs removed now"
    fi

    say "removing $DEST"
    rm -rf "$DEST" "$DEST.new" "$DEST.old"
    rmdir "$(dirname "$DEST")" 2>/dev/null

    say "done"
    cat <<EOF
Spatiand is removed. What you made with it is where it was, for you to keep or delete:

  ~/.config/spatiand               settings, and SpatiKey's passwords (apps/spatikey)
  ~/.config/spatiand-chromium      SpatiMesh's profiles: bookmarks, history, logins
  ~/.local/share/spatiand          environments and applications added to the room
  ~/.local/share/spatiand-session.log, spatiand-apps.log   logs
EOF
}

main "$@"
exit $?
