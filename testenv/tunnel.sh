#!/usr/bin/env bash
# SSH forward Mac 127.0.0.1:8099 -> knobas-wsl 127.0.0.1:8099 (XWiki).
#   testenv/tunnel.sh up|down|status
#
# Its own ssh master on its own control socket (-M -S, ControlPersist=no), so
# it neither rides on nor holds open the shared `knobas-wsl` ControlMaster from
# ~/.ssh/config, and is separate from knobas' testenv/live tunnel. A plain
# `ssh -fN -L` would hand the forward to that shared master, which drops it
# when ControlPersist expires.
set -euo pipefail

HOST="${XWIKI_SSH_HOST:-knobas-wsl}"
PORT="${XWIKI_PORT:-8099}"
# macOS caps socket paths at 104 bytes; keep it short.
SOCK="${XWIKI_TUNNEL_SOCK:-${TMPDIR:-/tmp}/knobas-xwiki.sock}"

is_up() {
  [ -S "$SOCK" ] && ssh -S "$SOCK" -O check "$HOST" >/dev/null 2>&1
}

case "${1:-status}" in
  up)
    if is_up; then
      echo "tunnel already up"
    else
      ssh -f -N -M -S "$SOCK" -o ControlPersist=no \
        -o ExitOnForwardFailure=yes -o ServerAliveInterval=30 -o ServerAliveCountMax=3 \
        -L "127.0.0.1:$PORT:127.0.0.1:$PORT" "$HOST" </dev/null
      echo "tunnel up: http://localhost:$PORT/xwiki/"
    fi
    ;;
  down)
    if is_up; then
      ssh -S "$SOCK" -O exit "$HOST" >/dev/null 2>&1 || true
      echo "tunnel down"
    else
      echo "tunnel not running"
    fi
    ;;
  status)
    if is_up; then
      code="$(curl -s -o /dev/null -w '%{http_code}' "http://127.0.0.1:$PORT/xwiki/rest/" || true)"
      echo "tunnel up; GET http://127.0.0.1:$PORT/xwiki/rest/ -> $code"
    else
      echo "tunnel down"
      exit 1
    fi
    ;;
  *)
    echo "usage: $0 up|down|status" >&2
    exit 2
    ;;
esac
