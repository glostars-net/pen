#!/bin/bash
# Builds Pen.app and a drag-to-Applications disk image.
# shortcut: ad-hoc signature only. Gatekeeper still blocks the download until a Developer ID certificate and notarization credentials are available.
set -euo pipefail

version="${GITHUB_REF_NAME:-0.1.0}"
version="${version#v}"
tmp="${RUNNER_TEMP:-${TMPDIR:-/tmp}}"
app="$tmp/Pen.app"
stage="$tmp/pen-dmg-stage"
rw="$tmp/pen-rw.dmg"
mount="$tmp/pen-dmg-mount"

rm -rf "$app" "$stage" "$mount" "$rw"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources" "$stage"

cp target/release/pen "$app/Contents/MacOS/pen"
chmod 755 "$app/Contents/MacOS/pen"
cp assets/icons/pen.icns "$app/Contents/Resources/pen.icns"
sed "s/0\\.1\\.0/${version}/g" packaging/macos/Info.plist > "$app/Contents/Info.plist"
printf 'APPL????' > "$app/Contents/PkgInfo"

identity="${APPLE_SIGNING_IDENTITY:-}"
if [ -z "$identity" ]; then
  identity=$(security find-identity -p codesigning -v 2>/dev/null | awk -F'"' '/Developer ID Application/{print $2; exit}')
fi
if [ -n "$identity" ]; then
  codesign --force --sign "$identity" "$app"
else
  codesign --force --sign - "$app"
fi

cp -R "$app" "$stage/Pen.app"
ln -s /Applications "$stage/Applications"

hdiutil create -volname Pen -srcfolder "$stage" -ov -format UDRW -fs HFS+ "$rw"
hdiutil resize -size "$(( $(stat -f%z "$rw") / 1048576 + 32 ))m" "$rw"
mkdir -p "$mount"
hdiutil attach -readwrite -noverify -noautoopen -mountpoint "$mount" "$rw"
cp assets/icons/pen.icns "$mount/.VolumeIcon.icns"
setfile=$(xcrun --find SetFile 2>/dev/null || true)
if [ -n "$setfile" ]; then
  "$setfile" -a C "$mount" || true
fi

# Best-effort Finder layout. The Applications link is the install step either way.
osascript - "$mount" <<'EOF' >/dev/null 2>&1 &
on run argv
  set mountPath to item 1 of argv
  tell application "Finder"
    set diskAlias to (POSIX file mountPath) as alias
    open diskAlias
    set win to container window of diskAlias
    set current view of win to icon view
    set toolbar visible of win to false
    set statusbar visible of win to false
    set bounds of win to {200, 120, 840, 440}
    set viewOptions to icon view options of win
    set arrangement of viewOptions to not arranged
    set icon size of viewOptions to 96
    set position of item "Pen.app" of win to {160, 160}
    set position of item "Applications" of win to {480, 160}
    update diskAlias without registering applications
    delay 1
    close win
  end tell
end run
EOF
layout_pid=$!
for _ in 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20 21 22 23 24 25 26 27 28 29 30 31 32 33 34 35 36 37 38 39 40; do
  if ! kill -0 "$layout_pid" 2>/dev/null; then
    break
  fi
  sleep 1
done
kill "$layout_pid" 2>/dev/null || true
wait "$layout_pid" 2>/dev/null || true

rm -rf "$mount/.fseventsd"
sync
hdiutil detach "$mount" || hdiutil detach -force "$mount"
rm -f Pen-macOS.dmg
hdiutil convert "$rw" -format UDZO -o Pen-macOS.dmg

check="$tmp/pen-dmg-check"
rm -rf "$check"
mkdir -p "$check"
hdiutil attach -readonly -nobrowse -mountpoint "$check" Pen-macOS.dmg
test -x "$check/Pen.app/Contents/MacOS/pen"
test -f "$check/Pen.app/Contents/Resources/pen.icns"
test -f "$check/Pen.app/Contents/Info.plist"
test -L "$check/Applications"
test "$(readlink "$check/Applications")" = "/Applications"
hdiutil detach "$check"
rm -rf "$rw"
