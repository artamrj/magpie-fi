#!/bin/sh
# Prepares magpie-fi's folders, then runs magpie-fi as PUID:PGID so every file in the
# library belongs to the host user. Compose should not use `user:` (some NAS
# runtimes reject it); the container starts as root and drops privileges here.
set -eu

runtime_uid="${PUID:-10001}"
runtime_gid="${PGID:-10001}"
data_dir="${MAGPIE_DATA_DIR:-/data}"
music_root="${MAGPIE_MUSIC_ROOT:-/music}"

validate_id() {
    case "$2" in
        ''|*[!0-9]*)
            echo "Error: $1 must be a positive numeric ID, got '$2'" >&2
            exit 64
            ;;
    esac
    if [ "$2" = "0" ]; then
        echo "Error: $1 must be greater than zero" >&2
        exit 64
    fi
}

validate_id PUID "$runtime_uid"
validate_id PGID "$runtime_gid"

if [ "$(id -u)" != "0" ]; then
    if [ "$(id -u)" != "$runtime_uid" ] || [ "$(id -g)" != "$runtime_gid" ]; then
        echo "Error: remove the Compose 'user:' override; use PUID and PGID instead" >&2
        exit 77
    fi
    exec "$@"
fi

wanted="${runtime_uid}:${runtime_gid}"

# magpie-fi's own folders. The database folder is magpie-fi's alone, so it is fixed
# recursively. For the music only the top folders are touched: your existing
# music files are never re-owned.
# Versions before the rename used .magpie/: move it once, before the new one exists.
if [ -d "$music_root/.magpie" ] && [ ! -e "$music_root/.magpie-fi" ]; then
    echo "Moving $music_root/.magpie to $music_root/.magpie-fi"
    mv "$music_root/.magpie" "$music_root/.magpie-fi"
fi
mkdir -p "$data_dir" "$music_root/inbox" "$music_root/library" "$music_root/.magpie-fi/trash"
if [ "$(stat -c '%u:%g' "$data_dir")" != "$wanted" ]; then
    echo "Setting ownership of $data_dir to $wanted"
    chown -R "$wanted" "$data_dir"
fi
for folder in "$music_root" "$music_root/inbox" "$music_root/library" \
              "$music_root/.magpie-fi" "$music_root/.magpie-fi/trash"; do
    [ "$(stat -c '%u:%g' "$folder")" = "$wanted" ] || chown "$wanted" "$folder"
done

exec setpriv --reuid="$runtime_uid" --regid="$runtime_gid" --clear-groups "$@"
