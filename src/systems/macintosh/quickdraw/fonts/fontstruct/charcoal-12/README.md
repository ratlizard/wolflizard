# Charcoal 12 FontStruction source record

`charcoal-12.otf`, `readme.txt` and `license.txt` are the unmodified contents of
the OpenType download of the FontStruction
[“Charcoal 12”](https://fontstruct.com/fontstructions/show/2813611) by Jeremy
Sachs, “a clone of Charcoal size 12, from Mac OS 8 and 9”. It is licensed under
the
[Creative Commons Attribution Share Alike 3.0 licence](http://creativecommons.org/licenses/by-sa/3.0/),
as `license.txt` states, and FontStruct's `readme.txt` asks that the font file
be redistributed with the other files of its archive, which is why both are
kept here.

The font is a bitmap strike built from squares on a grid of 16 per em (2000
units per em, 125 a square), so drawn unhinted at 16 pixels per em one square is
one pixel, as with the [Geneva 9 FontStruction](../README.md). It covers
printable ASCII and a few characters above it. Systemless uses it only for the
text of windows it draws in Mac OS 8's look (`loader/ppc/platinum.rs`), asking
for it as Charcoal's family ID, 2002, which it does not otherwise name to the
guest; characters it does not contain fall back to URW Nimbus Sans Bold at 12
points, as Chicago's do.

## SHA-256

```
d46c1111c5c6d54755f66f46020632aafed7dd478fedf945764da1ca8cd7c06b  charcoal-12.otf
efaedb59b42af45178e624924cec5d9c1221f06330193a5b612d5c077e516a51  license.txt
32518a9c17d591ab441427d47af10a524360d0d4287187d547782fd8c943420d  readme.txt
```
