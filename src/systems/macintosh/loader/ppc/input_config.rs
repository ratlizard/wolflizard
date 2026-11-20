//! ISpConfigure: InputSprocket's "Configure Inputs" window, in which the
//! player gives each of the application's needs a key.
//!
//! The layout is InputSprocketLib's own (InputSprocket 1.x on Mac OS 8.6,
//! DLOG and DITL 111): a device list at the left with its scroll bar, the
//! device's panel at the right, and under a separator the Sets pop-up,
//! Cancel and OK. The keyboard's panel is InputSprocket Keyboard's DITL 128:
//! pages of eight rows, each a need's name beside the field showing its key,
//! with page arrows and "Page 1 of 2" below. A need of kind 'move' takes four
//! rows, "Movement - Up" and on, as InputSprocket Keyboard's STR# 128 names
//! them. The key names are InputSprocket Keyboard's: 1000 plus the key code
//! for the keypad and function keys, 2000 plus the character for the named
//! keys, and the character itself otherwise. No Mac that could be run here
//! showed the window (InputSprocketLib does not load under SheepShaver or
//! QEMU), so the look of the parts the resources leave to code -- the list,
//! the key fields, the page arrows -- is drawn in Mac OS 8.5's Platinum
//! look, not measured.

use super::platinum::{
    draw_default_ring, draw_modal_frame_around, draw_push_button, grey, Pen, Shade, BACKGROUND,
    BLACK, WHITE,
};
use super::*;

const CHARCOAL: i16 = crate::quickdraw::fonts::FONT_CHARCOAL;
const GENEVA: i16 = 3;

const WIDTH: i16 = 581;
const HEIGHT: i16 = 365;
const OK: (i16, i16, i16, i16) = (337, 501, 357, 557);
const CANCEL: (i16, i16, i16, i16) = (337, 430, 357, 488);
const SETS: (i16, i16, i16, i16) = (337, 156, 358, 336);
const DEVICE_LIST: (i16, i16, i16, i16) = (0, 0, 364, 88);
const DEVICE_SCROLL: (i16, i16, i16, i16) = (0, 88, 364, 104);
const PANEL_LEFT: i16 = 112;
const ROWS_PER_PAGE: usize = 8;
const PAGE_ARROWS: (i16, i16, i16, i16) = (248, 122, 280, 154);
const KIND_MOVE: u32 = u32::from_be_bytes(*b"move");

/// One row of the keyboard panel: a need, and for a 'move' need which of
/// its four directions.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Row {
    element: u32,
    label: Vec<u8>,
    direction: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PpcInputConfigureState {
    return_address: u32,
    rows: Vec<Row>,
    /// Each row's key code, or None.
    keys: Vec<Option<u8>>,
    focus: Option<usize>,
    page: usize,
    custom: bool,
    sets_open: bool,
    pressed: Option<bool>,
    message: Option<&'static [u8]>,
    pub(crate) bounds: (i16, i16, i16, i16),
    front_buffer: PpcFrontBuffer,
    saved_pixels: crate::memory::SavedPixels<(i32, i32, u16)>,
}

impl PpcInputConfigureState {
    fn pages(&self) -> usize {
        self.rows.len().div_ceil(ROWS_PER_PAGE).max(1)
    }
    fn origin(&self) -> (i16, i16) {
        (self.bounds.0 + 3, self.bounds.1 + 3)
    }
    fn field_rect(&self, slot: usize) -> (i16, i16, i16, i16) {
        let top = 20 + 30 * slot as i16;
        (top, PANEL_LEFT + 143, top + 16, PANEL_LEFT + 283)
    }
}

/// ISpConfigure(inEventProc): opens the window on the first call; each call
/// after it, made again by the runtime while it is open, handles one event.
#[allow(clippy::too_many_arguments)]
pub(super) fn ppc_dispatch_isp_configure(
    cpu: &mut PpcCpu,
    memory: &mut PpcSectionMem,
    startup: &mut PpcToolboxStartupState,
    gworlds: &[PpcGWorldRecord],
    event_queue: &mut EventQueue,
    input_sprocket: &mut PpcInputSprocketState,
    virtual_elements: &mut [PpcInputSprocketVirtualElementRecord],
) -> PpcImportAction {
    let Some(mut state) = startup.input_configure.take() else {
        input_sprocket.initialized = true;
        input_sprocket.configure_count = input_sprocket.configure_count.saturating_add(1);
        return open(cpu, memory, startup, gworlds, input_sprocket, virtual_elements);
    };
    let close = |memory: &mut PpcSectionMem, state: &PpcInputConfigureState| {
        super::dispatch_standard_file::ppc_standard_file_restore_pixels(
            memory,
            &state.saved_pixels,
            state.front_buffer,
        );
        PpcImportAction::Return(ppc_i16_result(PPC_NO_ERR))
    };
    if state.return_address != cpu.lr {
        return close(memory, &state);
    }
    let event = event_queue
        .iter()
        .position(|event| matches!(event.what, 1 | 2 | 3 | 5))
        .and_then(|index| event_queue.remove(index));
    let before = state.clone();
    let (mut ok, mut cancel) = (false, false);
    if let Some(event) = event {
        let (top, left) = state.origin();
        let local = (event.where_v - top, event.where_h - left);
        let inside = |rect: (i16, i16, i16, i16)| {
            local.0 >= rect.0 && local.0 < rect.2 && local.1 >= rect.1 && local.1 < rect.3
        };
        match event.what {
            1 => {
                state.message = None;
                if state.sets_open {
                    state.sets_open = false;
                    let menu = sets_menu_rect();
                    if inside(menu) && (local.0 - menu.0) / 16 == 0 {
                        // Default: no key for any need.
                        state.keys.iter_mut().for_each(|key| *key = None);
                        state.custom = false;
                    } else if inside(menu) && (local.0 - menu.0) / 16 == 1 {
                        state.custom = true;
                    }
                } else if inside(OK) {
                    state.pressed = Some(true);
                } else if inside(CANCEL) {
                    state.pressed = Some(false);
                } else if inside(SETS) {
                    state.sets_open = true;
                } else if inside(PAGE_ARROWS) {
                    if local.1 < (PAGE_ARROWS.1 + PAGE_ARROWS.3) / 2 {
                        if state.page == 0 {
                            state.message = Some(b"This is the first page.");
                        }
                        state.page = state.page.saturating_sub(1);
                    } else if state.page + 1 < state.pages() {
                        state.page += 1;
                    } else {
                        state.message = Some(b"This is the last page.");
                    }
                    state.focus = None;
                } else {
                    state.focus = (0..ROWS_PER_PAGE)
                        .find(|slot| inside(state.field_rect(*slot)))
                        .map(|slot| state.page * ROWS_PER_PAGE + slot)
                        .filter(|row| *row < state.rows.len());
                }
            }
            2 => match state.pressed.take() {
                Some(true) => ok = inside(OK),
                Some(false) => cancel = inside(CANCEL),
                None => {}
            },
            _ => {
                let character = event.message as u8;
                let key_code = (event.message >> 8) as u8;
                if let Some(row) = state.focus {
                    if key_code == 0x35 {
                        // Escape stays the dialog's: Cancel.
                        state.message = Some(b"Escape is already in use.");
                    } else if key_code == 0x33 {
                        state.keys[row] = None;
                        state.custom = true;
                    } else if event.modifiers & 0x0100 == 0 {
                        state.keys[row] = Some(key_code);
                        state.custom = true;
                        let next = row + 1;
                        state.focus = (next < state.rows.len()).then_some(next);
                        if let Some(next) = state.focus {
                            state.page = next / ROWS_PER_PAGE;
                        }
                    }
                } else if crate::dialog_manager::is_dialog_default_key(character, key_code) {
                    ok = true;
                } else if crate::dialog_manager::is_dialog_cancel_key(
                    character,
                    key_code,
                    event.modifiers,
                ) {
                    cancel = true;
                }
            }
        }
    }
    if cancel {
        return close(memory, &state);
    }
    if ok {
        apply(&state, virtual_elements);
        return close(memory, &state);
    }
    if state != before {
        draw(memory, gworlds, &state);
    }
    startup.input_configure = Some(state);
    PpcImportAction::Yield(u64::MAX)
}

fn open(
    cpu: &PpcCpu,
    memory: &mut PpcSectionMem,
    startup: &mut PpcToolboxStartupState,
    gworlds: &[PpcGWorldRecord],
    input_sprocket: &PpcInputSprocketState,
    virtual_elements: &[PpcInputSprocketVirtualElementRecord],
) -> PpcImportAction {
    let count = (input_sprocket.last_virtual_need_count as usize).min(virtual_elements.len());
    let records = &virtual_elements[virtual_elements.len() - count..];
    if records.is_empty() {
        return PpcImportAction::Return(ppc_i16_result(PPC_NO_ERR));
    }
    let Some(front_buffer) = ppc_front_buffer_for_gworld(gworlds, PPC_MAIN_GWORLD) else {
        return PpcImportAction::Return(ppc_i16_result(PPC_NO_ERR));
    };
    let mut rows = Vec::new();
    let mut keys = Vec::new();
    for record in records {
        let name = encode_mac_roman_lossy(&record.need_name);
        let binding = record.keyboard_binding;
        if record.kind == KIND_MOVE {
            for (direction, suffix) in [" - Up", " - Down", " - Left", " - Right"].into_iter().enumerate() {
                rows.push(Row {
                    element: record.element,
                    label: [name.as_slice(), suffix.as_bytes()].concat(),
                    direction: Some(direction),
                });
                keys.push(binding.filter(|b| b.len == 4).map(|b| b.keycodes[direction]));
            }
        } else {
            rows.push(Row {
                element: record.element,
                label: name,
                direction: None,
            });
            keys.push(binding.filter(|b| b.len == 1).map(|b| b.keycodes[0]));
        }
    }
    let custom = keys.iter().any(Option::is_some);
    let (screen_h, screen_w) = (front_buffer.height as i16, front_buffer.width as i16);
    let top = ((screen_h - HEIGHT) / 2).max(4);
    let left = ((screen_w - WIDTH) / 2).max(4);
    let bounds = (top - 3, left - 3, top + HEIGHT + 4, left + WIDTH + 4);
    let state = PpcInputConfigureState {
        return_address: cpu.lr,
        rows,
        keys,
        focus: None,
        page: 0,
        custom,
        sets_open: false,
        pressed: None,
        message: None,
        bounds,
        front_buffer,
        saved_pixels: super::dispatch_standard_file::ppc_standard_file_save_pixels(
            memory,
            front_buffer,
            bounds,
        ),
    };
    draw(memory, gworlds, &state);
    startup.input_configure = Some(state);
    PpcImportAction::Yield(u64::MAX)
}

/// OK: each need takes the keys its rows show.
fn apply(state: &PpcInputConfigureState, virtual_elements: &mut [PpcInputSprocketVirtualElementRecord]) {
    for record in virtual_elements.iter_mut() {
        let indexes: Vec<usize> = state
            .rows
            .iter()
            .enumerate()
            .filter(|(_, row)| row.element == record.element)
            .map(|(index, _)| index)
            .collect();
        if indexes.is_empty() {
            continue;
        }
        let keys: Vec<Option<u8>> = indexes.iter().map(|index| state.keys[*index]).collect();
        record.keyboard_binding = if state.rows[indexes[0]].direction.is_some() {
            keys.iter().any(Option::is_some).then(|| {
                let key = |direction: usize| keys.get(direction).copied().flatten().unwrap_or(0xff);
                PpcInputSprocketKeyboardBinding::dpad(key(0), key(1), key(2), key(3))
            })
        } else {
            keys[0].map(PpcInputSprocketKeyboardBinding::button)
        };
    }
}

/// InputSprocket Keyboard's name for a key code.
fn key_name(key_code: u8) -> Vec<u8> {
    let named: Option<&str> = match key_code {
        36 => Some("Return"),
        48 => Some("Tab"),
        49 => Some("Space"),
        51 => Some("Delete"),
        53 => Some("Escape"),
        76 => Some("Enter"),
        123 => Some("Left"),
        124 => Some("Right"),
        125 => Some("Down"),
        126 => Some("Up"),
        114 => Some("Help"),
        115 => Some("Home"),
        116 => Some("Page Up"),
        117 => Some("Del"),
        119 => Some("End"),
        121 => Some("Page Down"),
        65 => Some("Num ."),
        67 => Some("Num *"),
        69 => Some("Num +"),
        71 => Some("Clear"),
        75 => Some("Num /"),
        78 => Some("Num -"),
        81 => Some("Num ="),
        82 => Some("Num 0"),
        83 => Some("Num 1"),
        84 => Some("Num 2"),
        85 => Some("Num 3"),
        86 => Some("Num 4"),
        87 => Some("Num 5"),
        88 => Some("Num 6"),
        89 => Some("Num 7"),
        91 => Some("Num 8"),
        92 => Some("Num 9"),
        122 => Some("F1"),
        120 => Some("F2"),
        99 => Some("F3"),
        118 => Some("F4"),
        96 => Some("F5"),
        97 => Some("F6"),
        98 => Some("F7"),
        100 => Some("F8"),
        101 => Some("F9"),
        109 => Some("F10"),
        103 => Some("F11"),
        111 => Some("F12"),
        _ => None,
    };
    if let Some(name) = named {
        return name.as_bytes().to_vec();
    }
    const US: &[u8; 51] = b"ASDFHGZXCV\0BQWERYT123465=97-80]OU[IP\0LJ'K;\\,/NM.\0\0`";
    match US.get(usize::from(key_code)) {
        Some(&character) if character != 0 => vec![character],
        _ => b"Unknown key".to_vec(),
    }
}

struct Canvas<'a> {
    pen: Pen<'a>,
    top: i16,
    left: i16,
}

impl Canvas<'_> {
    fn rect(&mut self, (t, l, b, r): (i16, i16, i16, i16), shade: Shade) {
        self.pen.rect((self.top + t, self.left + l, self.top + b, self.left + r), shade);
    }
    fn hline(&mut self, v: i16, h0: i16, h1: i16, shade: Shade) {
        self.pen.hline(self.top + v, self.left + h0, self.left + h1, shade);
    }
    fn vline(&mut self, h: i16, v0: i16, v1: i16, shade: Shade) {
        self.pen.vline(self.left + h, self.top + v0, self.top + v1, shade);
    }
    fn dot(&mut self, h: i16, v: i16, shade: Shade) {
        self.pen.dot(self.left + h, self.top + v, shade);
    }
    fn text(&mut self, (h, v): (i16, i16), bytes: &[u8], font: i16, size: i16) {
        self.pen
            .text_font((self.left + h, self.top + v), bytes, PPC_RGB_BLACK, font, size);
    }
    fn global(&self, (t, l, b, r): (i16, i16, i16, i16)) -> (i16, i16, i16, i16) {
        (self.top + t, self.left + l, self.top + b, self.left + r)
    }
}

fn sets_menu_rect() -> (i16, i16, i16, i16) {
    (SETS.0 - 16 * 2, SETS.1 + 40, SETS.0, SETS.3)
}

fn draw(memory: &mut PpcSectionMem, gworlds: &[PpcGWorldRecord], state: &PpcInputConfigureState) {
    ppc_with_open_window_manager_port(memory, |memory| {
        let (top, left) = state.origin();
        let mut canvas = Canvas {
            pen: Pen::new(memory, gworlds, PPC_MAIN_GWORLD),
            top,
            left,
        };
        draw_modal_frame_around(&mut canvas.pen, (top, left, top + HEIGHT, left + WIDTH));
        canvas.rect((0, 0, HEIGHT, WIDTH), grey(BACKGROUND));
        draw_device_list(&mut canvas);
        draw_panel(&mut canvas, state);
        // The separator above the buttons: an etched line.
        canvas.hline(324, 118, 552, grey(161));
        canvas.hline(325, 119, 553, grey(WHITE));
        draw_sets_popup(&mut canvas, state);
        let cancel = canvas.global(CANCEL);
        draw_push_button(&mut canvas.pen, cancel, b"Cancel", false, state.pressed == Some(false));
        let ok = canvas.global(OK);
        draw_default_ring(&mut canvas.pen, ok);
        draw_push_button(&mut canvas.pen, ok, b"OK", false, state.pressed == Some(true));
        if state.sets_open {
            draw_sets_menu(&mut canvas);
        }
    });
}

/// The device list: the keyboard, selected, with its name below its icon.
fn draw_device_list(canvas: &mut Canvas<'_>) {
    let (t, l, b, r) = DEVICE_LIST;
    canvas.rect((t, l, b, r), grey(WHITE));
    canvas.hline(t, l, r - 1, grey(BLACK));
    canvas.hline(b - 1, l, r - 1, grey(BLACK));
    canvas.vline(l, t, b - 1, grey(BLACK));
    // The selected device's cell, in the highlight grey.
    canvas.rect((t + 4, l + 4, t + 58, r - 3), grey(203));
    draw_keyboard_icon(canvas, (t + 10, (l + r) / 2 - 16));
    let name = b"Keyboard";
    let advance = ppc_text_bytes_advance_for_font(name, GENEVA, 9);
    canvas.text(((l + r - advance) / 2, t + 53), name, GENEVA, 9);
    // The scroll bar, inactive: nothing to scroll to.
    let (t, l, b, r) = DEVICE_SCROLL;
    canvas.rect((t, l, b, r), grey(241));
    canvas.hline(t, l, r - 1, grey(BLACK));
    canvas.hline(b - 1, l, r - 1, grey(BLACK));
    canvas.vline(l, t, b - 1, grey(BLACK));
    canvas.vline(r - 1, t, b - 1, grey(BLACK));
    let mid = l + 7;
    for row in 0..4i16 {
        canvas.hline(t + 5 + row, mid - row, mid + 1 + row, grey(161));
        canvas.hline(b - 9 + row, mid - 3 + row, mid + 4 - row, grey(161));
    }
}

/// A keyboard seen from above: a pale case and three rows of keys.
fn draw_keyboard_icon(canvas: &mut Canvas<'_>, (v, h): (i16, i16)) {
    canvas.rect((v + 8, h, v + 26, h + 32), grey(BLACK));
    canvas.rect((v + 9, h + 1, v + 25, h + 31), grey(221));
    for row in 0..3i16 {
        for key in 0..7i16 {
            let (kv, kh) = (v + 11 + row * 4, h + 3 + key * 4 + row);
            canvas.rect((kv, kh, kv + 3, kh + 3), grey(WHITE));
            canvas.hline(kv + 3, kh, kh + 2, grey(118));
        }
    }
    canvas.rect((v + 23, h + 8, v + 24, h + 24), grey(118));
}

/// The keyboard's panel: this page's rows, the page arrows and the page
/// number.
fn draw_panel(canvas: &mut Canvas<'_>, state: &PpcInputConfigureState) {
    let first = state.page * ROWS_PER_PAGE;
    for slot in 0..ROWS_PER_PAGE {
        let index = first + slot;
        let Some(row) = state.rows.get(index) else {
            break;
        };
        let field = state.field_rect(slot);
        let advance = ppc_text_bytes_advance_for_font(&row.label, CHARCOAL, 12);
        canvas.text((PANEL_LEFT + 140 - advance, field.0 + 13), &row.label, CHARCOAL, 12);
        draw_key_field(canvas, field, state.keys[index], state.focus == Some(index));
    }
    draw_page_arrows(canvas);
    let page = format!("Page {} of {}", state.page + 1, state.pages());
    canvas.text((PANEL_LEFT + 150, 262 + 13), page.as_bytes(), GENEVA, 10);
    canvas.hline(281, PANEL_LEFT + 11, PANEL_LEFT + 292, grey(161));
    canvas.hline(282, PANEL_LEFT + 12, PANEL_LEFT + 292, grey(WHITE));
    if let Some(message) = state.message {
        canvas.text((PANEL_LEFT + 20, 305), message, GENEVA, 10);
    }
}

/// A key's field: a white box framed like an edit-text field, the key's
/// name centred in it, and a focus ring when it is waiting for a key.
fn draw_key_field(canvas: &mut Canvas<'_>, (t, l, b, r): (i16, i16, i16, i16), key: Option<u8>, focused: bool) {
    let (b, r) = (b - 1, r - 1);
    if focused {
        let ring = super::platinum::rgb(133, 133, 161);
        canvas.hline(t - 2, l - 1, r + 1, ring);
        canvas.hline(t - 1, l - 2, r + 2, ring);
        canvas.hline(b + 1, l - 2, r + 2, ring);
        canvas.hline(b + 2, l - 1, r + 1, ring);
        canvas.vline(l - 2, t, b, ring);
        canvas.vline(l - 1, t, b, ring);
        canvas.vline(r + 1, t, b, ring);
        canvas.vline(r + 2, t, b, ring);
    } else {
        canvas.hline(t - 1, l - 1, r, grey(161));
        canvas.vline(l - 1, t - 1, b, grey(161));
        canvas.hline(b + 1, l, r + 1, grey(WHITE));
        canvas.vline(r + 1, t, b + 1, grey(WHITE));
    }
    canvas.rect((t, l, b + 1, r + 1), grey(WHITE));
    canvas.hline(t, l, r, grey(BLACK));
    canvas.hline(b, l, r, grey(BLACK));
    canvas.vline(l, t, b, grey(BLACK));
    canvas.vline(r, t, b, grey(BLACK));
    let name = key.map_or_else(|| b"None".to_vec(), key_name);
    let advance = ppc_text_bytes_advance_for_font(&name, GENEVA, 10);
    canvas.text(((l + r - advance) / 2, t + 12), &name, GENEVA, 10);
}

/// Two arrows in a 32-pixel square: the earlier page and the later one.
fn draw_page_arrows(canvas: &mut Canvas<'_>) {
    let (t, l, b, r) = PAGE_ARROWS;
    let mid_v = (t + b) / 2;
    for column in 0..6i16 {
        canvas.vline(l + 3 + column, mid_v - column, mid_v + column, grey(BLACK));
        canvas.vline(r - 4 - column, mid_v - column, mid_v + column, grey(BLACK));
    }
    canvas.dot((l + r) / 2, mid_v, grey(118));
}

/// The Sets pop-up: its title, then the set's name in a pop-up button.
fn draw_sets_popup(canvas: &mut Canvas<'_>, state: &PpcInputConfigureState) {
    let (t, l, b, r) = SETS;
    canvas.text((l, t + 15), b"Sets:", CHARCOAL, 12);
    let button = (t, l + 40, b - 1, r);
    let (bt, bl, bb, br) = button;
    canvas.rect((bt + 1, bl + 1, bb - 1, br - 1), grey(BACKGROUND));
    canvas.hline(bt, bl + 2, br - 3, grey(BLACK));
    canvas.hline(bb - 1, bl + 2, br - 3, grey(BLACK));
    canvas.vline(bl, bt + 2, bb - 3, grey(BLACK));
    canvas.vline(br - 1, bt + 2, bb - 3, grey(BLACK));
    canvas.dot(bl + 1, bt + 1, grey(BLACK));
    canvas.dot(br - 2, bt + 1, grey(BLACK));
    canvas.dot(bl + 1, bb - 2, grey(BLACK));
    canvas.dot(br - 2, bb - 2, grey(BLACK));
    canvas.hline(bt + 1, bl + 2, br - 3, grey(WHITE));
    canvas.vline(bl + 1, bt + 2, bb - 3, grey(WHITE));
    canvas.hline(bb - 2, bl + 2, br - 3, grey(148));
    canvas.vline(br - 2, bt + 2, bb - 3, grey(148));
    let name: &[u8] = if state.custom { b"Custom" } else { b"Default" };
    canvas.text((bl + 8, bt + 15), name, CHARCOAL, 12);
    let part = br - 20;
    canvas.vline(part, bt + 2, bb - 3, grey(191));
    canvas.vline(part + 1, bt + 2, bb - 3, grey(WHITE));
    let centre = part + 9;
    let mid = (bt + bb) / 2;
    for row in 0..4i16 {
        canvas.hline(mid - 5 + row, centre - row, centre + row, grey(BLACK));
        canvas.hline(mid + 4 - row, centre - row, centre + row, grey(BLACK));
    }
}

fn draw_sets_menu(canvas: &mut Canvas<'_>) {
    let (t, l, b, r) = sets_menu_rect();
    canvas.rect((t, l, b, r), grey(BACKGROUND));
    canvas.hline(t, l, r - 1, grey(BLACK));
    canvas.hline(b - 1, l, r - 1, grey(BLACK));
    canvas.vline(l, t, b - 1, grey(BLACK));
    canvas.vline(r - 1, t, b - 1, grey(BLACK));
    canvas.vline(r, t + 2, b, grey(BLACK));
    canvas.hline(b, l + 2, r, grey(BLACK));
    canvas.text((l + 12, t + 13), b"Default", CHARCOAL, 12);
    canvas.text((l + 12, t + 29), b"Custom", CHARCOAL, 12);
    let _ = canvas.dot(l, t, grey(BLACK));
}
