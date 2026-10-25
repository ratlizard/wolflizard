# Smoothed Charcoal 12

`charcoal-12.kfov` is Mac OS 8.5's smoothed Charcoal 12, the system font of
Mac OS 8 and 9 as "Smooth all fonts on screen" draws it, captured from Mac OS
8.5 running in SheepShaver. The file is in the font override format
(`../override_format.rs`): the 95 printable ASCII glyphs, each pixel's gray
level from 0 to 15 stored as `level * 17`. The maintainer chose to include it
(25 October 2026). Like the [Charcoal 12 FontStruction](../fontstruct/charcoal-12/README.md),
it is a copy of what Apple's software drew.

How it was made, with the workbench's `work/fonts/capture/` scripts:

1. A SimpleText document of every printable character in Charcoal 12, each
   between `|` bars with a space either side, opened at start-up on a copy of
   Mac OS 8.5 at full color depth. At that depth a smoothed pixel's gray is
   the blend itself: the text came out in exactly 16 grays, 255 × (15 − level)
   / 15.
2. Each character's pixels read between its bars, placed by the font's device
   widths (its `hdmx` at 12 pixels per em) and Charcoal 12's left bearings as
   measured on Mac OS 8.5's screen (`../compatibility/charcoal12-bearings.bin`).
3. Every line redrawn from the extracted glyphs: 0 pixels differ from the
   capture.

Its widths are the bitmap Charcoal 12's
(`../compatibility/charcoal12-advances.bin`), all 95, so text measures as
before and only its pixels change.

Smoothed from Mac OS 8.5's own Charcoal outline by `outline::smoothed_mask`
(the ignored test `smoothed_charcoal12_against_mac_os_8_5`, with the font
supplied locally), 52 of the 95 glyphs come out as captured and 104 pixels of
the rest differ, nearly all by one level. That is how the smoothing of other
outline fonts was calibrated.

## SHA-256

```
031a8fdf81d3ffe095f1f83001d968963692df43e61d636e3bf625d119469a35  charcoal-12.kfov
```
