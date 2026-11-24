#!/bin/sh
# Builds Factory.app from the Swift package, for personal use. It is not
# signed with a Developer ID and is not sandboxed: it talks to an engine that
# starts command-line tools.
set -eu
cd "$(dirname "$0")"
swift build -c release
app="build/Factory.app"
rm -rf "$app"
mkdir -p "$app/Contents/MacOS"
cp .build/release/FactoryApp "$app/Contents/MacOS/Factory"
cat > "$app/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>Factory</string>
  <key>CFBundleIdentifier</key><string>dev.factory.app</string>
  <key>CFBundleExecutable</key><string>Factory</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>0.1.0</string>
  <key>CFBundleVersion</key><string>1</string>
  <key>LSMinimumSystemVersion</key><string>14.0</string>
  <key>NSHighResolutionCapable</key><true/>
</dict>
</plist>
PLIST
codesign --force --sign - "$app" >/dev/null 2>&1 || true
echo "built $(pwd)/$app"
