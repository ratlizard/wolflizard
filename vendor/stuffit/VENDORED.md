# stuffit 0.3.0, vendored

The `stuffit` crate 0.3.0 from crates.io (Ben Letchford, MIT or Apache-2.0),
used through `[patch.crates-io]` in the top-level `Cargo.toml`, with one change:
the StuffIt 5 writer fills in the header fields StuffIt Expander checks.

The crate wrote the archive header CRC as a constant (`0x009b`, followed by
"Kestrel Sit5 Archive"), each entry's header CRC over its first 32 bytes only,
and no CRC in each entry's metadata block. Readers that skip these checks
(this crate's own parser, The Unarchiver) open such archives; StuffIt Expander
in Mac OS 9.2.2 refuses them with "The archive may be damaged. Error
#17537". Compared with archives StuffIt 5 and 6 wrote, each one CRC-16/ARC:

- the archive header's CRC (bytes 98-99) covers bytes 0-113 with the field
  zeroed, and bytes 100-113 are `0d a5 a5 "Reserved" a5 a5 00`;
- an entry's header CRC (header bytes 32-33) covers the whole header, name
  included, with the field zeroed;
- the second word of an entry's 36-byte metadata block is a CRC of that block,
  plus the 14-byte resource fork record when there is one, with the field
  zeroed.

StuffIt also writes 0x8000 at offset 28 of the metadata block, points a
folder's first child's `prev_off` at the folder, and puts in a folder's header
the unpacked size of every fork inside it (bytes 38-41) and the bytes its
contents take in the archive (42-45); the writer now does the same.
`test_sit5_header_crcs_match_stuffit` checks the layout. Archives written this
way were unpacked to identical forks by The Unarchiver; whether StuffIt
Expander opens them is being tried.

The archive in `tests/toolbox-showcase/` is written by its own packer with
stuffit 0.1.5 and is left as it is: its SHA-256 is recorded in the capture
manifests.
