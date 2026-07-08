#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
REPO_ROOT="$(cd -- "$SCRIPT_DIR/../.." && pwd -P)"
CARGO="${CARGO:-/opt/rust/cargo/bin/cargo}"
CC="${CC:-cc}"
SERVER_SRC="$SCRIPT_DIR/file_stream_server.c"

WORKDIR="$(mktemp -d)"
cleanup() {
    rm -rf "$WORKDIR"
}
trap cleanup EXIT

SERVER_BIN="$WORKDIR/file_stream_server"
PAYLOAD_FILE="$WORKDIR/payload.raw"
CONFIG_FILE="$WORKDIR/Fizzle.toml"
SERVER_STDOUT="$WORKDIR/server.stdout"
SERVER_STDERR="$WORKDIR/server.stderr"
BUILD_STDOUT="$WORKDIR/build.stdout"
BUILD_STDERR="$WORKDIR/build.stderr"

printf 'file-backed payload over fizzle\n' > "$PAYLOAD_FILE"

cat > "$CONFIG_FILE" <<'EOF'
[io."tcp-client:127.0.0.1:39175"]
method = "plugin"
when = "startup"
module = "fizzle-plugin-file-stream"
plugin = "FileBackedFuzzClient"
streams = 1
EOF

"$CC" -Wall -Wextra -Werror -O0 "$SERVER_SRC" -o "$SERVER_BIN"
set +e
FIZZLE_CONFIG="$CONFIG_FILE" "$CARGO" build -p fizzle >"$BUILD_STDOUT" 2>"$BUILD_STDERR"
build_status=$?
set -e

if [[ "$build_status" -ne 0 ]]; then
    cat "$BUILD_STDOUT"
    cat "$BUILD_STDERR" >&2
    exit "$build_status"
fi

set +e
RUST_LOG=fizzle=debug \
FIZZLE_PAYLOAD_FILE="$PAYLOAD_FILE" \
FIZZLE_CONFIG="$CONFIG_FILE" \
LD_PRELOAD="$REPO_ROOT/target/debug/libfizzle.so" \
"$SERVER_BIN" >"$SERVER_STDOUT" 2>"$SERVER_STDERR" &
server_pid=$!

timed_out=0
for _ in $(seq 1 50); do
    if ! kill -0 "$server_pid" 2>/dev/null; then
        break
    fi

    status="$(ps -o stat= -p "$server_pid" 2>/dev/null || true)"
    case "$status" in
        Z*) break ;;
    esac

    sleep 0.1
done

if kill -0 "$server_pid" 2>/dev/null; then
    status="$(ps -o stat= -p "$server_pid" 2>/dev/null || true)"
    case "$status" in
        Z*) ;;
        *)
            timed_out=1
            kill "$server_pid" 2>/dev/null || true
            ;;
    esac
fi

wait "$server_pid"
server_status=$?
set -e

if [[ "$timed_out" -ne 0 ]]; then
    echo "file stream e2e server timed out" >&2
    cat "$SERVER_STDERR" >&2
    exit 124
fi

if [[ "$server_status" -ne 0 ]]; then
    cat "$SERVER_STDOUT"
    cat "$SERVER_STDERR" >&2
    exit "$server_status"
fi
