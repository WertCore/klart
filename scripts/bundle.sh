#!/usr/bin/env bash
#
# Assembles dist/Klart.app.
#
# A menu bar agent has to be a bundle rather than a bare binary: `LSUIElement`
# lives in Info.plist, and without it macOS gives the agent a Dock tile the
# moment it is launched from Finder. The activation policy the agent sets for
# itself covers the case where it is run from a shell; this covers the other one.
#
# Usage: scripts/bundle.sh [version]

set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

# The workspace version unless a tag says otherwise, so a release names itself
# after the tag and a local build names itself after the manifest.
version="${1:-$(sed -n 's/^version = "\(.*\)"$/\1/p' Cargo.toml | head -1)}"

# arm64 only, and deliberately. The IORegistry walk that finds a monitor's name
# and its I2C channel matches `AppleCLCD2`, which Intel Macs do not publish —
# enumeration would work there and both hardware mechanisms would not. Shipping a
# half of a universal binary that is known to be degraded and has never been run
# is worse than shipping one architecture and saying so.
target="aarch64-apple-darwin"

app="dist/Klart.app"
contents="$app/Contents"

echo "building klart $version for $target"
cargo build --release --workspace --target "$target"

rm -rf "$app"
mkdir -p "$contents/MacOS" "$contents/Resources"

# Both binaries. The agent is what the bundle launches; the command line rides
# along so that one download is the whole thing, and so `klart` can be symlinked
# onto a PATH from inside the bundle.
cp "target/$target/release/klart-tray" "$contents/MacOS/"
cp "target/$target/release/klart" "$contents/MacOS/"

cat > "$contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>CFBundleName</key>
	<string>Klart</string>
	<key>CFBundleDisplayName</key>
	<string>Klart</string>
	<key>CFBundleIdentifier</key>
	<string>com.wertcore.klart</string>
	<key>CFBundleExecutable</key>
	<string>klart-tray</string>
	<key>CFBundlePackageType</key>
	<string>APPL</string>
	<key>CFBundleShortVersionString</key>
	<string>$version</string>
	<key>CFBundleVersion</key>
	<string>$version</string>
	<key>CFBundleInfoDictionaryVersion</key>
	<string>6.0</string>
	<!-- The whole reason this is a bundle: an agent with no Dock tile and no
	     entry in the application switcher. -->
	<key>LSUIElement</key>
	<true/>
	<key>NSHighResolutionCapable</key>
	<true/>
	<!-- The SF Symbol the menu bar icon uses arrived in Big Sur, and the
	     objc2 bindings this is built on target it. -->
	<key>LSMinimumSystemVersion</key>
	<string>11.0</string>
	<key>NSHumanReadableCopyright</key>
	<string>MIT or Apache-2.0</string>
</dict>
</plist>
PLIST

echo "wrote $app"
