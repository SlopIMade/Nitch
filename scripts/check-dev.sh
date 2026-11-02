#!/usr/bin/env bash
# Reports the state of the WSL dev environment. Changes nothing.
# Run inside WSL:  bash <repo>/scripts/check-dev.sh
# Exit code is non-zero if something required is missing.

fail=0
ok()   { printf '  \e[32mok\e[0m        %s\n' "$*"; }
miss() { printf '  \e[31mmissing\e[0m   %s\n' "$*"; fail=1; }
info() { printf '  \e[33m--\e[0m        %s\n' "$*"; }

echo "Tools"
for cmd in cargo rustc pkg-config gst-inspect-1.0 docker vainfo sqlite3; do
  if command -v "$cmd" >/dev/null 2>&1; then ok "$cmd"; else miss "$cmd"; fi
done

echo "Versions"
command -v rustc >/dev/null 2>&1 && info "$(rustc --version)"
pkg-config --exists gstreamer-1.0 2>/dev/null \
  && info "GStreamer $(pkg-config --modversion gstreamer-1.0)" \
  || miss "GStreamer dev libraries (pkg-config gstreamer-1.0)"
command -v docker >/dev/null 2>&1 && info "$(docker --version)"

echo "GStreamer elements (required)"
for e in compositor hlsdemux2 decodebin3 videoconvertscale x264enc avdec_h264 h264parse avenc_aac; do
  if gst-inspect-1.0 "$e" >/dev/null 2>&1; then ok "$e"; else miss "$e"; fi
done

echo "GStreamer elements (optional, hardware)"
for e in vah264dec vah264enc vah264lpenc; do
  if gst-inspect-1.0 "$e" >/dev/null 2>&1; then ok "$e"; else info "$e not available"; fi
done

echo "Docker"
if docker info >/dev/null 2>&1; then
  ok "daemon reachable"
else
  miss "daemon reachable (in the docker group? reopened WSL after setup?)"
fi

echo "Environment"
if [ -n "${CARGO_TARGET_DIR:-}" ]; then ok "CARGO_TARGET_DIR=$CARGO_TARGET_DIR"; else miss "CARGO_TARGET_DIR"; fi

exit $fail
