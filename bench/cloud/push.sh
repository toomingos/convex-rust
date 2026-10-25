#!/bin/sh
# Pushes bench/cloud/convex to the cloud deployment with the STOCK published convex CLI.
# The deploy key is read from the file named by CLOUD_DEPLOY_KEY_FILE (or $CONVEX_DEPLOY_KEY) and only
# exported into this process's environment; it is never printed or written anywhere.
set -e
cd "$(dirname "$0")"
if [ -z "$CONVEX_DEPLOY_KEY" ]; then
  [ -n "$CLOUD_DEPLOY_KEY_FILE" ] || { echo "set CLOUD_DEPLOY_KEY_FILE or CONVEX_DEPLOY_KEY" >&2; exit 2; }
  CONVEX_DEPLOY_KEY=$(cat "$CLOUD_DEPLOY_KEY_FILE")
  export CONVEX_DEPLOY_KEY
fi
exec npx --no-install convex deploy -y "$@"
