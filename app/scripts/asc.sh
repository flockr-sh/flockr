#!/bin/zsh
# asc.sh METHOD PATH [JSON]: call the App Store Connect API with a fresh token.
# Runs on the Mac, in its build copy (~/flockr-mobile, synced by testflight.sh).
# Needs ASC_KEY_ID and ASC_ISSUER_ID in the environment (see fastlane/Fastfile).
for name in ASC_KEY_ID ASC_ISSUER_ID; do
  if [[ -z "${(P)name}" ]]; then
    echo "asc.sh: $name is not set (see the top of fastlane/Fastfile)" >&2
    exit 1
  fi
done
cd ~/flockr-mobile && FASTLANE_SKIP_UPDATE_CHECK=1 /opt/homebrew/bin/fastlane ios api_token >/dev/null 2>&1 || {
  echo "asc.sh: fastlane api_token failed" >&2
  exit 1
}
TOK=$(cat ~/.flockr-build/token)
if [[ -n "$3" ]]; then
  curl -gs -X "$1" -H "Authorization: Bearer $TOK" -H "Content-Type: application/json" -d "$3" "https://api.appstoreconnect.apple.com/v1/$2"
else
  curl -gs -X "$1" -H "Authorization: Bearer $TOK" "https://api.appstoreconnect.apple.com/v1/$2"
fi
