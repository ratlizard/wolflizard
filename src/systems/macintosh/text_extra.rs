//! Character and space extra across a run of text, as QuickDraw applies them.
//!
//! `CharExtra` widens (or, negative, narrows) every character but the space
//! by a Fixed amount, and `SpaceExtra` every space (Inside Macintosh Volume
//! V, V-77; Volume I, I-171). The pen keeps the fraction: a Color QuickDraw
//! port carries it as `pnLocHFrac`, which a new pen position sets to one
//! half, and each character is drawn at the pen's whole pixel. So an extra
//! of -3.33 a character narrows fifteen characters by about 47 pixels, not
//! by 45 or 60, and `TextWidth` answers with the width the text is drawn at,
//! extras included; an application that narrows a string to fit and then
//! centres it by `TextWidth` relies on both.
//!
//! Until 28 September 2026 both of the fork's slices kept only the extra's
//! whole pixels, rounded down, and measured without it. Cythera fits a name
//! under a conversation portrait to 84 pixels with a negative `CharExtra`
//! and centres it by `TextWidth`, so a fifteen-letter name was measured at
//! its full width, centred 35 pixels left of where Mac OS 8.5 puts it, and
//! drawn narrower than 8.5 draws it.

/// Where the pen starts a run, as a fraction of a pixel: one half.
pub(crate) const RUN_START_FRACTION: i32 = 0x8000;

/// The pen after one character: `pen` in 16.16 fixed point, relative to the
/// run's start, advanced by the character's own advance in whole pixels and
/// by the extra that applies to it.
pub(crate) fn advance_pen(pen: i32, advance_px: i32, is_space: bool, char_extra: i32, space_extra: i32) -> i32 {
    let extra = if is_space { space_extra } else { char_extra };
    pen.saturating_add(advance_px.saturating_mul(1 << 16)).saturating_add(extra)
}

/// The whole pixel a pen in 16.16 fixed point stands on, rounded down.
pub(crate) fn pen_pixel(pen: i32) -> i32 {
    pen >> 16
}

/// A run's width as the measuring routines answer it: the base advances'
/// total with each extra added, from a pen that starts at one half, so it is
/// where drawing the same run leaves the pen.
pub(crate) fn run_width(base_px: i32, non_spaces: i32, spaces: i32, char_extra: i32, space_extra: i32) -> i32 {
    let pen = RUN_START_FRACTION
        .saturating_add(base_px.saturating_mul(1 << 16))
        .saturating_add(char_extra.saturating_mul(non_spaces))
        .saturating_add(space_extra.saturating_mul(spaces));
    pen_pixel(pen)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_extra_leaves_advances_whole() {
        let mut pen = RUN_START_FRACTION;
        let mut starts = Vec::new();
        for advance in [7, 9, 4] {
            starts.push(pen_pixel(pen));
            pen = advance_pen(pen, advance, false, 0, 0);
        }
        assert_eq!(starts, vec![0, 7, 16]);
        assert_eq!(pen_pixel(pen), 20);
        assert_eq!(run_width(20, 3, 0, 0, 0), 20);
    }

    #[test]
    fn a_fractional_narrowing_carries_its_fraction() {
        // Cythera's FitText: (84 - 134) / 15 as a Fixed, about -3.33 pixels.
        let extra = ((84 - 134) << 16) / 15;
        let mut pen = RUN_START_FRACTION;
        for i in 0..15 {
            // Fifteen characters, 134 pixels: a space of 3 at 5, the rest 131.
            let advance = match i { 5 => 3, 0 => 14, _ => 9 };
            pen = advance_pen(pen, advance, i == 5, extra, 0);
        }
        // 0.5 + 134 - 14 * 3.33 is 87.8: the pen stands on 87.
        assert_eq!(pen_pixel(pen), 87);
        assert_eq!(run_width(134, 14, 1, extra, 0), 87);
        // Whole pixels rounded down, as before, gave 134 - 14 * 4 = 78.
        assert_eq!((extra >> 16), -4);
        assert_ne!(pen_pixel(pen), 134 - 14 * 4);
    }

    #[test]
    fn space_extra_applies_to_spaces_only() {
        let space = 2 << 16;
        assert_eq!(run_width(10, 2, 1, 0, space), 12);
        assert_eq!(run_width(10, 2, 0, 0, space), 10);
    }
}
