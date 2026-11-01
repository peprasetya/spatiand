#!/usr/bin/env bash
# Spatiand's own applications in the desktop's menu, in a category of their own, to run without the
# room and without glasses: SteamOS's Plasma, as ordinary Wayland windows.
#
#   tools/install-desktop-apps.sh [folder with the programs]     (default: this tree's target/release)
#
# Needs no root: everything lands under $HOME. Entries are by app id, so that Plasma puts the right
# icon on a window. The room's launcher skips their category (desktop_entry.rs): it has these
# behind its own bubble.
set -euo pipefail

REPO="$(cd "$(dirname "$0")/.." && pwd)"
PROGRAMS="${1:-$REPO/target/release}"
BROWSER="$HOME/spatiand-chromium/run.sh"
ICONS="$HOME/.local/share/spatiand/desktop-icons"
ENTRIES="$HOME/.local/share/applications"
DIRECTORIES="$HOME/.local/share/desktop-directories"
MENUS="$HOME/.config/menus/applications-merged"
AUTOSTART="$HOME/.config/autostart"

[ -x "$PROGRAMS/spatiand" ] || { echo "no spatiand in $PROGRAMS: build it, or name the folder" >&2; exit 1; }
install -d "$ICONS" "$ENTRIES" "$DIRECTORIES" "$MENUS" "$AUTOSTART"
install -m 644 "$REPO"/assets/apps/*.svg "$REPO/assets/icons/spatiand.svg" "$ICONS/"

entry() { # file, name, comment, command, icon, window class
    cat > "$ENTRIES/$1.desktop" <<E
[Desktop Entry]
Type=Application
Name=$2
Comment=$3
Exec=$4
Icon=$ICONS/$5.svg
Terminal=false
Categories=X-Spatiand;
StartupWMClass=$6
E
}

app() { # program, name, comment; listed only if it is there to run
    if [ -x "$PROGRAMS/$1" ]; then
        entry "id.prasetya.spatiand.$1" "$2" "$3" "$PROGRAMS/$1" "$1" "id.prasetya.spatiand.$1"
        echo "listed $2"
    else
        rm -f "$ENTRIES/id.prasetya.spatiand.$1.desktop"
        echo "no $2 here ($PROGRAMS/$1)"
    fi
}

app spatishelf SpatiShelf "Files"
app spatireel SpatiReel "Video"
app spatiloupe SpatiLoupe "Pictures, PDF and 3D objects"
app spatiquill SpatiQuill "Terminal"
app spatipal SpatiPal "Companion"
app spatikey SpatiKey "Passwords and passkeys"

# Dictation is a switch, not a program with a window: this script turns it on and off, and it is on
# again at the next login only if it was left on.
SWITCH="$HOME/.local/share/spatiand/dictation.sh"
cat > "$SWITCH" <<E
#!/bin/sh
# toggle: from the menu. start: from the login, if it was left on.
LOGIN="$AUTOSTART/spatiand-dictation.desktop"
start() {
    systemd-run --user --quiet --collect --unit=spatiand-dictation --setenv=WAYLAND_DISPLAY=\${WAYLAND_DISPLAY:-wayland-0} "$PROGRAMS/spatiand" --dictation-service
}
if [ "\$1" = start ]; then start; exit; fi
if systemctl --user is-active -q spatiand-dictation; then
    systemctl --user stop spatiand-dictation
    rm -f "\$LOGIN"
    notify-send -a Dictation "Dictation is off" "The microphone is closed."
else
    start
    printf '[Desktop Entry]\\nType=Application\\nName=Dictation\\nExec=$SWITCH start\\nOnlyShowIn=KDE;\\nNoDisplay=true\\n' > "\$LOGIN"
    notify-send -a Dictation "Dictation is on" "Listening: what you say is typed into the front window. Choose Dictation in the menu again to stop."
fi
E
chmod +x "$SWITCH"
cat > "$ENTRIES/id.prasetya.spatiand.dictation.desktop" <<E
[Desktop Entry]
Type=Application
Name=Dictation
Comment=Turn dictation on or off: while on, the microphone is open and what you say is typed
Exec=$SWITCH toggle
Icon=audio-input-microphone
Terminal=false
Categories=X-Spatiand;
E

# The browser is Chromium, where it is unpacked, with the launcher that has its switches.
if [ -x "$BROWSER" ]; then
    entry id.prasetya.spatiand.spatimesh SpatiMesh "Web browser" "$BROWSER %U" spatimesh spatimesh-chromium
    echo "listed SpatiMesh"
    # The room's way in: a window of the room's launcher, not a second line in this menu.
    room_entry="$ENTRIES/spatiand-chromium.desktop"
    if [ -f "$room_entry" ] && ! grep -q '^NotShowIn=' "$room_entry"; then
        sed -i '/^\[Desktop Entry\]/a NotShowIn=KDE;' "$room_entry"
    fi
fi

cat > "$DIRECTORIES/spatiand.directory" <<E
[Desktop Entry]
Type=Directory
Name=Spatiand
Icon=$ICONS/spatiand.svg
E

cat > "$MENUS/spatiand.menu" <<'E'
<!DOCTYPE Menu PUBLIC "-//freedesktop//DTD Menu 1.0//EN" "http://www.freedesktop.org/standards/menu-spec/menu-1.0.dtd">
<Menu>
  <Name>Applications</Name>
  <Menu>
    <Name>Spatiand</Name>
    <Directory>spatiand.directory</Directory>
    <Include><Category>X-Spatiand</Category></Include>
  </Menu>
</Menu>
E

# Passwords on the desktop: the service the room otherwise holds. It ends with the desktop.
cat > "$AUTOSTART/spatikey-desktop.desktop" <<E
[Desktop Entry]
Type=Application
Name=SpatiKey service
Exec=$PROGRAMS/spatiand --spatikey-service
OnlyShowIn=KDE;
NoDisplay=true
E

# Have it now if this is the desktop already.
# In the desktop's own service manager, so that it outlives whatever ran this.
if [ -S "${XDG_RUNTIME_DIR:-/run/user/$(id -u)}/wayland-0" ] && pgrep -x plasmashell >/dev/null \
    && ! pgrep -fx '.*/spatiand --spatikey-service' >/dev/null; then
    systemd-run --user --quiet --collect --unit=spatikey-desktop --setenv=WAYLAND_DISPLAY=wayland-0 \
        "$PROGRAMS/spatiand" --spatikey-service || true
fi

# In Plasma's own environment: from anywhere else (ssh) this would fill a cache it does not read.
plasma="$(pgrep -x plasmashell | head -1 || true)"
if [ -n "$plasma" ] && [ -r "/proc/$plasma/environ" ]; then
    while IFS= read -r -d '' setting; do
        case "$setting" in
            XDG_MENU_PREFIX=*|XDG_CONFIG_DIRS=*|XDG_DATA_DIRS=*|XDG_CURRENT_DESKTOP=*|LANG=*|LC_ALL=*) export "$setting" ;;
        esac
    done < "/proc/$plasma/environ"
fi
for rebuild in kbuildsycoca6 kbuildsycoca5; do
    command -v "$rebuild" >/dev/null && { "$rebuild" >/dev/null 2>&1 || true; break; }
done
echo "done: look under Spatiand in the application menu"
