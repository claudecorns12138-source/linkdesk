#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
export MACOSX_DEPLOYMENT_TARGET=14.4
export CARGO_ENCODED_RUSTFLAGS="$(printf '%s\037%s\037%s\037%s' --remap-path-prefix "$PWD=linkdesk" --remap-path-prefix "$HOME=local-user")"
cargo build --release --locked --target-dir target
app="dist/LinkDesk.app/Contents"
mkdir -p "$app/MacOS" "$app/Resources"
xcrun clang++ -std=c++17 -Wall -Wextra -Werror native/macos/layout_test.cpp -o target/layout-guard-tests
target/layout-guard-tests
xcrun clang++ -std=c++17 -fobjc-arc -O2 -mmacosx-version-min=14.4 -ffile-prefix-map="$HOME=local-user" \
  -framework Foundation -framework CoreGraphics -framework ScreenCaptureKit \
  -framework VideoToolbox -framework CoreMedia -framework CoreVideo \
  native/macos/capture.mm -o "$app/MacOS/linkdesk-mac-helper"
cp target/release/linkdesk "$app/MacOS/linkdesk"
cp native/macos/Info.plist "$app/Info.plist"
cp LICENSE THIRD_PARTY_NOTICES.md "$app/Resources/"
codesign --force --sign - "$app/MacOS/linkdesk-mac-helper"
codesign --force --deep --sign - dist/LinkDesk.app
python3 scripts/test-macos-helper-lifecycle.py "$app/MacOS/linkdesk-mac-helper"
printf 'Built dist/LinkDesk.app (ad-hoc signed development build).\n'
