//! Logical metrics from separately licensed classic-layout data components.

use super::{FONT_APPLICATION, FONT_CHARCOAL, FONT_CHICAGO, FONT_GENEVA, FONT_MONACO};

pub(super) const FIRST_ASCII_CODE: u8 = 0x20;
pub(super) const LAST_ASCII_CODE: u8 = 0x7E;

static GENEVA9_ADVANCES: &[u8; 95] = include_bytes!("geneva9-advances.bin");
static CHICAGO12_ADVANCES: &[u8; 95] = include_bytes!("chicago12-advances.bin");
// Each byte is a signed left bearing: the column of the glyph's leftmost ink
// relative to the pen.
static CHICAGO12_BEARINGS: &[u8; 95] = include_bytes!("chicago12-bearings.bin");
// Ascent, descent, leading and widMax of the same source's Chicago 12 face.
const CHICAGO12_FRAME: (i16, i16, i16) = (12, 3, 1);
const CHICAGO12_WID_MAX: i16 = 14;
// Charcoal 12's advances and left bearings, measured from Mac OS 8.5's screen
// (README.md), and its line: ascent, descent and leading, and widMax.
static CHARCOAL12_ADVANCES: &[u8; 95] = include_bytes!("charcoal12-advances.bin");
static CHARCOAL12_BEARINGS: &[u8; 95] = include_bytes!("charcoal12-bearings.bin");
const CHARCOAL12_FRAME: (i16, i16, i16) = (12, 3, 1);
const CHARCOAL12_WID_MAX: i16 = 12;
// Geneva 10's line, ascent, descent and leading, and its widMax, as Mac OS
// 8.5's strike gives them (System 7.5.2's has a widMax of 12). The grid
// recreation (../geneva10grid/README.md) has its advances and no leading.
const GENEVA10_FRAME: (i16, i16, i16) = (10, 2, 1);
const GENEVA10_WID_MAX: i16 = 11;
// Cythera's Argos A Nouveau (`sfnt` 7289 in Cythera Data), known by its head
// table's checkSumAdjustment, at 18 points: ascent 13, descent 5, leading 1.
// Cythera draws its window titles and its To Do and Journal plaques in it at
// 18 points and places them from the ascent. Its hhea ascent, 13.21, rounded
// up put every one a row below Mac OS 8.5's (25 October 2026). The descent
// and leading fill out TextEdit's line height of 19 on Mac OS 8.5. Its other
// sizes are unmeasured: TextEdit's baselines alone did not separate ascent
// from leading, and so do not settle them.
const ARGOS_CHECKSUM_ADJUSTMENT: u32 = 0x5bfb_fdbb;
const ARGOS_FRAMES: [(i16, (i16, i16, i16)); 1] = [(18, (13, 5, 1))];

/// The line a game's own outline font has on Mac OS 8.5 at a size, where it
/// has been measured.
pub(super) fn resource_outline_frame(bytes: &[u8], size: i16) -> Option<(i16, i16, i16)> {
    let tables = u16::from_be_bytes(bytes.get(4..6)?.try_into().ok()?) as usize;
    let head = (0..tables).find_map(|i| {
        let entry = bytes.get(12 + 16 * i..28 + 16 * i)?;
        (&entry[..4] == b"head").then(|| u32::from_be_bytes(entry[8..12].try_into().unwrap()) as usize)
    })?;
    let adjustment = u32::from_be_bytes(bytes.get(head + 8..head + 12)?.try_into().ok()?);
    if adjustment != ARGOS_CHECKSUM_ADJUSTMENT {
        return None;
    }
    ARGOS_FRAMES.iter().find(|(s, _)| *s == size).map(|(_, frame)| *frame)
}

// Inside Macintosh: Text (1993), p. 4-91 defines FOND.ffWidMax as the
// normalized maximum glyph width for a one-point font.
const MONACO_MAX_ADVANCE_UNITS: i32 = 1552;
const MONACO_UNITS_PER_EM: i32 = 2048;

/// The classic layout data a bundled face takes from this component; every
/// part is optional and absent for most faces.
#[derive(Clone, Copy, Default)]
pub(super) struct Layout {
    pub(super) advances: Option<&'static [u8; 95]>,
    /// Applied only to glyphs drawn from a pixel strike, whose shapes these
    /// bearings place; a substitute outline keeps its own.
    pub(super) bearings: Option<&'static [u8; 95]>,
    pub(super) wid_max: Option<i16>,
    /// Ascent, descent and leading, used as given.
    pub(super) frame: Option<(i16, i16, i16)>,
}

pub(super) fn bundled_layout(font_id: i16, size: i16) -> Layout {
    Layout {
        advances: bundled_advances(font_id, size),
        bearings: match (font_id, size) {
            (FONT_CHICAGO, 12) => Some(CHICAGO12_BEARINGS),
            (FONT_CHARCOAL, 12) => Some(CHARCOAL12_BEARINGS),
            _ => None,
        },
        wid_max: bundled_wid_max(font_id, size),
        frame: match (font_id, size) {
            (FONT_CHICAGO, 12) => Some(CHICAGO12_FRAME),
            (FONT_CHARCOAL, 12) => Some(CHARCOAL12_FRAME),
            (FONT_APPLICATION | FONT_GENEVA, 10) => Some(GENEVA10_FRAME),
            _ => None,
        },
    }
}

pub(super) fn bundled_advances(font_id: i16, size: i16) -> Option<&'static [u8; 95]> {
    match (font_id, size) {
        (FONT_APPLICATION | FONT_GENEVA, 9) => Some(GENEVA9_ADVANCES),
        (FONT_CHICAGO, 12) => Some(CHICAGO12_ADVANCES),
        (FONT_CHARCOAL, 12) => Some(CHARCOAL12_ADVANCES),
        _ => None,
    }
}

pub(super) fn bundled_wid_max(font_id: i16, size: i16) -> Option<i16> {
    if font_id == FONT_CHICAGO && size == 12 {
        return Some(CHICAGO12_WID_MAX);
    }
    if font_id == FONT_CHARCOAL && size == 12 {
        return Some(CHARCOAL12_WID_MAX);
    }
    if matches!(font_id, FONT_APPLICATION | FONT_GENEVA) && size == 10 {
        return Some(GENEVA10_WID_MAX);
    }
    // Monaco's scalable classic-family metadata records a 1552/2048-em
    // maximum advance. Match outline metric rounding at every requested size.
    (font_id == FONT_MONACO && size > 0).then(|| {
        ((MONACO_MAX_ADVANCE_UNITS * i32::from(size) + MONACO_UNITS_PER_EM / 2)
            / MONACO_UNITS_PER_EM) as i16
    })
}
