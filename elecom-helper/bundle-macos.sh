#!/bin/sh
# Build a new bundle; never remove or silently replace an existing artifact.
set -eu
helper_root=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
ui_dist=${1:?Usage: bundle-macos.sh /absolute/path/to/app/dist-elecom [output.app]}
output=${2:-"$helper_root/dist/$(date +%Y%m%d-%H%M%S)/OpenMouse EX-G PRO.app"}
test -f "$ui_dist/elecom.html" || { echo "Offline UI is missing: $ui_dist" >&2; exit 1; }
test ! -e "$output" || { echo "Output already exists; choose a new output path: $output" >&2; exit 1; }
cargo build --locked --release --manifest-path "$helper_root/Cargo.toml"
mkdir -p "$output/Contents/MacOS" "$output/Contents/Resources/ui"
cp "$helper_root/target/release/openmouse-elecom" "$output/Contents/MacOS/openmouse-elecom"
# Copy only this build's referenced resources, leaving old local build files alone.
python3 - "$ui_dist" "$output/Contents/Resources/ui" <<'PY'
from html.parser import HTMLParser
from pathlib import Path
import shutil, sys
source, destination = map(Path, sys.argv[1:])
class Assets(HTMLParser):
    def handle_starttag(self, tag, attrs):
        for name, value in attrs:
            if name in ('src', 'href') and value:
                path = Path(value.lstrip('/'))
                if path.parts and path.parts[0] == 'assets' and '..' not in path.parts:
                    target = destination / path
                    target.parent.mkdir(parents=True, exist_ok=True)
                    shutil.copy2(source / path, target)
html = (source / 'elecom.html').read_text()
(destination / 'elecom.html').write_text(html)
Assets().feed(html)
PY
cp "$helper_root/README.md" "$output/Contents/Resources/README.md"
cp "$helper_root/LICENSE" "$output/Contents/Resources/LICENSE-helper"
if test -f "$ui_dist/../LICENSE"; then cp "$ui_dist/../LICENSE" "$output/Contents/Resources/LICENSE-ui"; fi
cat > "$output/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleExecutable</key><string>openmouse-elecom</string>
<key>CFBundleIdentifier</key><string>io.openmouse.elecom</string>
<key>CFBundleName</key><string>OpenMouse EX-G PRO</string>
<key>CFBundlePackageType</key><string>APPL</string>
<key>CFBundleShortVersionString</key><string>0.2.0</string>
<key>CFBundleVersion</key><string>2</string>
<key>LSUIElement</key><true/>
<key>LSMinimumSystemVersion</key><string>12.0</string>
<key>NSHighResolutionCapable</key><true/>
</dict></plist>
PLIST
codesign --force --sign - --identifier io.openmouse.elecom "$output"
codesign --verify --deep --strict "$output"
echo "$output"
