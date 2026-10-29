#!/usr/bin/env bash
# Tell the browsers where SpatiKey's native-messaging host is, so that the SpatiKey extension can
# reach SpatiKey (docs/spatikey.md, "The extensions"; extensions/spatikey/README.md).
#
#   tools/spatikey-browsers.sh [--print] [--all] [--remove] [/path/to/spatikey-host]
#
#   --print    show what would be written (or removed), and write nothing
#   --all      also for browsers whose folder is not there yet (not installed, or never started)
#   --remove   take the manifests away again
#
# The host is by default the one Spatiand ships: on the Mac
# ~/Applications/Spatiand.app/Contents/MacOS/spatikey-host, on Linux the program beside spatiand
# (the one on PATH, else ~/spatiand/target/release/). Needs no root: every manifest is the
# user's own, in the browser's folder under $HOME.
#
# Chrome, Chromium, Brave and Edge let in the extension by its id, which the fixed key in
# extensions/spatikey/manifest.chrome.json gives it; Firefox by its add-on id. A browser reads
# its manifest when the extension first connects, so nothing needs restarting.
#
# Flatpak browsers (on SteamOS every browser is one) are not set up: see the note printed at the
# end when there are any.
set -euo pipefail

NAME=id.prasetya.spatikey
# What Chrome makes of manifest.chrome.json's key; extensions/spatikey/README.md says how.
CHROME_ID=ndebkmflcohflpgdclfbnjjeogbigpib
FIREFOX_ID=spatikey@prasetya.id
DESCRIPTION="SpatiKey, Spatiand's passwords and passkeys"

usage() {
    sed -n '2,24p' "$0" | sed 's/^# \{0,1\}//'
}

print=false
all=false
remove=false
host=""
for arg in "$@"; do
    case "$arg" in
        --print) print=true ;;
        --all) all=true ;;
        --remove) remove=true ;;
        -h|--help) usage; exit 0 ;;
        -*) echo "!! unknown option: $arg" >&2; usage >&2; exit 2 ;;
        *) host="$arg" ;;
    esac
done

# Each browser as "name|its folder|chrome or firefox|where its manifests go".
browsers=()
case "$(uname -s)" in
    Darwin)
        support="$HOME/Library/Application Support"
        default_host="$HOME/Applications/Spatiand.app/Contents/MacOS/spatikey-host"
        for entry in "Chrome|Google/Chrome" "Chrome Beta|Google/Chrome Beta" "Chrome Canary|Google/Chrome Canary" \
                     "Chromium|Chromium" "Brave|BraveSoftware/Brave-Browser" "Edge|Microsoft Edge"; do
            dir="$support/${entry#*|}"
            browsers+=("${entry%%|*}|$dir|chrome|$dir/NativeMessagingHosts")
        done
        # Firefox and Firefox Developer Edition read the same folder.
        browsers+=("Firefox|$support/Mozilla|firefox|$support/Mozilla/NativeMessagingHosts")
        ;;
    Linux)
        config="${XDG_CONFIG_HOME:-$HOME/.config}"
        spatiand="$(command -v spatiand || true)"
        [[ -n "$spatiand" ]] || spatiand="$HOME/spatiand/target/release/spatiand"
        default_host="$(dirname "$spatiand")/spatikey-host"
        for entry in "Chrome|google-chrome" "Chrome Beta|google-chrome-beta" "Chrome Dev|google-chrome-unstable" \
                     "Chromium|chromium" "Brave|BraveSoftware/Brave-Browser" "Edge|microsoft-edge"; do
            dir="$config/${entry#*|}"
            browsers+=("${entry%%|*}|$dir|chrome|$dir/NativeMessagingHosts")
        done
        browsers+=("Firefox|$HOME/.mozilla|firefox|$HOME/.mozilla/native-messaging-hosts")
        ;;
    *)
        echo "!! $(uname -s): only the Mac and Linux have browsers this sets up" >&2
        exit 1
        ;;
esac

host="${host:-$default_host}"
# A manifest's path must be absolute: a relative one is read from the manifest's own folder.
[[ "$host" == /* ]] || host="$PWD/$host"
if ! $remove && [[ ! -x "$host" ]]; then
    echo "note: $host is not there (or not runnable) yet; the manifests name it anyway" >&2
fi

json_string() {
    local s=${1//\\/\\\\}
    s=${s//\"/\\\"}
    printf '"%s"' "$s"
}

manifest() {
    local kind=$1
    local allowed
    if [[ "$kind" == chrome ]]; then
        allowed="  \"allowed_origins\": [\"chrome-extension://$CHROME_ID/\"]"
    else
        allowed="  \"allowed_extensions\": [\"$FIREFOX_ID\"]"
    fi
    cat <<EOF
{
  "name": "$NAME",
  "description": $(json_string "$DESCRIPTION"),
  "path": $(json_string "$host"),
  "type": "stdio",
$allowed
}
EOF
}

done_any=false
for entry in "${browsers[@]}"; do
    IFS='|' read -r name dir kind target <<<"$entry"
    file="$target/$NAME.json"
    if $remove; then
        [[ -e "$file" ]] || continue
        if $print; then
            echo "$name: would remove $file"
        else
            rm -f "$file"
            echo "$name: removed $file"
        fi
        done_any=true
        continue
    fi
    if [[ ! -d "$dir" ]] && ! $all; then
        $print && echo "$name: not here ($dir); --all writes it anyway"
        continue
    fi
    if $print; then
        echo "$name: would write $file:"
        manifest "$kind" | sed 's/^/    /'
    else
        mkdir -p "$target"
        manifest "$kind" > "$file.new"
        mv "$file.new" "$file"
        echo "$name: wrote $file"
    fi
    done_any=true
done
$done_any || echo "no browser here to set up (--all writes the manifests anyway)"

# Flatpak browsers run in a sandbox. They read native-messaging manifests from their own folder
# under ~/.var/app, and a host they start runs inside the sandbox, where spatikey-host (outside
# it) and the service's socket ($XDG_RUNTIME_DIR/spatikey.sock) cannot be seen. Firefox's Flatpak
# reaches hosts outside only through the WebExtensions portal, where the desktop has it; the
# Chromium-based ones not at all. So they are only named here, with what it would take.
if [[ "$(uname -s)" == Linux ]] && command -v flatpak >/dev/null; then
    flatpaks=$(flatpak list --app --columns=application 2>/dev/null |
        grep -E '^(com\.google\.Chrome|com\.google\.ChromeDev|org\.chromium\.Chromium|com\.brave\.Browser|com\.microsoft\.Edge|org\.mozilla\.firefox|io\.github\.ungoogled_software\.ungoogled_chromium)$' || true)
    if [[ -n "$flatpaks" ]]; then
        cat >&2 <<EOF

note: these browsers are Flatpaks, which this does not set up:
$(echo "$flatpaks" | sed 's/^/    /')
  The SpatiKey extension in them cannot reach SpatiKey as things are. Untried, and a hole in the
  sandbox of your own making if you try it: show the browser the host and the socket, e.g. for
  Chrome
    flatpak override --user --filesystem=$(dirname "$host"):ro --filesystem=xdg-run/spatikey.sock com.google.Chrome
  and put the manifest where the sandbox looks:
    ~/.var/app/com.google.Chrome/config/google-chrome/NativeMessagingHosts/$NAME.json
  The proper way is a browser from outside Flatpak, or SpatiMesh, which has SpatiKey built in.
EOF
    fi
fi
