#!/bin/zsh
# asc.sh METHOD PATH [JSON]: call the App Store Connect API with a fresh token.
cd ~/flockr-mobile && FASTLANE_SKIP_UPDATE_CHECK=1 /opt/homebrew/bin/fastlane ios api_token >/dev/null 2>&1
TOK=$(cat ~/.flockr-build/token)
if [[ -n "$3" ]]; then
  curl -gs -X "$1" -H "Authorization: Bearer $TOK" -H "Content-Type: application/json" -d "$3" "https://api.appstoreconnect.apple.com/v1/$2"
else
  curl -gs -X "$1" -H "Authorization: Bearer $TOK" "https://api.appstoreconnect.apple.com/v1/$2"
fi
