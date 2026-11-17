#!/usr/bin/env bash
# Retakes the README's screenshots of the web UI, docs/screenshots/desktop.png
# and docs/screenshots/mobile.png, from a scene of fake repos the app fetches
# (tests/screenshots.rs). Needs git and Chrome or Chromium, like `cargo test`;
# set GIT_ARCHIVER_TEST_BROWSER to pick the browser.
#
# Usage: scripts/update-screenshots.sh
set -euo pipefail

cd "$(dirname "$0")/.."
cargo test --locked --test screenshots -- --ignored --exact readme_screenshots --nocapture
