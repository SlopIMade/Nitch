#!/usr/bin/env bash
# Generates the 2 s "Ad break" slate segments the server inserts when no ad-free source is live (D15).
#   - slate.ts                     for MPEG-TS streams
#   - slate-init.mp4 + slate.m4s   for fMP4 streams (they need their own EXT-X-MAP)
# Run inside WSL:  bash <repo>/scripts/make-slate.sh
set -euo pipefail

out="$(cd "$(dirname "$0")/.." && pwd)/server/crates/nitch-server/assets"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
mkdir -p "$out"

# 60 frames at 30 fps and 100 × 960 samples at 48 kHz: exactly 2 s each. H.264 High, 4:2:0, 8 bit (S4).
video='videotestsrc pattern=solid-color foreground-color=0xff18181f num-buffers=60
  ! video/x-raw,width=1280,height=720,framerate=30/1
  ! textoverlay text="Ad break" font-desc="Sans Bold 44" valignment=center halignment=center deltay=-45
  ! textoverlay text="The stream continues in a moment" font-desc="Sans 18" valignment=center halignment=center deltay=45
  ! videoconvert ! video/x-raw,format=I420
  ! x264enc key-int-max=60 bitrate=300 speed-preset=medium ! video/x-h264,profile=high ! h264parse ! queue'
audio='audiotestsrc wave=silence num-buffers=100 samplesperbuffer=960
  ! audio/x-raw,rate=48000,channels=2 ! avenc_aac ! aacparse ! queue'

# shellcheck disable=SC2086 # the pipelines are meant to be word-split
gst-launch-1.0 -q -e $video ! mux. $audio ! mux. mpegtsmux name=mux ! filesink location="$out/slate.ts"
# shellcheck disable=SC2086
gst-launch-1.0 -q -e $video ! mux. $audio ! mux. \
  mp4mux name=mux fragment-duration=5000 streamable=true ! filesink location="$tmp/slate.mp4"

# Split the fragmented MP4: init = ftyp + moov, media = moof + mdat (mfra, if any, is dropped).
python3 -I - "$tmp/slate.mp4" "$out/slate-init.mp4" "$out/slate.m4s" <<'EOF'
import struct, sys
data = open(sys.argv[1], "rb").read()
boxes, pos = [], 0
while pos < len(data):
    size, kind = struct.unpack(">I4s", data[pos:pos + 8])
    boxes.append((kind.decode(), data[pos:pos + size]))
    pos += size
init = b"".join(b for k, b in boxes if k in ("ftyp", "moov"))
media = b"".join(b for k, b in boxes if k in ("styp", "moof", "mdat"))
assert init and media, [k for k, _ in boxes]
assert b"mvex" in init, "init segment must describe fragments"
assert b"tfdt" in media, "fragment needs a decode time"
open(sys.argv[2], "wb").write(init)
open(sys.argv[3], "wb").write(media)
print("boxes:", " ".join(k for k, _ in boxes))
EOF

ls -la "$out"
