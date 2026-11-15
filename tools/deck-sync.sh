#!/bin/sh
# Copies the repository's tracked files to the Deck's checkout (/home/deck/spatiand), which the
# Deck's session runs from. Never .cargo/: the Deck's .cargo/config.toml is the Deck's own build
# environment (FFmpeg, libva, the rpath; a copy at ~/spatiand-build/config.toml.deck), while the
# repository's is the Mac's. rsync's --exclude does not stop a path named in --files-from, so the
# list is filtered here (it was overwritten twice on 2026-11-14/15 that way).
set -e
cd "$(dirname "$0")/.."
git ls-files -z | tr '\0' '\n' | grep -v '^\.cargo/' | tr '\n' '\0' |
    rsync -az --from0 --files-from=- ./ deck@steamdeck:/home/deck/spatiand/
ssh deck@steamdeck 'cmp -s ~/spatiand/.cargo/config.toml ~/spatiand-build/config.toml.deck || { echo "the Deck build config differs from its copy" >&2; exit 1; }'
echo "synced to the Deck"
