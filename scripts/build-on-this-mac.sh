#!/bin/bash
# Builds Playback Rig on this Mac and installs it in Applications, for when GitHub's
# Mac build machines aren't available. Paste into Terminal:
#
#   curl -fsSL https://raw.githubusercontent.com/hellofirsthour-design/playback-rig/claude/trusting-hopper-f3xbf4/scripts/build-on-this-mac.sh | bash
#
# The first run installs the build tools (Apple's command line tools, Rust, CMake) and
# takes about 20-30 minutes; later runs take a few minutes. Safe to run again any time.
# The app keeps updating itself from GitHub as usual afterwards.
set -euo pipefail

BRANCH="claude/trusting-hopper-f3xbf4"
REPO="https://github.com/hellofirsthour-design/playback-rig.git"
SRC="$HOME/Library/Caches/PlaybackRigBuild/playback-rig"
APP_NAME="Playback Rig"

say() { printf '\n\033[1m==> %s\033[0m\n' "$*"; }
fail() { printf '\n\033[31mStopped: %s\033[0m\n' "$*"; exit 1; }

[ "$(uname)" = "Darwin" ] || fail "this script is for a Mac."

# 1. Apple's command line tools (compiler, git). macOS shows its own installer window.
if ! xcode-select -p >/dev/null 2>&1; then
  say "Installing Apple's command line tools. A window will pop up: click Install."
  xcode-select --install || true
  fail "when that install finishes, paste the same command again."
fi

# 2. Rust (the language the app is written in), installed just for your user.
if ! command -v cargo >/dev/null 2>&1; then
  [ -f "$HOME/.cargo/env" ] && . "$HOME/.cargo/env"
fi
if ! command -v cargo >/dev/null 2>&1; then
  say "Installing Rust (about 2 minutes)"
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal
  . "$HOME/.cargo/env"
fi

# 3. CMake (needed for the speech model that reads the guide track).
if ! command -v cmake >/dev/null 2>&1; then
  if command -v brew >/dev/null 2>&1; then
    say "Installing CMake with Homebrew"
    brew install cmake
  else
    say "Installing CMake (for your user only)"
    python3 -m pip install --user --quiet cmake
    PATH="$(python3 -m site --user-base)/bin:$PATH"
    export PATH
  fi
fi
command -v cmake >/dev/null 2>&1 || fail "CMake didn't install. Send Claude a screenshot of this window."

# 4. Tauri's build tool (makes the .app).
if ! cargo tauri --version 2>/dev/null | grep -q "tauri-cli 2"; then
  say "Installing the app build tool (about 5-10 minutes, once)"
  cargo install tauri-cli --version "^2" --locked
fi

# 5. The latest code.
if [ -d "$SRC/.git" ]; then
  say "Getting the latest code"
  git -C "$SRC" fetch --depth 1 origin "$BRANCH"
  git -C "$SRC" reset --hard FETCH_HEAD
else
  say "Downloading the code"
  mkdir -p "$(dirname "$SRC")"
  git clone --depth 1 --branch "$BRANCH" "$REPO" "$SRC"
fi
VERSION=$(python3 -c "import json;print(json.load(open('$SRC/apps/desktop/src-tauri/tauri.conf.json'))['version'])")

# 6. Build. Generic Apple Silicon/Intel build of the speech library; no update signing needed here.
say "Building Playback Rig $VERSION (about 5-10 minutes the first time)"
cd "$SRC/apps/desktop/src-tauri"
WHISPER_NATIVE=OFF cargo tauri build --bundles app --config '{"bundle":{"createUpdaterArtifacts":false}}'
BUILT="$SRC/apps/desktop/src-tauri/target/release/bundle/macos/$APP_NAME.app"
[ -d "$BUILT" ] || fail "the build finished but the app wasn't where expected. Send Claude a screenshot."

# 7. Install: quit the running app, replace it in Applications, open it.
say "Installing in Applications"
osascript -e "quit app \"$APP_NAME\"" >/dev/null 2>&1 || true
sleep 2
rm -rf "/Applications/$APP_NAME.app"
ditto "$BUILT" "/Applications/$APP_NAME.app"
open "/Applications/$APP_NAME.app"
say "Done: Playback Rig $VERSION is installed and open."
