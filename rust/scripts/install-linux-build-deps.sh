#!/usr/bin/env bash
# The libraries the window and fonts build against on Linux (Ubuntu).
set -euo pipefail
sudo apt-get update -q
sudo apt-get install -yq --no-install-recommends \
  pkg-config libxkbcommon-dev libxkbcommon-x11-dev libwayland-dev libx11-dev libx11-xcb-dev \
  libxcursor-dev libxrandr-dev libxi-dev libfontconfig-dev libfreetype-dev libvulkan-dev
