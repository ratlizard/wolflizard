# Geneva 9 FontStruction source record

`geneva-9.ttf`, `readme.txt` and `license.txt` are the unmodified contents of
the TrueType download of the FontStruction
[“Geneva 9”](https://fontstruct.com/fontstructions/show/349277) by Kelsey
Higham. It is licensed under the
[Creative Commons Attribution 3.0 licence](http://creativecommons.org/licenses/by/3.0/),
as `license.txt` states, and FontStruct's `readme.txt` asks that the font file
be redistributed with the other files of its archive, which is why both are
kept here.

The font is a bitmap strike built from squares on a grid of 16 per em. Drawn
unhinted at 16 pixels per em, one square is one pixel, so its glyphs rasterize
to exactly the pixels it was drawn with. Systemless uses it only for
Application and Geneva requests at 9 points (`bundled::pixel_strike`), with the
printable-ASCII advances still taken from the
[compatibility table](../compatibility/README.md). Characters it does not
contain fall back to URW Nimbus Sans Regular at 9 points, as every other
Geneva size does.

## SHA-256

```
abda94180bfdf47d8601b0940707288ddef6a2ffe89a787fa5163d36bf928626  geneva-9.ttf
6c01c32393e2cabdbc4d1d9380a52bbbb33ebaab1984c98a506a2e4a3982f4cf  license.txt
51b14cc082fffd77bd0e508071c515ba259beaa5b41f00c3a7643eb55d61a205  readme.txt
```
