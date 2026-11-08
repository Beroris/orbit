#!/bin/sh
# Builds platforms/macos/build/Orbit.app: the Rust core, the Swift host, an icon, an Info.plist with
# LSUIElement (no Dock icon), and an ad-hoc signature. Needs Rust and the Command Line Tools only.
#   scripts/macos/build_app.sh [--out DIR]
set -eu

root=$(cd "$(dirname "$0")/../.." && pwd)
out="$root/platforms/macos/build"
if [ "${1:-}" = "--out" ]; then out=$2; fi
package="$root/platforms/macos"

echo "== Rust core"
(cd "$root" && cargo build --release -p orbit-ffi)

# Some Command Line Tools installs keep stale Swift 5.10 *.private.swiftinterface files in
# ManifestAPI, which break the manifest of every Swift package with an "Undefined symbols" link
# error. When that's the case, use a copy without them, under this repo's build folder. The system
# copy is left alone. Reinstalling the Command Line Tools fixes it properly.
libs="/Library/Developer/CommandLineTools/usr/lib/swift/pm"
if ls "$libs"/ManifestAPI/PackageDescription.swiftmodule/*.private.swiftinterface >/dev/null 2>&1 \
    && ! (cd "$package" && swift package describe >/dev/null 2>&1); then
    echo "== Stale SwiftPM files in the Command Line Tools: using a cleaned copy for this build"
    clean="$package/.build/swiftpm-libs"
    rm -rf "$clean"
    mkdir -p "$clean"
    cp -R "$libs/ManifestAPI" "$clean/ManifestAPI"
    cp -R "$libs/PluginAPI" "$clean/PluginAPI"
    find "$clean" -name '*.private.swiftinterface' -delete
    SWIFTPM_CUSTOM_LIBS_DIR="$clean"
    export SWIFTPM_CUSTOM_LIBS_DIR
fi

echo "== Swift host"
(cd "$package" && swift build -c release)
bin=$(cd "$package" && swift build -c release --show-bin-path)

app="$out/Orbit.app"
rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp "$bin/Orbit" "$app/Contents/MacOS/Orbit"
strip -x "$app/Contents/MacOS/Orbit" 2>/dev/null || true
cp "$bin/OrbitBenchSink" "$out/OrbitBenchSink"

version=$(grep -m1 '^version' "$root/Cargo.toml" | sed 's/.*"\(.*\)".*/\1/')
cat > "$app/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleExecutable</key><string>Orbit</string>
    <key>CFBundleIdentifier</key><string>app.orbit.launcher</string>
    <key>CFBundleName</key><string>Orbit</string>
    <key>CFBundleDisplayName</key><string>Orbit</string>
    <key>CFBundlePackageType</key><string>APPL</string>
    <key>CFBundleShortVersionString</key><string>$version</string>
    <key>CFBundleVersion</key><string>$version</string>
    <key>CFBundleIconFile</key><string>Orbit</string>
    <key>LSMinimumSystemVersion</key><string>14.0</string>
    <key>LSUIElement</key><true/>
    <key>NSHighResolutionCapable</key><true/>
    <key>NSAppleEventsUsageDescription</key><string>Orbit asks Finder to empty the trash, only when you choose that command.</string>
</dict>
</plist>
PLIST

# The icon, from assets/icon.svg. sips reads SVG on current macOS. If it can't, the app still works,
# just with a generic icon, so this doesn't stop the build.
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
if sips -s format png -z 1024 1024 "$root/assets/icon.svg" --out "$work/icon.png" >/dev/null 2>&1; then
    set_dir="$work/Orbit.iconset"
    mkdir -p "$set_dir"
    for size in 16 32 128 256 512; do
        sips -z $size $size "$work/icon.png" --out "$set_dir/icon_${size}x${size}.png" >/dev/null
        double=$((size * 2))
        sips -z $double $double "$work/icon.png" --out "$set_dir/icon_${size}x${size}@2x.png" >/dev/null
    done
    iconutil -c icns "$set_dir" -o "$app/Contents/Resources/Orbit.icns"
    echo "== Icon built from assets/icon.svg"
else
    echo "== sips couldn't read assets/icon.svg: no custom icon in this build"
fi

codesign --force --sign - --timestamp=none "$app"
codesign --verify "$app"
echo "== Built $app"
du -sh "$app"
