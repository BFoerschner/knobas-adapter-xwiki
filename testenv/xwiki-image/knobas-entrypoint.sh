#!/bin/bash
# Writes xwiki.cfg and xwiki.properties into the permanent directory before the
# official entrypoint runs. The official entrypoint copies those two files over
# WEB-INF on every start (saveConfigurationFile / restoreConfigurationFile), so
# the settings below survive restarts and win over its own first-start edits.
#
# Settings:
#   xwiki.superadminpassword       enables superadmin (needed once, to create the
#                                  `knobas` admin; see ../README.md)
#   distribution.job.interactive   false: the Distribution Wizard runs on its own
#   distribution.defaultUI         the XWiki Standard flavor it installs
#   openoffice.autoStart           false: no LibreOffice process (saves ~300 MB)
#
# Delete /usr/local/xwiki/data/xwiki.{cfg,properties} in the volume to regenerate.
set -euo pipefail

DATA=/usr/local/xwiki/data
SUPERADMIN_PASSWORD="${XWIKI_SUPERADMIN_PASSWORD:-knobas-superadmin}"
FLAVOR="org.xwiki.platform:xwiki-platform-distribution-flavor-mainwiki/${XWIKI_VERSION}"

# Pristine copies: the webapp sits in ROOT until the official entrypoint moves
# it to $CONTEXT_PATH on the container's first start.
SRC=""
for d in /usr/local/tomcat/webapps/ROOT/WEB-INF "/usr/local/tomcat/webapps/${CONTEXT_PATH:-ROOT}/WEB-INF"; do
  if [ -f "$d/xwiki.cfg" ]; then SRC="$d"; break; fi
done

# set_kv FILE KEY VALUE: replace the first (possibly commented) KEY line, else append.
set_kv() {
  local file="$1" key="$2" value="$3"
  local tmp="$file.knobas-tmp"
  if grep -qE "^#? ?${key//./\\.} ?=" "$file"; then
    awk -v k="$key" -v v="$value" '
      BEGIN { done = 0; re = "^#? ?" k " ?=" ; gsub(/\./, "\\.", re) }
      !done && $0 ~ re { print k "=" v; done = 1; next }
      { print }' "$file" > "$tmp"
  else
    cp "$file" "$tmp"
    printf '%s=%s\n' "$key" "$value" >> "$tmp"
  fi
  cat "$tmp" > "$file"
  rm -f "$tmp"
}

mkdir -p "$DATA"
if [ -n "$SRC" ] && [ ! -f "$DATA/xwiki.cfg" ]; then
  echo "knobas: writing $DATA/xwiki.cfg"
  cp "$SRC/xwiki.cfg" "$DATA/xwiki.cfg"
  set_kv "$DATA/xwiki.cfg" xwiki.superadminpassword "$SUPERADMIN_PASSWORD"
fi
if [ -n "$SRC" ] && [ ! -f "$DATA/xwiki.properties" ]; then
  echo "knobas: writing $DATA/xwiki.properties"
  cp "$SRC/xwiki.properties" "$DATA/xwiki.properties"
  # What the official entrypoint's configure() would have set.
  set_kv "$DATA/xwiki.properties" environment.permanentDirectory "$DATA"
  set_kv "$DATA/xwiki.properties" openoffice.autoStart false
  set_kv "$DATA/xwiki.properties" distribution.job.interactive false
  set_kv "$DATA/xwiki.properties" distribution.defaultUI "$FLAVOR"
fi

exec /usr/local/bin/docker-entrypoint.sh "$@"
