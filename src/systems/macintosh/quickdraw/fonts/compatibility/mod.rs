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
// table's checkSumAdjustment, at 18 points: ascent 13, descent 6, leading 0.
// Cythera draws its window titles and its To Do and Journal plaques in it at
// 18 points and places them from the ascent. Its hhea ascent, 13.21, rounded
// up put every one a row below Mac OS 8.5's (25 October 2026). The descent
// and leading fill out TextEdit's line height of 19 on Mac OS 8.5. The split
// between them is from the Character Archetype list (20 November 2026):
// TListBox::LDEFDraw puts the baseline at (top + bottom + ascent - descent)
// / 2, and Mac OS 8.5 draws the names a row above where a descent of 5 put
// them. That split is hhea's ascent and descent (3006 and 1282 of 4096 units)
// each rounded to the nearest pixel, and the same rule gives TextEdit's line
// heights on Mac OS 8.5 at every size measured: 10 + 4 = 14 at 14 points,
// 13 + 6 = 19 at 18, 16 + 7 = 23 at 22 and 18 + 8 = 26 at 24. Cythera's
// conversation window is Argos 22, and with the line rounded up (17 + 7 = 24)
// its topics and its reply field sat a row below 8.5's, the field a row taller
// (pair-talk-shots, 23 November 2026). So the frame is that rounding at every
// size.
const ARGOS_CHECKSUM_ADJUSTMENT: u32 = 0x5bfb_fdbb;

/// The line a game's own outline font has on Mac OS 8.5 at a size, where it
/// has been measured: Argos's hhea ascent, descent and line gap, each scaled
/// to `size` and rounded to the nearest pixel.
pub(super) fn resource_outline_frame(bytes: &[u8], size: i16) -> Option<(i16, i16, i16)> {
    let tables = u16::from_be_bytes(bytes.get(4..6)?.try_into().ok()?) as usize;
    let table = |tag: &[u8; 4]| {
        (0..tables).find_map(|i| {
            let entry = bytes.get(12 + 16 * i..28 + 16 * i)?;
            (&entry[..4] == tag).then(|| u32::from_be_bytes(entry[8..12].try_into().unwrap()) as usize)
        })
    };
    let head = table(b"head")?;
    let adjustment = u32::from_be_bytes(bytes.get(head + 8..head + 12)?.try_into().ok()?);
    if adjustment != ARGOS_CHECKSUM_ADJUSTMENT || size <= 0 {
        return None;
    }
    let units_per_em = i32::from(u16::from_be_bytes(bytes.get(head + 18..head + 20)?.try_into().ok()?));
    let hhea = table(b"hhea")?;
    let field = |at: usize| -> Option<i32> {
        Some(i32::from(i16::from_be_bytes(bytes.get(hhea + at..hhea + at + 2)?.try_into().ok()?)))
    };
    let (ascent, descent, gap) = (field(4)?, -field(6)?, field(8)?);
    if units_per_em <= 0 {
        return None;
    }
    let scaled = |units: i32| ((2 * units * i32::from(size) + units_per_em) / (2 * units_per_em)) as i16;
    Some((scaled(ascent), scaled(descent), scaled(gap)))
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

#[cfg(test)]
mod tests {
    use super::*;

    /// An sfnt directory with only the head and hhea fields the frame reads,
    /// set to Argos A Nouveau's: its checkSumAdjustment, 4096 units to the
    /// em, ascent 3006, descent -1282, no line gap.
    fn argos_like(adjustment: u32) -> Vec<u8> {
        let mut bytes = vec![0u8; 12 + 2 * 16 + 54 + 36];
        bytes[4..6].copy_from_slice(&2u16.to_be_bytes());
        let (head, hhea) = (44usize, 98usize);
        for (i, (tag, at)) in [(b"head", head), (b"hhea", hhea)].into_iter().enumerate() {
            let entry = 12 + 16 * i;
            bytes[entry..entry + 4].copy_from_slice(tag);
            bytes[entry + 8..entry + 12].copy_from_slice(&(at as u32).to_be_bytes());
        }
        bytes[head + 8..head + 12].copy_from_slice(&adjustment.to_be_bytes());
        bytes[head + 18..head + 20].copy_from_slice(&4096u16.to_be_bytes());
        bytes[hhea + 4..hhea + 6].copy_from_slice(&3006i16.to_be_bytes());
        bytes[hhea + 6..hhea + 8].copy_from_slice(&(-1282i16).to_be_bytes());
        bytes
    }

    #[test]
    fn argos_lines_are_mac_os_8_5s_textedit_line_heights() {
        // TextEdit's line heights for Argos on Mac OS 8.5: 14, 19, 23 and 26
        // at 14, 18, 22 and 24 points; 18 points split 13 over 6 (the
        // Character Archetype list's baselines).
        let argos = argos_like(ARGOS_CHECKSUM_ADJUSTMENT);
        assert_eq!(resource_outline_frame(&argos, 18), Some((13, 6, 0)));
        for (size, line) in [(14, 14), (18, 19), (22, 23), (24, 26)] {
            let (ascent, descent, leading) = resource_outline_frame(&argos, size).unwrap();
            assert_eq!(ascent + descent + leading, line, "{size} points");
        }
        assert_eq!(resource_outline_frame(&argos_like(0), 22), None, "another font keeps its own line");
    }
}
