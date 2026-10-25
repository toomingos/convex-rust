#!/usr/bin/env bash
# Publish packages/convex-rust to npm. Asks for a 6-digit 2FA code (authenticator app); press Enter with no code
# if your 2FA is a security key / passkey, and npm will open a browser page to confirm instead.
# Usage: release/publish-npm.sh [--dry-run]
# Auth: uses $NPM_USERCONFIG if set (an npmrc with an _authToken line), else your normal `npm login` session.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT/packages/convex-rust"

USERCONFIG=()
[ -n "${NPM_USERCONFIG:-}" ] && USERCONFIG=(--userconfig "$NPM_USERCONFIG")

NAME="$(node -p 'require("./package.json").name')"
VERSION="$(node -p 'require("./package.json").version')"
echo "npm user: $(npm whoami ${USERCONFIG[@]+"${USERCONFIG[@]}"})"
if npm view "$NAME@$VERSION" version >/dev/null 2>&1; then
  echo "ERROR: $NAME@$VERSION is already published; bump the version in package.json first." >&2; exit 1
fi

npm pack --dry-run 2>&1 | grep -E "name:|version:|package size|total files"
if [ "${1:-}" = "--dry-run" ]; then echo "dry run: not publishing"; exit 0; fi

echo "Do NOT enter a recovery code: npm then blocks publishing for 72 hours."
read -r -p "Publish $NAME@$VERSION? 6-digit authenticator code, or Enter for browser/security-key 2FA: " OTP
if [ -z "$OTP" ]; then
  npm publish --access public --auth-type=web ${USERCONFIG[@]+"${USERCONFIG[@]}"}
else
  [[ "$OTP" =~ ^[0-9]{6}$ ]] || { echo "ERROR: expected a 6-digit code (or nothing)" >&2; exit 1; }
  npm publish --access public --otp "$OTP" ${USERCONFIG[@]+"${USERCONFIG[@]}"}
fi

sleep 3
echo "registry now has: $(npm view "$NAME@$VERSION" version 2>/dev/null || echo '(not visible yet; check again in a minute)')"
echo "Try it: npm install $NAME && npx $NAME init"
