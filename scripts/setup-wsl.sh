#!/usr/bin/env bash
# One-time setup of the Linux dev environment in WSL2 (Ubuntu 24.04).
# Run inside WSL:  bash <repo>/scripts/setup-wsl.sh
# Safe to run again; asks for your sudo password.
set -euo pipefail

sudo apt-get update
sudo apt-get install -y \
  build-essential pkg-config git curl libssl-dev sqlite3 \
  libgstreamer1.0-dev libgstreamer-plugins-base1.0-dev libgstreamer-plugins-bad1.0-dev \
  gstreamer1.0-tools gstreamer1.0-plugins-base gstreamer1.0-plugins-good \
  gstreamer1.0-plugins-bad gstreamer1.0-plugins-ugly gstreamer1.0-libav \
  vainfo mesa-va-drivers \
  docker.io docker-compose-v2 docker-buildx

sudo usermod -aG docker "$USER"

if [ ! -x "$HOME/.cargo/bin/rustup" ]; then
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
fi

# Build output on the Linux disk instead of /mnt/<drive>. ~/.profile is read by
# login shells (WSL terminals) and by `bash -l`, which tooling uses.
line='export CARGO_TARGET_DIR="$HOME/.cache/nitch-target"'
grep -qxF "$line" "$HOME/.profile" || echo "$line" >> "$HOME/.profile"

echo
echo "Done. Now run 'wsl --shutdown' in Windows, reopen WSL, and check with:"
echo "  bash <repo>/scripts/check-dev.sh"
