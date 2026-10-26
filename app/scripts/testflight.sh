#!/usr/bin/env bash
# Build the flockr dev app on the Mac and upload it to TestFlight.
#   scripts/testflight.sh
# Needs the Mac set up as in fastlane/Fastfile: ~/.flockr-build holds the signing
# keychain (and its password), the App Store Connect API key, and the push key.
# Only native changes need a new build; JS loads from Metro.
set -euo pipefail
cd "$(dirname "$0")/.."
rsync -a --delete --exclude node_modules --exclude .expo --exclude ios --exclude android --exclude dist ./ mac:flockr-mobile/
ssh mac 'export PATH=/opt/homebrew/bin:$PATH LANG=en_US.UTF-8 FASTLANE_SKIP_UPDATE_CHECK=1 FASTLANE_HIDE_CHANGELOG=1
cd ~/flockr-mobile && npm ci --no-audit --no-fund >/dev/null
APP_VARIANT=development npx expo prebuild -p ios --clean >/dev/null
fastlane ios beta'
