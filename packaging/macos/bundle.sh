#!/usr/bin/env bash
# Build OpenSuperCAD.app (universal: arm64 + x86_64) and a .dmg.
# usage: packaging/macos/bundle.sh <version> <out-dir>
# Expects target/{aarch64,x86_64}-apple-darwin/release/{opensupercad,opensupercad-mcp}.
set -euo pipefail
version=$1
out=$2
app="$out/OpenSuperCAD.app"
rm -rf "$app" && mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"

for bin in opensupercad opensupercad-mcp; do
  lipo -create \
    "target/aarch64-apple-darwin/release/$bin" \
    "target/x86_64-apple-darwin/release/$bin" \
    -output "$app/Contents/MacOS/$bin"
done
sed "s/@VERSION@/$version/g" packaging/macos/Info.plist > "$app/Contents/Info.plist"

# Icon: render the SVG to an .icns (qlmanage + sips + iconutil ship with macOS).
iconset="$out/OpenSuperCAD.iconset"
mkdir -p "$iconset"
qlmanage -t -s 1024 -o "$out" packaging/linux/opensupercad.svg >/dev/null 2>&1 || true
if [[ -f "$out/opensupercad.svg.png" ]]; then
  for size in 16 32 128 256 512; do
    sips -z $size $size "$out/opensupercad.svg.png" --out "$iconset/icon_${size}x${size}.png" >/dev/null
    sips -z $((size * 2)) $((size * 2)) "$out/opensupercad.svg.png" --out "$iconset/icon_${size}x${size}@2x.png" >/dev/null
  done
  iconutil -c icns "$iconset" -o "$app/Contents/Resources/OpenSuperCAD.icns"
fi
rm -rf "$iconset" "$out/opensupercad.svg.png"

# Ad-hoc signature so Gatekeeper reports "unidentified developer" rather than "damaged".
codesign --force --deep --sign - "$app"

dmg="$out/OpenSuperCAD-$version-macos-universal.dmg"
staging="$out/dmg"
rm -rf "$staging" && mkdir -p "$staging"
cp -R "$app" "$staging/"
ln -s /Applications "$staging/Applications"
hdiutil create -volname "OpenSuperCAD $version" -srcfolder "$staging" -ov -format UDZO "$dmg"
rm -rf "$staging"
echo "$dmg"
