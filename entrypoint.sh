#!/bin/bash
set -e
echo "$@"

if [ "$1" = 'rust-proxy' ]; then
    /app/bin/redis-rust-proxy "$@"
fi

exec /app/bin/redis-rust-proxy "$@"
