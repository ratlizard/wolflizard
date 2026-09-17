# Chicago Kare source record

The unmodified `ChicagoKare-Regular.ttf` and `LICENSE` come from
[KingDuane/Chicago-Kare](https://github.com/KingDuane/Chicago-Kare/tree/dca7a9e4f2b971e39cf3a5e8a3f9309d4293d56d),
commit `dca7a9e4f2b971e39cf3a5e8a3f9309d4293d56d`. Copyright (c) 2024 Duane King,
distributed under the [MIT License](LICENSE).

The font draws its printable-ASCII glyphs from squares on a grid of 16 per em,
so drawn unhinted at 16 pixels per em one square is one pixel. Systemless uses it
only for Chicago requests at 12 points (`bundled::pixel_strike`). It draws each
glyph with its leftmost ink at the pen, so the
[compatibility component](../compatibility/README.md) supplies the printable-ASCII
left bearings and advances and the face's ascent, descent, leading and maximum
width. Other characters it contains keep its own spacing; characters it lacks
fall back to URW Nimbus Sans Bold at 12 points.

SHA-256:

```
23d870a6138aedf9c8479a537f7da47881bf2d6fef51d275b4031c67513057af  ChicagoKare-Regular.ttf
2920e2c708b6f166eb065faecd1cb7494c1fc1df11cddc88ce727710326b2523  LICENSE
```
