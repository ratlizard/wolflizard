//! Mac OS 8's look for the windows an application builds through the
//! Appearance Manager.
//!
//! A window made with one of the Appearance window definitions (WDEF 64 to
//! 66, definition IDs 1024 to 1071) belongs to an application that asked for
//! the Appearance Manager, and on Mac OS 8 the system draws such a window and
//! every control in it in its own look, the classic push buttons and check
//! boxes included. This host draws everything else as System 7 did; these
//! windows it draws as Mac OS 8.5 does.
//!
//! Every shape and grey here was measured from Mac OS 8.5 (Infinite Mac,
//! Power Macintosh 9500) showing Cythera's Preferences window, which is the
//! one window in that game built this way: a modal dialog (1042) holding
//! group boxes, sliders, check boxes, push buttons, static text and icons.
//! The colours are the ones that screen showed in 256 colours with Cythera's
//! colour table. Mac OS 8.5 asked for its own greys and Color2Index gave it
//! the nearest entries of that table, which the monitor's gamma then showed
//! as these; so on an eight-bit screen each is drawn with the entry this
//! host shows as that colour, which is the entry Mac OS 8.5 used, and
//! elsewhere with the colour itself. What that window never showed -- a
//! pressed button, a disabled check box, a vertical slider -- is drawn as
//! the nearest state it did show, not guessed at.

use super::*;

/// kWindowModalDialogProc: the one Appearance window whose frame was
/// measured.
pub(super) const PPC_PLATINUM_MODAL_DIALOG_PROC: i16 = 1042;

const BACKGROUND: u8 = 227;
const WHITE: u8 = 255;
const BLACK: u8 = 0;

/// A colour as Mac OS 8.5 showed it on the screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct Shade(u8, u8, u8);

fn grey(value: u8) -> Shade {
    Shade(value, value, value)
}

fn rgb(r: u8, g: u8, b: u8) -> Shade {
    Shade(r, g, b)
}

/// What to draw with to show `shade` in `port`: in an indexed port whose
/// colour table has an entry the display's gamma shows as `shade`, that
/// entry's colour and index; otherwise the colour itself.
fn resolve(
    memory: &mut PpcSectionMem,
    gworlds: &[PpcGWorldRecord],
    port: u32,
    shade: Shade,
) -> (PpcRgbColor, Option<u8>) {
    let exact = PpcRgbColor {
        red: u16::from(shade.0) * 0x0101,
        green: u16::from(shade.1) * 0x0101,
        blue: u16::from(shade.2) * 0x0101,
    };
    let Some(surface) = ppc_live_quickdraw_surface(memory, gworlds, port) else {
        return (exact, None);
    };
    if surface.front_buffer.depth != 8 {
        return (exact, None);
    }
    let fallback = TrapDispatcher::standard_mac_8bpp_clut();
    let clut = surface
        .ctable_handle
        .and_then(|handle| ppc_read_ctable_clut(memory, handle, &fallback))
        .unwrap_or(fallback);
    let gamma = crate::display::default_display_gamma();
    clut.iter()
        .position(|[r, g, b]| {
            gamma[0][usize::from(r >> 8)] == shade.0
                && gamma[1][usize::from(g >> 8)] == shade.1
                && gamma[2][usize::from(b >> 8)] == shade.2
        })
        .map_or((exact, None), |index| {
            let [red, green, blue] = clut[index];
            (PpcRgbColor { red, green, blue }, Some(index as u8))
        })
}

/// The dialog background the Appearance Manager's theme gives a window
/// (kThemeBrushDialogBackgroundActive), as SetThemeWindowBackground sets it:
/// the colour that shows as Mac OS 8.5's dialog grey in that window.
pub(super) fn ppc_platinum_dialog_background(
    memory: &mut PpcSectionMem,
    gworlds: &[PpcGWorldRecord],
    window: u32,
) -> PpcRgbColor {
    resolve(memory, gworlds, window, grey(BACKGROUND)).0
}

/// The definition ID a window was made with, before this host maps an
/// Appearance window onto the classic frame it stands for.
pub(super) fn ppc_window_definition_id(memory: &mut PpcSectionMem, window: u32) -> i16 {
    memory
        .read_u32_be(window.wrapping_add(PPC_CWINDOW_DEF_PROC_OFFSET))
        .filter(|handle| *handle != 0)
        .and_then(|handle| memory.read_u32_be(handle))
        .filter(|data| *data != 0)
        .and_then(|data| memory.read_u16_be(data))
        .unwrap_or(0) as i16
}

/// Whether a window was made with an Appearance window definition and is
/// not drawn by the application's own WDEF.
pub(super) fn ppc_window_is_platinum(memory: &mut PpcSectionMem, window: u32) -> bool {
    window != 0
        && super::dispatch_defproc::ppc_app_wdef_window_proc_id(window).is_none()
        && (64..=66).contains(&(ppc_window_definition_id(memory, window) >> 4))
}

/// Whether a window is an Appearance modal dialog, whose frame is drawn
/// here rather than as the classic dBoxProc it otherwise stands for.
pub(super) fn ppc_window_is_platinum_modal(memory: &mut PpcSectionMem, window: u32) -> bool {
    ppc_window_is_platinum(memory, window)
        && ppc_window_definition_id(memory, window) == PPC_PLATINUM_MODAL_DIALOG_PROC
}

/// The modal dialog's structure around its content: a black line three
/// pixels out, and a one-pixel shadow beyond it on the right and below.
pub(super) fn ppc_platinum_modal_structure_bounds(
    (top, left, bottom, right): (i16, i16, i16, i16),
) -> (i16, i16, i16, i16) {
    (
        top.saturating_sub(3),
        left.saturating_sub(3),
        bottom.saturating_add(4),
        right.saturating_add(4),
    )
}

/// Paints single pixels and lines in one port, with inclusive ends.
struct Pen<'a> {
    memory: &'a mut PpcSectionMem,
    gworlds: &'a [PpcGWorldRecord],
    port: u32,
    wrote: bool,
    resolved: std::collections::HashMap<Shade, (PpcRgbColor, Option<u8>)>,
}

impl<'a> Pen<'a> {
    fn new(memory: &'a mut PpcSectionMem, gworlds: &'a [PpcGWorldRecord], port: u32) -> Self {
        Self {
            memory,
            gworlds,
            port,
            wrote: false,
            resolved: std::collections::HashMap::new(),
        }
    }

    fn rect(&mut self, (top, left, bottom, right): (i16, i16, i16, i16), shade: Shade) {
        if bottom > top && right > left {
            let (color, index) = match self.resolved.get(&shade) {
                Some(found) => *found,
                None => {
                    let found = resolve(self.memory, self.gworlds, self.port, shade);
                    self.resolved.insert(shade, found);
                    found
                }
            };
            self.wrote |= ppc_paint_rect_bounds(
                self.memory,
                self.gworlds,
                self.port,
                (top, left, bottom, right),
                color,
                index,
            );
        }
    }

    fn dot(&mut self, h: i16, v: i16, shade: Shade) {
        self.rect((v, h, v + 1, h + 1), shade);
    }

    /// A horizontal run from `h0` to `h1`, both included.
    fn hline(&mut self, v: i16, h0: i16, h1: i16, shade: Shade) {
        self.rect((v, h0, v + 1, h1 + 1), shade);
    }

    /// A vertical run from `v0` to `v1`, both included.
    fn vline(&mut self, h: i16, v0: i16, v1: i16, shade: Shade) {
        self.rect((v0, h, v1 + 1, h + 1), shade);
    }

    fn text(&mut self, pen: (i16, i16), bytes: &[u8]) {
        let _ = ppc_draw_text_bytes(
            self.memory,
            self.gworlds,
            self.port,
            pen,
            crate::quickdraw::fonts::FONT_CHARCOAL,
            12,
            PPC_QD_TEXT_MODE_SRC_OR,
            PPC_RGB_BLACK,
            None,
            bytes,
        );
        self.wrote = true;
    }
}

/// The modal dialog's frame and background, drawn on the screen around the
/// window's content (global coordinates). Returns false when the window has
/// no content region to draw around.
pub(super) fn ppc_draw_platinum_dialog_frame(
    memory: &mut PpcSectionMem,
    gworlds: &[PpcGWorldRecord],
    window: u32,
    height: i16,
    width: i16,
) -> bool {
    Pen::new(memory, gworlds, window).rect((0, 0, height, width), grey(BACKGROUND));
    let Some((t, l, b, r)) = memory
        .read_u32_be(window + PPC_CWINDOW_CONTENT_RGN_OFFSET)
        .and_then(|region| ppc_read_rgn_bbox(memory, region))
    else {
        return false;
    };
    let mut pen = Pen::new(memory, gworlds, PPC_MAIN_GWORLD);
    // A black line three pixels out, and a shadow one pixel beyond it on
    // the right and below, starting two pixels in from the corner.
    pen.hline(t - 3, l - 3, r + 2, grey(BLACK));
    pen.hline(b + 2, l - 3, r + 2, grey(BLACK));
    pen.vline(l - 3, t - 3, b + 2, grey(BLACK));
    pen.vline(r + 2, t - 3, b + 2, grey(BLACK));
    pen.vline(r + 3, t - 1, b + 3, grey(BLACK));
    pen.hline(b + 3, l - 1, r + 3, grey(BLACK));
    // Inside it two bevels, light above and to the left, dark below and to
    // the right; the corners where light meets dark take the outer bevel's
    // light grey.
    pen.hline(t - 2, l - 2, r + 1, grey(203));
    pen.vline(l - 2, t - 2, b + 1, grey(203));
    pen.hline(b + 1, l - 1, r + 1, grey(118));
    pen.vline(r + 1, t - 1, b + 1, grey(118));
    pen.hline(t - 1, l - 1, r - 1, grey(WHITE));
    pen.vline(l - 1, t - 1, b - 1, grey(WHITE));
    pen.hline(b, l, r, grey(178));
    pen.vline(r, t, b, grey(178));
    pen.dot(r, t - 1, grey(203));
    pen.dot(l - 1, b, grey(203));
    pen.wrote
}

/// Where a Platinum slider's thumb runs: its left edge from five pixels in
/// to eighteen short of the right, thirteen wide. Only horizontal sliders
/// were measured; a vertical one is laid out the same way down its length.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct PpcPlatinumSliderTrack {
    pub(super) start: i16,
    pub(super) travel: i16,
}

pub(super) const PPC_PLATINUM_THUMB_WIDTH: i16 = 13;
const THUMB_HEIGHT: i16 = 16;

pub(super) fn ppc_platinum_slider_track(start: i16, end: i16) -> PpcPlatinumSliderTrack {
    PpcPlatinumSliderTrack {
        start: start + 5,
        travel: (end - start - 23).max(0),
    }
}

/// The thumb's offset along its travel for `value`, rounded to the nearest
/// pixel as Mac OS 8.5 places it.
pub(super) fn ppc_platinum_thumb_offset(travel: i16, value: i16, min: i16, max: i16) -> i16 {
    let range = i32::from(max) - i32::from(min);
    if range <= 0 {
        return 0;
    }
    let relative = i32::from(value.clamp(min, max)) - i32::from(min);
    ((relative * i32::from(travel) * 2 + range) / (range * 2)) as i16
}

/// The value for the pointer at `coordinate` along a Platinum slider: the
/// thumb centred on the pointer, pinned to its travel.
pub(super) fn ppc_platinum_slider_value_at(
    start: i16,
    end: i16,
    coordinate: i16,
    min: i16,
    max: i16,
) -> i16 {
    let track = ppc_platinum_slider_track(start, end);
    let range = i32::from(max) - i32::from(min);
    let travel = i32::from(track.travel);
    if travel == 0 || range <= 0 {
        return min;
    }
    let position =
        (i32::from(coordinate) - i32::from(track.start) - i32::from(PPC_PLATINUM_THUMB_WIDTH / 2))
            .clamp(0, travel);
    (i32::from(min) + (position * range + travel / 2) / travel) as i16
}

/// The check box's tick, relative to the box's top-left corner: black
/// strokes with grey edges, reaching two pixels past the box's right side.
const TICK: &[(i16, i16, u8)] = &[
    (12, 1, 0),
    (10, 2, 0),
    (12, 2, 148),
    (13, 2, 60),
    (9, 3, 0),
    (10, 3, 0),
    (12, 3, 60),
    (8, 4, 0),
    (9, 4, 0),
    (10, 4, 118),
    (2, 5, 0),
    (3, 5, 0),
    (7, 5, 0),
    (8, 5, 0),
    (9, 5, 148),
    (10, 5, 148),
    (3, 6, 0),
    (4, 6, 0),
    (6, 6, 0),
    (7, 6, 0),
    (8, 6, 148),
    (9, 6, 60),
    (3, 7, 60),
    (4, 7, 0),
    (5, 7, 0),
    (6, 7, 0),
    (7, 7, 148),
    (8, 7, 60),
    (4, 8, 60),
    (5, 8, 0),
    (6, 8, 148),
    (7, 8, 60),
    (5, 9, 148),
    (6, 9, 60),
];

/// A control in a Platinum window, drawn as Mac OS 8.5 draws it, in the
/// owner's port (local coordinates). None for the kinds this does not draw,
/// which keep the host's classic drawing.
#[allow(clippy::too_many_arguments)]
pub(super) fn ppc_draw_platinum_control(
    memory: &mut PpcSectionMem,
    gworlds: &[PpcGWorldRecord],
    owner: u32,
    handle: u32,
    control: u32,
    proc_id: i16,
    rect: (i16, i16, i16, i16),
) -> Option<bool> {
    let title = ppc_read_pstring_bytes(memory, control + PPC_CONTROL_TITLE_OFFSET)
        .unwrap_or_default()
        .into_iter()
        .flat_map(|byte| {
            if byte == 0xc9 {
                vec![b'.', b'.', b'.']
            } else {
                vec![byte]
            }
        })
        .collect::<Vec<_>>();
    let value = memory
        .read_u16_be(control + PPC_CONTROL_VALUE_OFFSET)
        .unwrap_or(0) as i16;
    let min = memory
        .read_u16_be(control + PPC_CONTROL_MIN_OFFSET)
        .unwrap_or(0) as i16;
    let max = memory
        .read_u16_be(control + PPC_CONTROL_MAX_OFFSET)
        .unwrap_or(0) as i16;
    let default = super::appearance_controls::ppc_control_is_default(handle);
    let mut pen = Pen::new(memory, gworlds, owner);
    match proc_id {
        0 => draw_push_button(&mut pen, rect, &title, default),
        1 => draw_check_box(&mut pen, rect, &title, value != 0),
        160 | 161 => draw_group_box(&mut pen, rect, &title),
        p if super::appearance_controls::ppc_is_slider_proc_id(p) => {
            draw_slider(&mut pen, rect, value, min, max)
        }
        _ => return None,
    }
    let _ = pen.wrote;
    Some(true)
}

/// A title's advance in Charcoal 12, and the ascent and descent the
/// measured layouts were read against: the system font's at 12 points, whose
/// baseline sits twelve pixels below a group box's top and whose line centres
/// on a button's height.
fn title_metrics(title: &[u8]) -> (i16, i16, i16) {
    let advance =
        ppc_text_bytes_advance_for_font(title, crate::quickdraw::fonts::FONT_CHARCOAL, 12);
    (advance, 12, 3)
}

/// The baseline that centres a line of the system font in `top..bottom`.
fn centred_baseline(top: i16, bottom: i16, ascent: i16, descent: i16) -> i16 {
    (top + bottom) / 2 + (ascent - descent) / 2
}

/// A push button: a black outline with its corners cut by a pixel of dark
/// grey each way, a light bevel inside it above and to the left and a dark
/// one below and to the right, on the dialog background.
fn draw_push_button(
    pen: &mut Pen<'_>,
    (t, l, b, r): (i16, i16, i16, i16),
    title: &[u8],
    default: bool,
) {
    if b - t < 8 || r - l < 8 {
        return;
    }
    if default {
        draw_default_ring(pen, (t, l, b, r));
    }
    let (g60, g148, g191, g203) = (grey(60), grey(148), grey(191), grey(203));
    pen.rect((t + 3, l + 1, b - 3, r - 1), grey(BACKGROUND));
    // The outline.
    pen.hline(t, l + 3, r - 4, grey(BLACK));
    pen.hline(b - 1, l + 3, r - 4, grey(BLACK));
    pen.vline(l, t + 3, b - 4, grey(BLACK));
    pen.vline(r - 1, t + 3, b - 4, grey(BLACK));
    pen.dot(l + 1, t + 1, grey(BLACK));
    pen.dot(r - 2, t + 1, grey(BLACK));
    pen.dot(l + 1, b - 2, grey(BLACK));
    pen.dot(r - 2, b - 2, grey(BLACK));
    for (h, v) in [
        (l + 2, t),
        (l, t + 2),
        (r - 3, t),
        (r - 1, t + 2),
        (l, b - 3),
        (l + 2, b - 1),
        (r - 1, b - 3),
        (r - 3, b - 1),
    ] {
        pen.dot(h, v, g60);
    }
    // The bevels.
    pen.hline(t + 1, l + 3, r - 4, grey(BACKGROUND));
    pen.dot(l + 2, t + 1, g203);
    pen.dot(r - 3, t + 1, g203);
    pen.dot(l + 1, t + 2, g203);
    pen.dot(r - 2, t + 2, g203);
    pen.hline(t + 2, l + 2, r - 4, grey(WHITE));
    pen.dot(r - 3, t + 2, grey(BACKGROUND));
    pen.vline(l + 2, t + 3, b - 4, grey(WHITE));
    pen.dot(l + 3, t + 3, grey(WHITE));
    pen.vline(l + 1, t + 3, b - 4, grey(BACKGROUND));
    pen.vline(r - 3, t + 3, b - 4, g191);
    pen.dot(r - 4, b - 4, g191);
    pen.vline(r - 2, t + 3, b - 4, g148);
    pen.dot(l + 1, b - 3, g203);
    pen.dot(l + 2, b - 3, grey(BACKGROUND));
    pen.hline(b - 3, l + 3, r - 4, g191);
    pen.hline(b - 3, r - 3, r - 2, g148);
    pen.dot(l + 2, b - 2, g203);
    pen.hline(b - 2, l + 3, r - 3, g148);
    if !title.is_empty() {
        let (advance, ascent, descent) = title_metrics(title);
        let h = l + (r - l - advance) / 2;
        pen.text((h, centred_baseline(t, b, ascent, descent)), title);
    }
}

/// The default button's ring: three pixels wide, three pixels out from the
/// button, with a black outer edge, and greys that run into the button's
/// own corners.
fn draw_default_ring(pen: &mut Pen<'_>, (t, l, b, r): (i16, i16, i16, i16)) {
    let (g60, g148, g161, g191, g203, g215) = (
        grey(60),
        grey(148),
        grey(161),
        grey(191),
        grey(203),
        grey(215),
    );
    // Above the button.
    pen.dot(l, t - 3, g60);
    pen.hline(t - 3, l + 1, r - 2, grey(BLACK));
    pen.dot(r - 1, t - 3, g60);
    pen.dot(l - 1, t - 2, grey(BLACK));
    pen.hline(t - 2, l, r - 2, grey(BACKGROUND));
    pen.dot(r - 1, t - 2, g215);
    pen.dot(r, t - 2, grey(BLACK));
    pen.dot(l - 2, t - 1, grey(BLACK));
    pen.hline(t - 1, l - 1, l, grey(BACKGROUND));
    pen.hline(t - 1, l + 1, r - 1, g191);
    pen.dot(r, t - 1, g203);
    pen.dot(r + 1, t - 1, grey(BLACK));
    // Beside it: black outside, then the dialog background on the left and
    // a light and a mid grey on each side.
    pen.vline(l - 3, t + 1, b - 2, grey(BLACK));
    pen.vline(l - 2, t, b - 2, grey(BACKGROUND));
    pen.vline(l - 1, t + 1, b - 2, g191);
    pen.vline(r, t + 1, b - 2, g191);
    pen.vline(r + 1, t + 1, b - 2, g148);
    pen.vline(r + 2, t + 1, b - 2, grey(BLACK));
    // Where it meets the button's corners.
    pen.dot(l - 3, t, g60);
    pen.dot(l - 1, t, grey(BACKGROUND));
    pen.dot(l, t, g191);
    pen.dot(l + 1, t, g148);
    pen.dot(l, t + 1, g148);
    pen.dot(r - 2, t, g148);
    pen.dot(r - 1, t, g191);
    pen.dot(r, t, g191);
    pen.dot(r + 1, t, g161);
    pen.dot(r + 2, t, g60);
    pen.dot(r - 1, t + 1, g148);
    pen.dot(l, b - 2, g148);
    pen.dot(r - 1, b - 2, g148);
    // Below it.
    pen.dot(l - 3, b - 1, g60);
    pen.dot(l - 2, b - 1, g215);
    pen.dot(l - 1, b - 1, g191);
    pen.dot(l, b - 1, g191);
    pen.dot(l + 1, b - 1, g148);
    pen.dot(r - 2, b - 1, g148);
    pen.dot(r - 1, b - 1, g191);
    pen.dot(r, b - 1, g161);
    pen.dot(r + 1, b - 1, g148);
    pen.dot(r + 2, b - 1, g60);
    pen.dot(l - 2, b, grey(BLACK));
    pen.dot(l - 1, b, g203);
    pen.hline(b, l, r - 2, g191);
    pen.dot(r - 1, b, g161);
    pen.dot(r, b, g148);
    pen.dot(r + 1, b, grey(BLACK));
    pen.dot(l - 1, b + 1, grey(BLACK));
    pen.dot(l, b + 1, g161);
    pen.hline(b + 1, l + 1, r - 1, g148);
    pen.dot(r, b + 1, grey(BLACK));
    pen.dot(l, b + 2, g60);
    pen.hline(b + 2, l + 1, r - 2, grey(BLACK));
    pen.dot(r - 1, b + 2, g60);
}

/// A check box: a twelve-pixel box two pixels in from the control's left
/// and centred on its height, white inside its top and left edges and mid
/// grey inside its bottom and right, the title six pixels after it.
fn draw_check_box(
    pen: &mut Pen<'_>,
    (t, l, b, _): (i16, i16, i16, i16),
    title: &[u8],
    checked: bool,
) {
    let (x, y) = (l + 2, t + (b - t - 12) / 2);
    pen.rect((y + 1, x + 1, y + 11, x + 11), grey(BACKGROUND));
    pen.hline(y, x, x + 11, grey(BLACK));
    pen.hline(y + 11, x, x + 11, grey(BLACK));
    pen.vline(x, y, y + 11, grey(BLACK));
    pen.vline(x + 11, y, y + 11, grey(BLACK));
    pen.hline(y + 1, x + 1, x + 9, grey(WHITE));
    pen.vline(x + 1, y + 1, y + 9, grey(WHITE));
    pen.hline(y + 10, x + 2, x + 10, grey(161));
    pen.vline(x + 10, y + 2, y + 10, grey(161));
    if checked {
        for &(dx, dy, value) in TICK {
            pen.dot(x + dx, y + dy, grey(value));
        }
    }
    if !title.is_empty() {
        let (_, ascent, descent) = title_metrics(title);
        pen.text((x + 17, centred_baseline(t, b, ascent, descent)), title);
    }
}

/// A group box: an embossed frame, a mid grey line with a white one inside
/// it, whose top runs level with the title's baseline and breaks three
/// pixels before the title's pen, which is twelve pixels in, and four after
/// its advance.
fn draw_group_box(pen: &mut Pen<'_>, (t, l, b, r): (i16, i16, i16, i16), title: &[u8]) {
    let (advance, ascent, _) = title_metrics(title);
    let frame_top = t + ascent - 1;
    let text_left = l + 12;
    let (gap_start, gap_end) = if title.is_empty() {
        (r, r)
    } else {
        (text_left - 3, text_left + advance + 4)
    };
    let dark = grey(161);
    let light = grey(WHITE);
    for (v, color, h0, h1) in [
        (frame_top, dark, l, r - 2),
        (frame_top + 1, light, l + 1, r - 3),
    ] {
        if gap_start > h0 {
            pen.hline(v, h0, (gap_start - 1).min(h1), color);
        }
        if gap_end <= h1 {
            pen.hline(v, gap_end.max(h0), h1, color);
        }
    }
    pen.vline(l, frame_top, b - 2, dark);
    pen.vline(l + 1, frame_top + 1, b - 3, light);
    pen.vline(r - 2, frame_top, b - 2, dark);
    pen.vline(r - 1, frame_top, b - 1, light);
    pen.hline(b - 2, l, r - 2, dark);
    pen.hline(b - 1, l, r - 1, light);
    if !title.is_empty() {
        pen.text((text_left, t + ascent), title);
    }
}

/// A horizontal slider: a grooved track four pixels below the control's
/// top, rounded at both ends, and a blue thumb with three grip lines level
/// with the top.
fn draw_slider(
    pen: &mut Pen<'_>,
    (t, l, b, r): (i16, i16, i16, i16),
    value: i16,
    min: i16,
    max: i16,
) {
    let (g60, g191) = (grey(60), grey(191));
    pen.rect((t, l, b.min(t + THUMB_HEIGHT), r), grey(BACKGROUND));
    pen.hline(t + 4, l + 3, r - 5, g191);
    pen.hline(t + 5, l + 2, l + 3, g191);
    pen.hline(t + 5, l + 4, r - 5, g60);
    pen.dot(r - 4, t + 5, g191);
    for v in t + 6..=t + 8 {
        pen.dot(l + 2, v, g191);
        pen.dot(l + 3, v, g60);
        pen.hline(v, l + 4, r - 5, g191);
        pen.dot(r - 4, v, g60);
        pen.dot(r - 3, v, grey(WHITE));
    }
    pen.dot(l + 3, t + 9, grey(WHITE));
    pen.hline(t + 9, l + 4, r - 5, g60);
    pen.hline(t + 9, r - 4, r - 3, grey(WHITE));
    pen.hline(t + 10, l + 4, r - 4, grey(WHITE));

    let track = ppc_platinum_slider_track(l, r);
    let x = track.start + ppc_platinum_thumb_offset(track.travel, value, min, max);
    let (light, fill, edge, shine, grip) = (
        rgb(203, 230, 252),
        rgb(181, 221, 252),
        rgb(133, 133, 161),
        grey(241),
        rgb(0, 114, 181),
    );
    pen.hline(t, x + 1, x + 11, grey(BLACK));
    pen.hline(t + 15, x + 1, x + 11, grey(BLACK));
    pen.vline(x, t + 1, t + 14, grey(BLACK));
    pen.vline(x + 12, t + 1, t + 14, grey(BLACK));
    pen.rect((t + 1, x + 1, t + 15, x + 12), fill);
    pen.dot(x + 1, t + 1, shine);
    pen.hline(t + 1, x + 2, x + 10, light);
    pen.vline(x + 1, t + 2, t + 13, light);
    pen.vline(x + 11, t + 2, t + 13, edge);
    pen.dot(x + 1, t + 14, fill);
    pen.hline(t + 14, x + 2, x + 11, edge);
    for dx in [3, 5, 7] {
        pen.dot(x + dx, t + 4, shine);
        pen.vline(x + dx, t + 5, t + 10, light);
        pen.vline(x + dx + 1, t + 5, t + 11, grip);
    }
}
