#!/usr/bin/env python3
"""Turns a recorded Twitch media playlist into a test fixture that is safe to commit.

Keeps what ad detection needs:
  - segment timing and titles
  - discontinuities
  - DATERANGE classes with ad timing and pod attributes

Removes:
  - URLs (segments, prefetch, tracking)
  - session IDs and other tracking fields

Usage: sanitize-playlist.py IN.m3u8 OUT.m3u8
"""
import re
import sys

# DATERANGE attributes worth keeping; everything else (URLs, session and tracking IDs) is dropped.
KEEP = {
    "ID", "CLASS", "START-DATE", "END-DATE", "DURATION", "PLANNED-DURATION", "END-ON-NEXT",
    "X-TV-TWITCH-STREAM-SOURCE", "X-TV-TWITCH-AD-ROLL-TYPE", "X-TV-TWITCH-AD-POD-LENGTH",
    "X-TV-TWITCH-AD-POD-POSITION", "X-TV-TWITCH-AD-POD-FILLED-DURATION", "X-TV-TWITCH-AD-AD-FORMAT",
}
UNQUOTED = {"DURATION", "PLANNED-DURATION", "END-ON-NEXT"}
ATTR = re.compile(r'([A-Z0-9-]+)=("[^"]*"|[^,]*)')


def daterange(value: str) -> str:
    attrs = [(k, v.strip('"')) for k, v in ATTR.findall(value) if k in KEEP]
    return ",".join(f"{k}={v}" if k in UNQUOTED else f'{k}="{v}"' for k, v in attrs)


def main(src: str, dst: str) -> None:
    lines = open(src, encoding="utf-8").read().splitlines()
    seq = next(int(l.split(":")[1]) for l in lines if l.startswith("#EXT-X-MEDIA-SEQUENCE:"))
    out, prefetch = [], 0
    for line in lines:
        if line.startswith("#EXT-X-DATERANGE:"):
            out.append("#EXT-X-DATERANGE:" + daterange(line.split(":", 1)[1]))
        elif line.startswith("#EXT-X-MAP:"):
            # fMP4 streams: the init segment URL is signed like the segments.
            out.append(f'#EXT-X-MAP:URI="https://cdn.example/init-{seq}.mp4"')
        elif line.startswith("#EXT-X-TWITCH-PREFETCH:"):
            out.append(f"#EXT-X-TWITCH-PREFETCH:https://cdn.example/prefetch-{prefetch}.ts")
            prefetch += 1
        elif line and not line.startswith("#"):
            out.append(f"https://cdn.example/seg-{seq}.ts")
            seq += 1
        else:
            out.append(line)
    open(dst, "w", encoding="utf-8", newline="\n").write("\n".join(out) + "\n")


if __name__ == "__main__":
    main(sys.argv[1], sys.argv[2])
