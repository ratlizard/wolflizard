//! Logical metrics from separately licensed classic-layout data components.

use super::{FONT_APPLICATION, FONT_CHICAGO, FONT_GENEVA, FONT_MONACO};

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
        bearings: (font_id == FONT_CHICAGO && size == 12).then_some(CHICAGO12_BEARINGS),
        wid_max: bundled_wid_max(font_id, size),
        frame: (font_id == FONT_CHICAGO && size == 12).then_some(CHICAGO12_FRAME),
    }
}

pub(super) fn bundled_advances(font_id: i16, size: i16) -> Option<&'static [u8; 95]> {
    match (font_id, size) {
        (FONT_APPLICATION | FONT_GENEVA, 9) => Some(GENEVA9_ADVANCES),
        (FONT_CHICAGO, 12) => Some(CHICAGO12_ADVANCES),
        _ => None,
    }
}

pub(super) fn bundled_wid_max(font_id: i16, size: i16) -> Option<i16> {
    if font_id == FONT_CHICAGO && size == 12 {
        return Some(CHICAGO12_WID_MAX);
    }
    // Monaco's scalable classic-family metadata records a 1552/2048-em
    // maximum advance. Match outline metric rounding at every requested size.
    (font_id == FONT_MONACO && size > 0).then(|| {
        ((MONACO_MAX_ADVANCE_UNITS * i32::from(size) + MONACO_UNITS_PER_EM / 2)
            / MONACO_UNITS_PER_EM) as i16
    })
}
