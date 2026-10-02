#!/usr/bin/env bash
# Prove the engine is reachable through the real SurrealQL front end.
#
# This is the evidence that Phase 0's last task is done. Not "it compiles" and
# not "the socket is open": a CREATE committed and a SELECT read it back through
# the same HTTP surface a client would use, on top of our storage engine.
#
# What it checks, and why each one earns its place:
#
#   /health   liveness — the process is up
#   /ready    readiness — the datastore can serve. This is the route an edition
#              overrides with its own condition, so a pass here proves we are on
#              the community route set and not a stub.
#   /version  the community version route, also a proxy for "community router"
#   /rpc      the point of the exercise: SurrealQL over HTTP on ds+mem://
#
# Round trip, not a bare `RETURN 1`: a RETURN proves nothing about our engine, and
# would pass against any datastore including a wrong one. CREATE then SELECT
# proves a write committed through our transaction layer and a read saw it.
#
# Asserts on HTTP status and response bodies, never on the server's stdout: a
# debug build prints a warning banner, and upstream prints an online version
# check result. Both are noise that would make this brittle.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

BIN="target/debug/surrealdb-ds-server"
# An ephemeral high port. Chosen to be unlikely to collide on a shared machine
# without needing a free-port helper.
PORT="${SMOKE_PORT:-18000}"
BASE="http://127.0.0.1:$PORT"
NS=smoke
DB=smoke
USERNAME="root"
PASSWORD="root"
AUTH="$USERNAME:$PASSWORD"
LOG="$(mktemp)"
SERVER_PID=""

cleanup() {
  if [ -n "$SERVER_PID" ] && kill -0 "$SERVER_PID" 2>/dev/null; then
    kill "$SERVER_PID" 2>/dev/null || true
    wait "$SERVER_PID" 2>/dev/null || true
  fi
  if [ "${KEEP_LOG:-0}" = 1 ]; then
    echo "--- server log kept at $LOG"
  else
    rm -f "$LOG"
  fi
}
trap cleanup EXIT

fail() {
  echo "FAIL: $*" >&2
  echo "--- server log ---" >&2
  cat "$LOG" >&2
  exit 1
}

pass() { echo "  ok   $*"; }

need() { command -v "$1" >/dev/null 2>&1 || fail "$1 is required but not installed"; }
need curl

if [ ! -x "$BIN" ]; then
  echo "building $BIN"
  cargo build -q -p surrealdb-ds-server
fi

# `--offline` is not a flag; the online version check is `--online-version-check`
# and defaults to TRUE, which would make every run depend on a call to
# surrealdb.com. Set it off.
SURREAL_ONLINE_VERSION_CHECK=false "$BIN" start \
  --bind "127.0.0.1:$PORT" \
  --username "$USERNAME" \
  --password "$PASSWORD" \
  --log warn \
  ds+mem:// > "$LOG" 2>&1 &
SERVER_PID=$!

# Wait for readiness rather than sleeping a fixed amount: a fixed sleep is either
# flaky or slow, and on a cold debug build it will be slow.
wait_for() {
  local path="$1" tries=0
  until curl -fsS -o /dev/null "$BASE$path" 2>/dev/null; do
    tries=$((tries + 1))
    if [ "$tries" -gt 120 ]; then
      fail "$path never became ready after 120 tries (2s each)"
    fi
    if ! kill -0 "$SERVER_PID" 2>/dev/null; then
      fail "the server exited before $path was ready"
    fi
    sleep 2
  done
}

echo "waiting for the server on $BASE ..."
wait_for /ready
pass "server is up"

# /health and /ready answered 200 by construction in wait_for. Assert the rest.
code=$(curl -sS -o /dev/null -w '%{http_code}' "$BASE/health")
[ "$code" = 200 ] || fail "/health returned $code, expected 200"
pass "/health -> 200"

code=$(curl -sS -o /dev/null -w '%{http_code}' "$BASE/ready")
[ "$code" = 200 ] || fail "/ready returned $code, expected 200"
pass "/ready -> 200"

body=$(curl -sS "$BASE/version")
[ -n "$body" ] || fail "/version returned an empty body"
pass "/version -> $body"

# One SurrealQL statement, as a client sends it.
#
# `/rpc` with `Content-Type: application/json` wants a JSON-RPC *object*, not a
# bare SurrealQL string: the handler decodes the body and rejects anything that
# is not an object. So the query goes in `params`, not in the body.
#
# Note there is no `--offline` flag. The online version check is
# `--online-version-check` and defaults to TRUE, which would make every run
# depend on a call to surrealdb.com; it is disabled above.
rpc() {
  local params="$1"
  shift
  curl -sS -u "$AUTH" \
    -H 'Content-Type: application/json' \
    -H 'Accept: application/json' \
    "$@" \
    --data "$(printf '{"method":"query","params":[%s]}' "$params")" \
    "$BASE/rpc"
}

# The same, with a raw body, for cases that are not a `query`. `"${@:2}"`, not
# `"$@"`: the body is the first argument and curl would read it as a URL.
rpc_raw() {
  local body="$1"
  shift
  curl -sS -u "$AUTH" \
    -H 'Content-Type: application/json' \
    -H 'Accept: application/json' \
    "$@" \
    --data "$body" \
    "$BASE/rpc"
}

ns_db=(-H "surreal-ns: $NS" -H "surreal-db: $DB")

# A namespace and database have to exist before they can be addressed; there is no
# auto-create. `USE NS` is what lets DEFINE DATABASE name one.
out=$(rpc '"DEFINE NAMESPACE IF NOT EXISTS '"$NS"'; USE NS '"$NS"'; DEFINE DATABASE IF NOT EXISTS '"$DB"';"')
case "$out" in
  *'"status":"OK"'*) pass "namespace and database defined over /rpc" ;;
  *) fail "could not define the namespace/database: $out" ;;
esac

# A write, then a read that must see it. The engine is the thing under test, so
# the write has to go through a real transaction that commits. A `RETURN 1` would
# not: it touches no datastore and would pass against any backend, including a
# wrong one.
out=$(rpc '"CREATE smoke:probe SET seen = true;"' "${ns_db[@]}")
case "$out" in
  *'"status":"OK"'*) pass "CREATE over /rpc" ;;
  *) fail "CREATE did not succeed: $out" ;;
esac

out=$(rpc '"SELECT * FROM smoke:probe;"' "${ns_db[@]}")
case "$out" in
  *'"status":"OK"'*) pass "SELECT over /rpc" ;;
  *) fail "SELECT did not succeed: $out" ;;
esac
# The document must be there, with the value that was written. A datastore that
# acknowledged the write and dropped it would still answer OK.
case "$out" in
  *'smoke:probe'*'"seen":true'*) pass "the created document is readable back with its value" ;;
  *) fail "SELECT did not return the created document: $out" ;;
esac

# An unknown method must come back as an error. A front end that swallowed errors
# would pass every check above.
out=$(rpc_raw '{"method":"no_such_method","params":[]}' "${ns_db[@]}")
case "$out" in
  *'"error"'*) pass "an unknown RPC method is reported as an error" ;;
  *) fail "an unknown method did not produce an error: $out" ;;
esac

# Addressing a database that does not exist must fail. This is the check that
# would catch a stub datastore answering everything with OK, and it only passes
# because the real keyspace is being consulted — which is our engine's.
#
# It has to be a statement that *reads*. `RETURN 1` is answered from the
# expression alone without resolving the database, so it returns OK against any
# database name at all and would make this check vacuous.
out=$(rpc '"SELECT * FROM probe:one;"' -H "surreal-ns: $NS" -H "surreal-db: no_such_database")
case "$out" in
  *'"status":"ERR"'*) pass "an unknown database is refused, not silently accepted" ;;
  *) fail "an unknown database was accepted: $out" ;;
esac

echo
echo "smoke: all checks passed against ds+mem:// over HTTP"
