#!/bin/sh
# Create the folders magpie-fi needs, owned by PUID:PGID from .env.
# Run once before the first `docker compose up` (with sudo if the paths need root).
#
#   sudo ./scripts/setup-folders.sh
#
# Docker would otherwise create missing folders as root, and Navidrome (which runs
# as PUID) could not write to them.
set -eu

cd "$(dirname "$0")/.."
if [ ! -f .env ]; then
    echo "No .env found. Run: cp .env.example .env  (then edit it)" >&2
    exit 1
fi

value() {
    sed -n "s/^$1=//p" .env | tail -n 1
}

music_root="$(value MUSIC_ROOT)"
data_root="$(value DATA_ROOT)"
puid="$(value PUID)"
pgid="$(value PGID)"
music_root="${music_root:-./music}"
data_root="${data_root:-./data}"
puid="${puid:-1000}"
pgid="${pgid:-1000}"

for folder in \
    "$music_root/inbox" \
    "$music_root/library/Playlists" \
    "$data_root/navidrome" \
    "$data_root/navidrome-backup" \
    "$data_root/core" \
    "$data_root/bot"; do
    mkdir -p "$folder"
    echo "ok  $folder"
done

if [ "$(id -u)" = "0" ]; then
    # Only the folders themselves; existing music files are left untouched.
    chown "$puid:$pgid" \
        "$music_root" "$music_root/inbox" "$music_root/library" "$music_root/library/Playlists" \
        "$data_root" "$data_root/navidrome" "$data_root/navidrome-backup" "$data_root/core" "$data_root/bot"
    echo "Owner set to $puid:$pgid"
elif [ "$(id -u)" != "$puid" ]; then
    echo "Note: you are not uid $puid. If Navidrome cannot write to $data_root/navidrome," >&2
    echo "run this script again with sudo." >&2
fi
