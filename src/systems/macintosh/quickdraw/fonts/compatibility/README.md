# Classic layout compatibility data

Systemless uses bundled URW and Coppet outlines for fallback font pixels. This directory
contains separately licensed logical metrics used where a classic-compatible
layout is required. It does not contain Apple font software or glyph artwork.

## Geneva 9 advances

`geneva9-advances.bin` contains 95 one-byte logical advances in Mac Roman ASCII
order, from `0x20` through `0x7E`. Its SHA-256 is
`44262c59655597d527318a7ba9b62350defba4f556cb97172d0fea05a6f0fd35`.

The values come exclusively from the `advance` fields in Systemless's
historical OFL-licensed Kurrajong 9 source:

- public commit: `0540ef827d800fbdb00eafc67922cef3245b81e5`
- path: `src/quickdraw/fonts/pixel_font/geneva9.rs`
- Git blob: `afc3cc9ec393dd1c6341c6375aedb95d3d106cd0`
- source SHA-256:
  `2578381b3fcf9f3bde359d1b258bcf0b95fcff8ea3181863d0c395c7563bfada`
- copyright: Copyright (c) 2026 Ben Letchford
- licence: SIL Open Font License 1.1
- Reserved Font Name: Systemless

PR #1524 later removed the old bitmap catalogue. The compatibility component
retains only the 95 advance values from that licensed Systemless source. It
excludes the historical bitmap pixels, masks, bearings, origins, heights, and
all extended characters. [Coppet](../coppet/README.md) supplies the 9-point ASCII
artwork; URW retains extended Mac Roman glyphs and the existing font metrics.

To verify the public source and component:

```sh
git show 0540ef827d800fbdb00eafc67922cef3245b81e5:src/quickdraw/fonts/pixel_font/geneva9.rs > /tmp/geneva9.rs
python3 - /tmp/geneva9.rs /tmp/geneva9-advances.bin <<'PY'
import pathlib
import re
import sys

source = pathlib.Path(sys.argv[1]).read_text()
advances = [int(value) for value in re.findall(r"g!\(\s*(\d+)", source)]
assert len(advances) == 95
pathlib.Path(sys.argv[2]).write_bytes(bytes(advances))
PY
shasum -a 256 /tmp/geneva9.rs /tmp/geneva9-advances.bin
cmp /tmp/geneva9-advances.bin src/quickdraw/fonts/compatibility/geneva9-advances.bin
```

The source file should hash to `2578381b...` and the component to
`44262c59...`. Extracting the first numeric argument from each of its 95 `g!`
records, in source order, reproduces `geneva9-advances.bin` exactly.

See [OFL.txt](OFL.txt) for the component's copyright and licence notice.

## Chicago 12 advances, bearings and frame

`chicago12-advances.bin` contains 95 one-byte logical advances and
`chicago12-bearings.bin` 95 signed one-byte left bearings, each in Mac Roman
ASCII order from `0x20` through `0x7E`. Their SHA-256 hashes are
`f6d4c00199ea5d1751dbd60a44e26d823250074e17b1fef145d172e8496d3bee` and
`5d0b6e35fea233863184d9e51722f2772103e7d22b6b2776855316d062c17007`. The face's
ascent 12, descent 3, leading 1 and maximum width 14 are constants in
[`mod.rs`](mod.rs).

All come exclusively from Systemless's historical OFL-licensed Jarrah 12
source, whose header records its advances, origins and side bearings as
conformed to the classic Chicago 12 strike:

- public commit: `0540ef827d800fbdb00eafc67922cef3245b81e5`
- path: `src/quickdraw/fonts/pixel_font/chicago12.rs`
- Git blob: `37ae148146affd46ddeb03c7838a81f524c72348`
- source SHA-256:
  `267441f4309c7b157e4e273e376d56190340930b984cd1a3f8fd37e1f3c340bc`
- copyright: Copyright (c) 2026 Ben Letchford
- licence: SIL Open Font License 1.1
- Reserved Font Name: Systemless

The advances are the first argument of each of the 95 `g!` records, the
bearings the first element of the origin pair that follows it, and the frame
the source's `CHICAGO_12_METRICS` constant. The component excludes the bitmap
pixels, vertical origins, heights and all extended characters; Chicago Kare
supplies the pixels.

To verify the public source and components:

```sh
git show 0540ef827d800fbdb00eafc67922cef3245b81e5:src/quickdraw/fonts/pixel_font/chicago12.rs > /tmp/chicago12.rs
python3 - /tmp/chicago12.rs <<'PY'
import pathlib
import re
import sys

source = pathlib.Path(sys.argv[1]).read_text()
records = re.findall(r"g!\(\s*(\d+)\s*,\s*\(\s*(-?\d+)\s*,", source)
assert len(records) == 95
pathlib.Path("/tmp/chicago12-advances.bin").write_bytes(bytes(int(a) for a, _ in records))
pathlib.Path("/tmp/chicago12-bearings.bin").write_bytes(bytes(int(b) & 0xFF for _, b in records))
PY
shasum -a 256 /tmp/chicago12.rs
cmp /tmp/chicago12-advances.bin src/quickdraw/fonts/compatibility/chicago12-advances.bin
cmp /tmp/chicago12-bearings.bin src/quickdraw/fonts/compatibility/chicago12-bearings.bin
```

## Monaco maximum advance

The bundled Monaco substitute retains its own outline pixels and per-glyph
advances. Its `widMax` compatibility metric is derived at each requested size
from the classic scalable Monaco family's normalized maximum advance of
1552/2048 em. This is metadata only; no original font software or glyph artwork
is included.
