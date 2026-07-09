#!/usr/bin/env bash
# build-app.sh — wrap the compiled macOS binary into a double-clickable Talos.app.
#
# `deno compile` produces a bare Mach-O binary; macOS wants an .app BUNDLE (a
# folder with a conventional layout) for a GUI app to double-click, show an icon,
# and sit in the Dock. This is DISTRIBUTION PACKAGING, not build — run it AFTER
# `deno task build:mac` has produced dist/Talos.
#
# Layout produced:
#   dist/Talos.app/Contents/
#     Info.plist            metadata (name, id, icon)
#     MacOS/Talos           the binary (from dist/Talos)
#     MacOS/bundles/        integrator content (engine reads it beside the binary)
#     Resources/Talos.icns  icon, generated from public/talos.svg
#
# NOTE: bundles live INSIDE the .app so the engine's "bundles beside the exe"
# lookup finds them unchanged — this seals them in, unlike the Windows OneDrive
# model where bundles/ sits beside Talos.exe and an integrator swaps them freely.
# Acceptable for the Mac dev/test target; revisit if Mac becomes a real
# integrator-distribution surface.
#
# NOT signed/notarised: Gatekeeper warns on other Macs until you sign with an
# Apple Developer cert (codesign + xcrun notarytool). This builds the unsigned
# bundle — structure correct, the trust step is yours to add.
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
dist="$root/dist"
bin="$dist/Talos"
svg="$root/public/talos.svg"

[ -f "$bin" ] || { echo "dist/Talos not found — run 'deno task build:mac' first."; exit 1; }

app="$dist/Talos.app"
contents="$app/Contents"
rm -rf "$app"
mkdir -p "$contents/MacOS" "$contents/Resources"

# Binary + bundles inside the app (engine reads bundles beside the binary).
cp "$bin" "$contents/MacOS/Talos"
cp -r "$dist/bundles" "$contents/MacOS/bundles"

# Icon: SVG -> .iconset (all sizes macOS expects) -> .icns.
iconset="$dist/Talos.iconset"
rm -rf "$iconset"; mkdir "$iconset"
while read -r sz nm; do
  rsvg-convert -w "$sz" -h "$sz" "$svg" -o "$iconset/$nm"
done <<'SIZES'
16 icon_16x16.png
32 icon_16x16@2x.png
32 icon_32x32.png
64 icon_32x32@2x.png
128 icon_128x128.png
256 icon_128x128@2x.png
256 icon_256x256.png
512 icon_256x256@2x.png
512 icon_512x512.png
1024 icon_512x512@2x.png
SIZES
iconutil -c icns "$iconset" -o "$contents/Resources/Talos.icns"
rm -rf "$iconset"

# Info.plist — metadata macOS reads to treat this as a GUI app.
cat > "$contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key>               <string>Talos</string>
  <key>CFBundleDisplayName</key>        <string>Talos</string>
  <key>CFBundleIdentifier</key>         <string>be.lemer.talos</string>
  <key>CFBundleVersion</key>            <string>0.0.0</string>
  <key>CFBundleShortVersionString</key> <string>0.0.0</string>
  <key>CFBundleExecutable</key>         <string>Talos</string>
  <key>CFBundleIconFile</key>           <string>Talos.icns</string>
  <key>CFBundlePackageType</key>        <string>APPL</string>
  <key>LSMinimumSystemVersion</key>     <string>11.0</string>
  <key>NSHighResolutionCapable</key>    <true/>
</dict>
</plist>
PLIST

plutil -lint "$contents/Info.plist" >/dev/null
echo "built $app (unsigned — Gatekeeper will warn on other Macs until codesign + notarize)"
