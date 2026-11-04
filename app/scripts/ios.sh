#!/usr/bin/env bash
# Drive the iOS Simulator on the Mac over SSH (host alias `mac` in ~/.ssh/config).
# UI actions go through MobileBuildMCP's CLI; coordinate taps use its bundled AXe.
#   scripts/ios.sh boot              boot the simulator and open Simulator.app
#   scripts/ios.sh open [URL]        open the app in Expo Go (default exp://100.92.14.3:8081)
#   scripts/ios.sh snap              list tappable elements with their refs
#   scripts/ios.sh tap REF           tap an element ref from the latest snap
#   scripts/ios.sh tapl LABEL        snap, then tap the first element whose label starts with LABEL
#   scripts/ios.sh tapxy X Y         tap at screen points (for the Skia terminal view)
#   scripts/ios.sh touch X Y         touch down/up at points (switches ignore tapxy)
#   scripts/ios.sh swipexy X1 Y1 X2 Y2 [SECONDS]
#   scripts/ios.sh type REF 'TEXT'   type into a field ref from the latest snap
#   scripts/ios.sh swipe REF up|down|left|right [DISTANCE]
#   scripts/ios.sh shot FILE.png     save a screenshot here (1206x2622 pixels, 3x points)
#   scripts/ios.sh stop              shut the simulator down
# IOS_SIM overrides the simulator UDID (default: iPhone 17 Pro).
set -euo pipefail

SIM="${IOS_SIM:-CAA6E3C0-3D3D-490D-88C2-EE57BB15B999}"
remote() { ssh mac "export PATH=/opt/homebrew/bin:\$PATH; SIM=$SIM; m() { npx -y mobilebuildmcp@latest "\$@"; }; $1"; }
q() { printf '%q' "$1"; }
axe() { remote "AXE=\$(ls ~/.npm/_npx/*/node_modules/mobilebuildmcp/bundled/axe | head -1); \$AXE $1 --udid \$SIM" >/dev/null; }

case "${1:-}" in
  boot) remote 'xcrun simctl boot $SIM 2>/dev/null || true; xcrun simctl bootstatus $SIM >/dev/null; open -a Simulator; echo booted' ;;
  open) remote "xcrun simctl launch \$SIM host.exp.Exponent >/dev/null; sleep 3; xcrun simctl openurl \$SIM $(q "${2:-exp://100.92.14.3:8081}")" ;;
  snap) remote 'm ui-automation snapshot-ui --simulator-id $SIM' | sed -n '/^Targets/,/^Tips/p' | sed '$d' ;;
  tap) remote "m ui-automation tap --simulator-id \$SIM --element-ref $(q "$2")" >/dev/null ;;
  tapl)
    ref=$("$0" snap | awk -F'|' -v l="$2" 'index($4, l) == 1 { gsub(/ /, "", $1); print $1; exit }')
    [ -n "$ref" ] || { echo "no element labeled '$2'" >&2; exit 1; }
    "$0" tap "$ref" ;;
  tapxy) axe "tap -x $(q "$2") -y $(q "$3")" ;;
  touch) axe "touch -x $(q "$2") -y $(q "$3") --down --up --delay 0.15" ;;
  swipexy)
    axe "swipe --start-x $(q "$2") --start-y $(q "$3") --end-x $(q "$4") --end-y $(q "$5") --duration $(q "${6:-0.4}")" ;;
  type) remote "m ui-automation type-text --simulator-id \$SIM --element-ref $(q "$2") --text $(q "$3")" >/dev/null ;;
  swipe)
    remote "m ui-automation swipe --simulator-id \$SIM --within-element-ref $(q "$2") --direction $(q "$3") --distance $(q "${4:-0.5}")" >/dev/null ;;
  shot) remote 'xcrun simctl io $SIM screenshot /tmp/flockr-sim.png >/dev/null 2>&1 && cat /tmp/flockr-sim.png' >"$2" ;;
  stop) remote 'xcrun simctl shutdown $SIM' ;;
  *) sed -n '2,16p' "$0"; exit 1 ;;
esac
