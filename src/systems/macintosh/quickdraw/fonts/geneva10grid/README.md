# Geneva 10 Grid

`Geneva10Grid.ttf` is a recreation of the classic Mac's Geneva 10 bitmap
strike, drawn by the maintainer of this fork on a grid of 10 squares per em
(1,000 units, a square every 100). Drawn unhinted at 10 pixels per em, one
square is one pixel. Systemless uses it for Application and Geneva requests at
10 points (`bundled::pixel_strike`), the size Cythera's message pane is set in,
with the line (ascent 10, descent 2, leading 1) and widMax (11) of Mac OS
8.5's strike from the compatibility component. Characters it does not contain
fall back to URW Nimbus Sans Regular at 10 points, as every other Geneva size
does.

Against Mac OS 8.5's Geneva 10 strike, all 95 printable ASCII glyphs have the
same pixels and the same advances. In Cythera's message pane the fork's
"Welcome to Delver version 1.0.3" and Mac OS 8.5's in SheepShaver have the
same 312 pixels. System 7.5.2's strike is older and differs in eight glyphs
(`*`, `Q`, `R`, `^`, `i`, `j`, `l`, `y`) and in its widMax (12).

**No licence has been given for this font yet.** It is included here by its
author's choice. Ask before reusing it elsewhere.

## SHA-256

```
492f074ba0bc415439dfe1c50b7447e61550e35a08adab9a7babfd21a8d8fadb  Geneva10Grid.ttf
```
