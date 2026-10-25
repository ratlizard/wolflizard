//! Mac OS 8.5's font smoothing: "Smooth all fonts on screen" in the
//! Appearance control panel.
//!
//! Technical Note TN1149, "Smoothing Fonts", describes it. Mac OS 8.5's
//! TrueType scaler grid-fits the outline for the requested size, scan-converts
//! it at four times the resolution, and maps the count of covered sub-pixels in
//! each pixel's 4x4 square to a gray level from 0 to 15. QuickDraw Text's
//! blitter blends that level from the foreground color into the pixel already
//! there. Smoothing applies from a minimum size the user sets, never below 12,
//! and is on as Mac OS 8.5 ships.
//!
//! Only the PowerPC slice's text drawers ask for smoothed glyphs
//! (`get_smoothed_glyph`), since the PowerPC slice stands for Mac OS 8.5 and
//! the 68K slice for Mac OS 7.6 and 8.1, which do not smooth. A smoothed face
//! has the widths of the sharp one, so measuring text is unchanged.
//! `SYSTEMLESS_TEXT_SMOOTHING=0` turns it off, and a number sets the minimum
//! size.
//!
//! A face here stores each pixel's level as `level * 17`, so 255 is full ink
//! and code that tests a pixel against 128 still draws a sharp version. The
//! PowerPC text drawers blend the levels in between (`ppc_blend_text_pixel`).
//!
//! The faces:
//! - Charcoal 12, the system font of Mac OS 8 and 9, is Mac OS 8.5's own
//!   smoothed rendering (`smoothed/README.md`): the fork has no Charcoal
//!   outline to smooth.
//! - A game's own TrueType fonts, and the bundled substitute outlines, are
//!   smoothed from their outlines (`outline::rasterize_smoothed`).
use std::collections::HashMap;
use std::sync::atomic::{AtomicI16, Ordering};
use std::sync::{LazyLock, Mutex};

use super::{override_format, FontFace, Glyph, FONT_CHARCOAL};

/// The minimum smoothed size, 0 when smoothing is off, and -1 before the
/// environment has been read.
static MIN_SIZE: AtomicI16 = AtomicI16::new(-1);

/// Turn smoothing on from `min_size` points (raised to 12, as Mac OS 8.5's
/// SetAntiAliasedTextEnabled does), or off with None.
pub fn set_text_smoothing(min_size: Option<i16>) {
    MIN_SIZE.store(min_size.map_or(0, |size| size.max(12)), Ordering::Release);
}

/// The minimum smoothed size, or None when smoothing is off.
pub fn text_smoothing() -> Option<i16> {
    let mut min = MIN_SIZE.load(Ordering::Acquire);
    if min < 0 {
        min = match std::env::var("SYSTEMLESS_TEXT_SMOOTHING").ok().as_deref() {
            None => 12,
            Some(value) => value.trim().parse::<i16>().map_or(12, |size| if size <= 0 { 0 } else { size.max(12) }),
        };
        MIN_SIZE.store(min, Ordering::Release);
    }
    (min > 0).then_some(min)
}

fn leak_blob(bytes: &[u8]) -> Option<&'static FontFace> {
    let blob = override_format::read_blob(bytes).ok()?;
    Some(Box::leak(Box::new(FontFace {
        font_id: blob.font_id,
        size: blob.size,
        metrics: blob.metrics,
        glyphs: Box::leak(blob.glyphs.into_boxed_slice()),
        data: Box::leak(blob.data.into_boxed_slice()),
    })))
}

static CHARCOAL_12: LazyLock<Option<&'static FontFace>> =
    LazyLock::new(|| leak_blob(include_bytes!("smoothed/charcoal-12.kfov")));

/// Outline faces smoothed so far, by font and size: a game's own TrueType
/// fonts, then the bundled substitute outlines.
static OUTLINE_FACES: LazyLock<Mutex<HashMap<(i16, i16), Option<&'static FontFace>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn face(font_id: i16, size: i16) -> Option<&'static FontFace> {
    if (font_id, size) == (FONT_CHARCOAL, 12) {
        return *CHARCOAL_12;
    }
    if let Some(face) = OUTLINE_FACES.lock().ok()?.get(&(font_id, size)) {
        return *face;
    }
    // A game's own sfnt first, as the sharp lookup has it (mod.rs,
    // get_font_face). A bundled substitute outline is smoothed too, since
    // the fonts it stands in for are TrueType on Mac OS 8.5, but not at a
    // size a pixel recreation draws (Chicago 12): that stays as drawn.
    let resource = super::resources::RESOURCE_OUTLINE_FONTS
        .lock()
        .ok()?
        .get(&font_id)
        .copied();
    let bytes = match resource {
        Some(bytes) => Some(bytes),
        None if super::bundled::pixel_strike(font_id, size).is_none() => super::bundled::bytes(font_id),
        None => None,
    };
    let face = bytes
        .filter(|_| (1..=96).contains(&size))
        .and_then(|bytes| super::outline::rasterize_smoothed(font_id, size, bytes))
        .map(|(face, _)| face);
    OUTLINE_FACES.lock().ok()?.insert((font_id, size), face);
    face
}

/// The smoothed glyph for a printable ASCII character, when smoothing is on,
/// reaches `size`, and there is a smoothed face for the font. Other
/// characters, and every other case, are the caller's to look up as before.
pub fn get_smoothed_glyph(font_id: i16, size: i16, ch: char) -> Option<(&'static Glyph, &'static [u8])> {
    if !(' '..='~').contains(&ch) || !text_smoothing().is_some_and(|min| size >= min) {
        return None;
    }
    let face = face(font_id, size)?;
    let glyph = face.glyphs.get(ch as usize - 32)?;
    (glyph.width != 0 || glyph.height != 0 || glyph.advance != 0).then_some((glyph, face.data))
}

/// Whether a glyph has pixels between no ink and full ink, which only a
/// smoothed face has.
pub fn glyph_has_partial_coverage(glyph: &Glyph, data: &[u8]) -> bool {
    let count = usize::from(glyph.width) * usize::from(glyph.height);
    data.get(glyph.data_offset..glyph.data_offset + count)
        .is_some_and(|pixels| pixels.iter().any(|&p| p != 0 && p != 255))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_smoothed_charcoal12_is_mac_os_8_5s() {
        let face = CHARCOAL_12.expect("the smoothed Charcoal 12");
        assert_eq!((face.font_id, face.size), (FONT_CHARCOAL, 12));
        // Mac OS 8.5's widths are the bitmap Charcoal 12's
        // (compatibility/charcoal12-advances.bin), and its levels run 0 to 15.
        let advances: Vec<u8> = face.glyphs.iter().take(95).map(|g| g.advance).collect();
        assert_eq!(advances.as_slice(), include_bytes!("compatibility/charcoal12-advances.bin"));
        let a = &face.glyphs[usize::from(b'a' - b' ')];
        assert!(glyph_has_partial_coverage(a, face.data));
        assert!(face.data.iter().all(|&p| p % 17 == 0));
    }
}
