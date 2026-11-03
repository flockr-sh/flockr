#!/usr/bin/env bash
# Drive the Android emulator for UI testing.
#   scripts/emu.sh boot              start the headless flockr-pixel AVD and wait for boot
#   scripts/emu.sh open [URL]        open the app in Expo Go (default exp://100.92.14.3:8081)
#   scripts/emu.sh shot FILE.png     save a screenshot (1080x2400)
#   scripts/emu.sh tap X Y           tap at screen pixels
#   scripts/emu.sh swipe X1 Y1 X2 Y2 [MS]
#   scripts/emu.sh text 'STRING'     type into the focused field
#   scripts/emu.sh key KEYCODE       e.g. 4 (back), 66 (enter), 111 (escape)
#   scripts/emu.sh stop              shut the emulator down
# EMU_GPU overrides the GPU mode (default host; Skia fails to start on swiftshader_indirect).
set -euo pipefail

SDK="${ANDROID_HOME:-$HOME/Android/Sdk}"
ADB="$SDK/platform-tools/adb"
AVD="flockr-pixel"

case "${1:-}" in
  boot)
    if ! "$ADB" get-state >/dev/null 2>&1; then
      nohup "$SDK/emulator/emulator" -avd "$AVD" -no-window -gpu "${EMU_GPU:-host}" -no-audio \
        -no-boot-anim -no-snapshot-save >"${TMPDIR:-/tmp}/flockr-emulator.log" 2>&1 &
    fi
    "$ADB" wait-for-device
    until [ "$("$ADB" shell getprop sys.boot_completed 2>/dev/null | tr -d '\r')" = 1 ]; do sleep 2; done
    echo booted
    ;;
  open) "$ADB" shell am start -a android.intent.action.VIEW -d "${2:-exp://100.92.14.3:8081}" >/dev/null ;;
  shot) "$ADB" exec-out screencap -p >"$2" ;;
  tap) "$ADB" shell input tap "$2" "$3" ;;
  swipe) "$ADB" shell input swipe "$2" "$3" "$4" "$5" "${6:-300}" ;;
  text) "$ADB" shell "input text '${2// /%s}'" ;;
  key) "$ADB" shell input keyevent "$2" ;;
  stop) "$ADB" emu kill ;;
  *) sed -n '2,11p' "$0"; exit 1 ;;
esac
