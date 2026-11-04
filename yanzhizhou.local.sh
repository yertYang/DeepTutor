#!/bin/sh
set -eu

project_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
export DEEPTUTOR_HOME="${DEEPTUTOR_HOME:-$project_dir/native-home}"
export NEXT_TELEMETRY_DISABLED=1
cd "$project_dir"
server="${DEEPTUTOR_RUST_SERVER_BIN:-$project_dir/target/release/yanzhizhou}"
if [ ! -x "$server" ]; then
    echo "Build the native backend first: cargo build --release --locked -p deeptutor-server" >&2
    exit 1
fi

if [ "$#" -gt 0 ]; then
    exec "$server" --home "$DEEPTUTOR_HOME" "$@"
fi

exec "$server" --home "$DEEPTUTOR_HOME" start \
    --web "$project_dir/web/.next/standalone" \
    --node "${DEEPTUTOR_NODE_BIN:-node}" \
    --backend-port "${BACKEND_PORT:-8002}" --port "${FRONTEND_PORT:-3782}"
