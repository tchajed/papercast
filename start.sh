#!/bin/sh
set -e

# Restore DB from Tigris on first boot if it doesn't exist
if [ ! -f /data/podcast.db ]; then
    litestream restore -if-replica-exists /data/podcast.db
fi

# Run Litestream as supervisor, launching the app as a subprocess
exec litestream replicate -exec "/usr/local/bin/backend"
