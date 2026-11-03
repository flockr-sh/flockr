#!/usr/bin/env bash
# Build the flockr dev app on the Mac and upload it to TestFlight.
#   scripts/testflight.sh
# Needs the Mac set up as in fastlane/Fastfile: ~/.flockr-build holds the signing
# keychain (and its password), the App Store Connect API key, and the push key.
# Needs ASC_KEY_ID, ASC_ISSUER_ID, APPLE_TEAM_ID, APPLE_CERT_ID, and
# APPLE_SIGNING_IDENTITY in the environment; they are passed to fastlane on the Mac.
# Only native changes need a new build; JS loads from Metro.
set -euo pipefail
cd "$(dirname "$0")/.."
envs=""
for name in ASC_KEY_ID ASC_ISSUER_ID APPLE_TEAM_ID APPLE_CERT_ID APPLE_SIGNING_IDENTITY; do
  if [ -z "${!name:-}" ]; then
    echo "testflight.sh: $name is not set (see the top of fastlane/Fastfile)" >&2
    exit 1
  fi
  envs="$envs $name=$(printf '%q' "${!name}")"
done
# The Mac keeps its build copy in ~/flockr-mobile (prebuild output lives there).
rsync -a --delete --exclude node_modules --exclude .expo --exclude ios --exclude android --exclude dist ./ mac:flockr-mobile/
ssh mac "export PATH=/opt/homebrew/bin:\$PATH LANG=en_US.UTF-8 FASTLANE_SKIP_UPDATE_CHECK=1 FASTLANE_HIDE_CHANGELOG=1$envs
cd ~/flockr-mobile && npm ci --no-audit --no-fund >/dev/null
APP_VARIANT=development npx expo prebuild -p ios --clean >/dev/null
fastlane ios beta"
