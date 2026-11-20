//! Navigation Services' NavGetFile and NavPutFile windows, drawn as Mac OS
//! 8.5 draws them.
//!
//! Every place and colour was measured from Cythera's "Create Player:" and
//! "Select Player File" windows on Mac OS 8.5 under SheepShaver (800 by 600,
//! 256 colours). The window is a movable modal window: a striped title bar,
//! a location pop-up and three shortcut buttons above a list with Name and
//! Date Modified columns, and below it either the name field (NavPutFile) or
//! Show Preview (NavGetFile), then a help button, Cancel and the default
//! button. The small icons are drawn here in the same places and colours,
//! not copied from the system's.
//!
//! The window is drawn over the screen and the pixels under it are put back
//! when it closes, as the Standard File windows of `dispatch_standard_file`
//! are, and its reply is the NavReplyRecord that module writes.

use super::dispatch_standard_file::{
    ppc_navigation_write_put_reply, ppc_standard_file_centered_bounds,
    ppc_standard_file_folder_chain, ppc_standard_file_get_entries,
    ppc_standard_file_point_in_rect, ppc_standard_file_restore_pixels,
    ppc_standard_file_save_pixels, PpcNavigationReplyContext, PpcStandardFileEntry,
    PPC_NAV_REPLY_RECORD_SIZE, PPC_NAV_REPLY_SELECTION_OFFSET,
};
use super::platinum::{
    draw_default_ring, draw_push_button, grey, rgb, Pen, Shade, BACKGROUND, BLACK, WHITE,
};
use super::*;

const CHARCOAL: i16 = crate::quickdraw::fonts::FONT_CHARCOAL;
const GENEVA: i16 = 3;

/// The frame's width, black edge to black edge; a one-pixel shadow lies
/// right of it and below it.
const FRAME_WIDTH: i16 = 429;
const PUT_FRAME_HEIGHT: i16 = 293;
const GET_FRAME_HEIGHT: i16 = 287;

// Places in the frame, top, left, bottom, right, the bottom and right
// excluded.
const POPUP: (i16, i16, i16, i16) = (35, 15, 55, 215);
const SHORTCUT_BUTTONS: [i16; 3] = [305, 341, 377];
const SHORTCUT_TOP: i16 = 33;
const LIST_TOP: i16 = 63;
const LIST_LEFT: i16 = 15;
const LIST_RIGHT: i16 = 413;
const PUT_LIST_BOTTOM: i16 = 213;
const GET_LIST_BOTTOM: i16 = 210;
const HEADER_BOTTOM: i16 = 84;
const NAME_COLUMN_RIGHT: i16 = 314;
const SCROLL_LEFT: i16 = 397;
const ROW_HEIGHT: i16 = 20;
const NAME_FIELD: (i16, i16, i16, i16) = (221, 72, 243, 292);
const NEW_BUTTON: (i16, i16, i16, i16) = (222, 331, 242, 416);
const SHOW_PREVIEW_BUTTON: (i16, i16, i16, i16) = (217, 314, 237, 414);
const HELP_LEFT: i16 = 15;
const CANCEL_LEFT: i16 = 236;
const DEFAULT_LEFT: i16 = 331;
const BUTTON_WIDTH: i16 = 83;

/// One NavGetFile or NavPutFile window while it is open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PpcNavigationDialogState {
    get: bool,
    return_address: u32,
    reply: u32,
    title: Vec<u8>,
    name: Vec<u8>,
    sel_start: usize,
    sel_end: usize,
    file_types: Option<Vec<u32>>,
    dir_id: u32,
    entries: Vec<PpcStandardFileEntry>,
    selected: Option<usize>,
    first_row: usize,
    folder_chain: Vec<(u32, Vec<u8>)>,
    popup_open: bool,
    last_click: Option<(u32, usize)>,
    pressed: Option<Press>,
    pub(crate) bounds: (i16, i16, i16, i16),
    front_buffer: PpcFrontBuffer,
    saved_pixels: crate::memory::SavedPixels<(i32, i32, u16)>,
}

/// A push button held down, drawn pressed until the button is released.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Press {
    Cancel,
    Default,
}

impl PpcNavigationDialogState {
    fn frame_height(&self) -> i16 {
        if self.get {
            GET_FRAME_HEIGHT
        } else {
            PUT_FRAME_HEIGHT
        }
    }

    fn list_bottom(&self) -> i16 {
        if self.get {
            GET_LIST_BOTTOM
        } else {
            PUT_LIST_BOTTOM
        }
    }

    fn visible_rows(&self) -> usize {
        ((self.list_bottom() - 1 - HEADER_BOTTOM) / ROW_HEIGHT) as usize
    }

    fn buttons_top(&self) -> i16 {
        self.frame_height() - 35
    }

    fn cancel_rect(&self) -> (i16, i16, i16, i16) {
        let top = self.buttons_top();
        (top, CANCEL_LEFT, top + 20, CANCEL_LEFT + BUTTON_WIDTH)
    }

    fn default_rect(&self) -> (i16, i16, i16, i16) {
        let top = self.buttons_top();
        (top, DEFAULT_LEFT, top + 20, DEFAULT_LEFT + BUTTON_WIDTH)
    }

    /// The frame-relative point of a global one.
    fn local(&self, (v, h): (i16, i16)) -> (i16, i16) {
        (v - self.bounds.0, h - self.bounds.1)
    }

    fn reload(
        &mut self,
        vfs_directories: &[PpcVfsDirectory],
        vfs_files: &[PpcVfsFileRecord],
        vfs_resource_files: &[PpcVfsResourceFileRecord],
    ) {
        self.entries = ppc_standard_file_get_entries(
            vfs_directories,
            vfs_files,
            vfs_resource_files,
            self.dir_id,
            None,
        );
        if let Some(types) = &self.file_types {
            self.entries
                .retain(|entry| entry.is_directory || types.contains(&entry.file_type));
        }
        self.folder_chain = ppc_standard_file_folder_chain(vfs_directories, self.dir_id);
        self.selected = (self.get && !self.entries.is_empty()).then_some(0);
        self.first_row = 0;
        self.last_click = None;
    }

    fn keep_selection_visible(&mut self) {
        let rows = self.visible_rows().max(1);
        if let Some(selected) = self.selected {
            if selected < self.first_row {
                self.first_row = selected;
            } else if selected >= self.first_row + rows {
                self.first_row = selected + 1 - rows;
            }
        }
        let last = self.entries.len().saturating_sub(rows);
        self.first_row = self.first_row.min(last);
    }

    /// Whether the default button acts on the selection: a folder opens, and
    /// in NavGetFile a file is chosen.
    fn default_enabled(&self) -> bool {
        if self.get {
            self.selected.is_some()
        } else {
            !self.name.is_empty()
        }
    }
}

/// NavGetFile and NavPutFile: the first call opens the window, and each call
/// after it, made again by the runtime while the window is open, handles the
/// next mouse or keyboard event.
#[allow(clippy::too_many_arguments)]
pub(super) fn ppc_dispatch_navigation_dialog(
    get: bool,
    cpu: &mut PpcCpu,
    memory: &mut PpcSectionMem,
    startup: &mut PpcToolboxStartupState,
    process_memory_manager: &mut ProcessNativeMemoryManager,
    heap_cursor: &mut u32,
    last_mem_error: &mut i16,
    gworlds: &[PpcGWorldRecord],
    vfs_directories: &mut Vec<PpcVfsDirectory>,
    vfs_files: &ProcessVfsFileRecords,
    vfs_resource_files: &[PpcVfsResourceFileRecord],
    default_dir_id: u32,
    event_queue: &mut EventQueue,
    navigation: &mut PpcNavigationReplyContext<'_>,
) -> PpcImportAction {
    let Some(mut dialog) = startup.navigation_dialog.take() else {
        return open(
            get,
            cpu,
            memory,
            startup,
            gworlds,
            vfs_directories,
            vfs_files,
            vfs_resource_files,
            default_dir_id,
        );
    };
    let finish = |memory: &mut PpcSectionMem,
                  process_memory_manager: &mut ProcessNativeMemoryManager,
                  heap_cursor: &mut u32,
                  last_mem_error: &mut i16,
                  navigation: &mut PpcNavigationReplyContext<'_>,
                  dialog: PpcNavigationDialogState,
                  chosen: Option<(u32, Vec<u8>, bool)>| {
        ppc_standard_file_restore_pixels(memory, &dialog.saved_pixels, dialog.front_buffer);
        let saved = chosen
            .as_ref()
            .map(|(dir_id, name, replacing)| (PPC_BOOT_VOLUME_REF_NUM, *dir_id, name.as_slice(), *replacing));
        let result = ppc_navigation_write_put_reply(
            memory,
            process_memory_manager,
            heap_cursor,
            last_mem_error,
            navigation,
            dialog.reply,
            saved,
        );
        PpcImportAction::Return(ppc_i16_result(result))
    };
    if dialog.return_address != cpu.lr {
        return finish(
            memory,
            process_memory_manager,
            heap_cursor,
            last_mem_error,
            navigation,
            dialog,
            None,
        );
    }
    let event = event_queue
        .iter()
        .position(|event| matches!(event.what, 1 | 2 | 3 | 5))
        .and_then(|index| event_queue.remove(index));
    let before = dialog.clone();
    let mut accept = false;
    let mut cancel = false;
    if let Some(event) = event {
        let local = dialog.local((event.where_v, event.where_h));
        match event.what {
            1 => mouse_down(
                &mut dialog,
                local,
                event.when,
                memory,
                vfs_directories,
                vfs_files,
                vfs_resource_files,
            ),
            2 => match dialog.pressed.take() {
                Some(Press::Cancel) => {
                    cancel = ppc_standard_file_point_in_rect(local, dialog.cancel_rect());
                }
                Some(Press::Default) => {
                    accept = ppc_standard_file_point_in_rect(local, dialog.default_rect());
                }
                None => {}
            },
            _ => {
                let character = event.message as u8;
                let key_code = (event.message >> 8) as u8;
                if crate::dialog_manager::is_dialog_default_key(character, key_code) {
                    accept = true;
                } else if crate::dialog_manager::is_dialog_cancel_key(
                    character,
                    key_code,
                    event.modifiers,
                ) {
                    cancel = true;
                } else if key_code == 0x7e || character == 0x1e {
                    dialog.selected = Some(dialog.selected.map_or(0, |s| s.saturating_sub(1)));
                } else if (key_code == 0x7d || character == 0x1f) && !dialog.entries.is_empty() {
                    dialog.selected = Some(
                        dialog
                            .selected
                            .map_or(0, |s| (s + 1).min(dialog.entries.len() - 1)),
                    );
                } else if !dialog.get {
                    edit_name(&mut dialog, character, key_code, event.modifiers);
                }
                if dialog.selected.is_some_and(|s| s >= dialog.entries.len()) {
                    dialog.selected = None;
                }
            }
        }
    }
    if cancel {
        return finish(
            memory,
            process_memory_manager,
            heap_cursor,
            last_mem_error,
            navigation,
            dialog,
            None,
        );
    }
    if accept && dialog.default_enabled() {
        let selected = dialog.selected.and_then(|index| dialog.entries.get(index)).cloned();
        match selected {
            Some(entry) if entry.is_directory => {
                dialog.dir_id = entry.dir_id;
                dialog.reload(vfs_directories, vfs_files, vfs_resource_files);
            }
            Some(entry) if dialog.get => {
                let name = entry.name.clone();
                return finish(
                    memory,
                    process_memory_manager,
                    heap_cursor,
                    last_mem_error,
                    navigation,
                    dialog,
                    Some((entry.dir_id, name, false)),
                );
            }
            _ if !dialog.get => {
                let replacing = ppc_fsspec_target_exists(
                    vfs_directories,
                    vfs_files,
                    vfs_resource_files,
                    dialog.dir_id,
                    &dialog.name,
                );
                let chosen = Some((dialog.dir_id, dialog.name.clone(), replacing));
                return finish(
                    memory,
                    process_memory_manager,
                    heap_cursor,
                    last_mem_error,
                    navigation,
                    dialog,
                    chosen,
                );
            }
            _ => {}
        }
    }
    dialog.keep_selection_visible();
    if dialog != before {
        draw(memory, gworlds, &dialog);
    }
    startup.navigation_dialog = Some(dialog);
    PpcImportAction::Yield(u64::MAX)
}

#[allow(clippy::too_many_arguments)]
fn open(
    get: bool,
    cpu: &PpcCpu,
    memory: &mut PpcSectionMem,
    startup: &mut PpcToolboxStartupState,
    gworlds: &[PpcGWorldRecord],
    vfs_directories: &[PpcVfsDirectory],
    vfs_files: &[PpcVfsFileRecord],
    vfs_resource_files: &[PpcVfsResourceFileRecord],
    default_dir_id: u32,
) -> PpcImportAction {
    // NavDialogOptions (Navigation.h, mac68k alignment): windowTitle at 266,
    // savedFileName at 1034.
    let options = cpu.gpr[5];
    let reply = cpu.gpr[4];
    let read = |memory: &mut PpcSectionMem, offset: u32| {
        if options == 0 {
            Vec::new()
        } else {
            ppc_read_pstring_bytes(memory, options + offset).unwrap_or_default()
        }
    };
    let mut title = read(memory, 266);
    if title.is_empty() {
        title = if get { b"Open".to_vec() } else { b"Save".to_vec() };
    }
    let mut name = if get { Vec::new() } else { read(memory, 1034) };
    if !get && name.is_empty() {
        name = b"untitled".to_vec();
    }
    name.truncate(31);
    // NavTypeList: componentSignature, a reserved word, osTypeCount at +6
    // and the types from +8.
    let file_types = (get && cpu.gpr[9] != 0)
        .then(|| memory.read_u32_be(cpu.gpr[9]))
        .flatten()
        .filter(|list| *list != 0)
        .and_then(|list| {
            let count = memory.read_u16_be(list + 6)?.min(64);
            (0..u32::from(count))
                .map(|index| memory.read_u32_be(list + 8 + index * 4))
                .collect::<Option<Vec<u32>>>()
        })
        .filter(|types| !types.is_empty());
    let Some(front_buffer) = ppc_front_buffer_for_gworld(gworlds, PPC_MAIN_GWORLD) else {
        if reply != 0 && memory.write_bytes(reply, &[0; PPC_NAV_REPLY_RECORD_SIZE]).is_some() {
            let _ = memory.write_u32_be(reply + PPC_NAV_REPLY_SELECTION_OFFSET, PPC_TYPE_NULL);
        }
        return PpcImportAction::Return(ppc_i16_result(-128));
    };
    let height = if get { GET_FRAME_HEIGHT } else { PUT_FRAME_HEIGHT };
    let bounds = ppc_standard_file_centered_bounds(front_buffer, FRAME_WIDTH + 1, height + 1, None);
    let dir_id = ppc_directory_path_for_id(vfs_directories, default_dir_id)
        .map(|_| default_dir_id)
        .unwrap_or(PPC_ROOT_DIR_ID);
    let name_len = name.len();
    let mut dialog = PpcNavigationDialogState {
        get,
        return_address: cpu.lr,
        reply,
        title,
        name,
        sel_start: 0,
        sel_end: name_len,
        file_types,
        dir_id,
        entries: Vec::new(),
        selected: None,
        first_row: 0,
        folder_chain: Vec::new(),
        popup_open: false,
        last_click: None,
        pressed: None,
        bounds,
        front_buffer,
        saved_pixels: ppc_standard_file_save_pixels(memory, front_buffer, bounds),
    };
    dialog.reload(vfs_directories, vfs_files, vfs_resource_files);
    draw(memory, gworlds, &dialog);
    startup.navigation_dialog = Some(dialog);
    PpcImportAction::Yield(u64::MAX)
}

fn mouse_down(
    dialog: &mut PpcNavigationDialogState,
    local: (i16, i16),
    when: u32,
    memory: &mut PpcSectionMem,
    vfs_directories: &[PpcVfsDirectory],
    vfs_files: &[PpcVfsFileRecord],
    vfs_resource_files: &[PpcVfsResourceFileRecord],
) {
    let last_click = dialog.last_click.take();
    if dialog.popup_open {
        dialog.popup_open = false;
        let menu = popup_menu_rect(dialog);
        if ppc_standard_file_point_in_rect(local, menu) {
            let row = ((local.0 - menu.0 - 1) / POPUP_MENU_ROW_HEIGHT).max(0) as usize;
            if let Some(&(dir_id, _)) = dialog.folder_chain.get(row) {
                if dir_id != dialog.dir_id {
                    dialog.dir_id = dir_id;
                    dialog.reload(vfs_directories, vfs_files, vfs_resource_files);
                }
            }
        }
        return;
    }
    if ppc_standard_file_point_in_rect(local, POPUP) {
        dialog.popup_open = true;
    } else if ppc_standard_file_point_in_rect(local, dialog.cancel_rect()) {
        dialog.pressed = Some(Press::Cancel);
    } else if ppc_standard_file_point_in_rect(local, dialog.default_rect()) && dialog.default_enabled() {
        dialog.pressed = Some(Press::Default);
    } else if ppc_standard_file_point_in_rect(local, list_rows_rect(dialog)) {
        let row = ((local.0 - HEADER_BOTTOM) / ROW_HEIGHT) as usize;
        let index = dialog.first_row + row;
        if index >= dialog.entries.len() {
            dialog.selected = None;
            return;
        }
        let double_time = memory
            .read_u32_be(crate::memory::globals::addr::DOUBLE_TIME)
            .unwrap_or(PPC_DEFAULT_DOUBLE_TIME_TICKS);
        let double = last_click.is_some_and(|(at, last)| {
            last == index && when.wrapping_sub(at) <= double_time
        });
        dialog.selected = Some(index);
        if double && dialog.entries[index].is_directory {
            dialog.dir_id = dialog.entries[index].dir_id;
            dialog.reload(vfs_directories, vfs_files, vfs_resource_files);
        } else if double && dialog.get {
            // A double-click on a file chooses it, as Open does.
            dialog.pressed = Some(Press::Default);
        } else {
            dialog.last_click = Some((when, index));
            if !dialog.get && !dialog.entries[index].is_directory {
                dialog.name = dialog.entries[index].name.clone();
                dialog.name.truncate(31);
                dialog.sel_start = 0;
                dialog.sel_end = dialog.name.len();
            }
        }
    } else if ppc_standard_file_point_in_rect(local, scroll_rect(dialog)) {
        let rows = dialog.visible_rows();
        let span = scroll_rect(dialog);
        if local.0 < span.0 + 15 {
            dialog.first_row = dialog.first_row.saturating_sub(1);
        } else if local.0 >= span.2 - 15 {
            dialog.first_row += 1;
        } else if local.0 < (span.0 + span.2) / 2 {
            dialog.first_row = dialog.first_row.saturating_sub(rows);
        } else {
            dialog.first_row += rows;
        }
        let last = dialog.entries.len().saturating_sub(rows);
        dialog.first_row = dialog.first_row.min(last);
        // Scrolling alone does not move the selection into view.
        if let Some(selected) = dialog.selected {
            if selected < dialog.first_row || selected >= dialog.first_row + rows {
                dialog.last_click = None;
            }
        }
    } else if !dialog.get && ppc_standard_file_point_in_rect(local, NAME_FIELD) {
        dialog.sel_start = dialog.name.len();
        dialog.sel_end = dialog.name.len();
    }
}

/// A key typed into the name field: printable characters replace the
/// selection, Delete removes it or the character before it, Command-A
/// selects all.
fn edit_name(dialog: &mut PpcNavigationDialogState, character: u8, key_code: u8, modifiers: u16) {
    let start = dialog.sel_start.min(dialog.name.len());
    let end = dialog.sel_end.min(dialog.name.len()).max(start);
    if modifiers & 0x0100 != 0 {
        if character.eq_ignore_ascii_case(&b'a') {
            dialog.sel_start = 0;
            dialog.sel_end = dialog.name.len();
        }
    } else if character == 0x08 || key_code == 0x33 {
        if start < end {
            dialog.name.drain(start..end);
            dialog.sel_start = start;
        } else if start > 0 {
            dialog.name.remove(start - 1);
            dialog.sel_start = start - 1;
        }
        dialog.sel_end = dialog.sel_start;
    } else if key_code == 0x7b || character == 0x1c {
        dialog.sel_start = start.saturating_sub(usize::from(start == end));
        dialog.sel_end = dialog.sel_start;
    } else if key_code == 0x7c || character == 0x1d {
        dialog.sel_end = (end + usize::from(start == end)).min(dialog.name.len());
        dialog.sel_start = dialog.sel_end;
    } else if (0x20..=0xfe).contains(&character)
        && character != b':'
        && dialog.name.len() - (end - start) < 31
    {
        dialog.name.splice(start..end, [character]);
        dialog.sel_start = start + 1;
        dialog.sel_end = start + 1;
    }
}

fn list_rows_rect(dialog: &PpcNavigationDialogState) -> (i16, i16, i16, i16) {
    (HEADER_BOTTOM, LIST_LEFT + 1, dialog.list_bottom() - 1, SCROLL_LEFT)
}

fn scroll_rect(dialog: &PpcNavigationDialogState) -> (i16, i16, i16, i16) {
    (HEADER_BOTTOM, SCROLL_LEFT, dialog.list_bottom() - 1, LIST_RIGHT)
}

const POPUP_MENU_ROW_HEIGHT: i16 = 18;

fn popup_menu_rect(dialog: &PpcNavigationDialogState) -> (i16, i16, i16, i16) {
    let rows = dialog.folder_chain.len().min(10) as i16;
    (
        POPUP.2 - 1,
        POPUP.1,
        POPUP.2 + 1 + rows * POPUP_MENU_ROW_HEIGHT,
        POPUP.3,
    )
}

/// Draws a frame-relative picture: `Pen` with the frame's origin added.
struct Frame<'a> {
    pen: Pen<'a>,
    top: i16,
    left: i16,
}

impl Frame<'_> {
    fn rect(&mut self, (t, l, b, r): (i16, i16, i16, i16), shade: Shade) {
        self.pen.rect((self.top + t, self.left + l, self.top + b, self.left + r), shade);
    }
    fn dot(&mut self, h: i16, v: i16, shade: Shade) {
        self.pen.dot(self.left + h, self.top + v, shade);
    }
    fn hline(&mut self, v: i16, h0: i16, h1: i16, shade: Shade) {
        self.pen.hline(self.top + v, self.left + h0, self.left + h1, shade);
    }
    fn vline(&mut self, h: i16, v0: i16, v1: i16, shade: Shade) {
        self.pen.vline(self.left + h, self.top + v0, self.top + v1, shade);
    }
    fn text(&mut self, (h, v): (i16, i16), bytes: &[u8], color: PpcRgbColor, font: i16, size: i16) {
        self.pen.text_font((self.left + h, self.top + v), bytes, color, font, size);
    }
    fn push_button(&mut self, (t, l, b, r): (i16, i16, i16, i16), title: &[u8], default: bool, pressed: bool) {
        let global = (self.top + t, self.left + l, self.top + b, self.left + r);
        draw_push_button(&mut self.pen, global, title, default, pressed);
    }
}

const BLACK_RGB: PpcRgbColor = PPC_RGB_BLACK;
const GREY_TEXT: PpcRgbColor = PpcRgbColor {
    red: 0x8585,
    green: 0x8585,
    blue: 0x8585,
};

fn draw(memory: &mut PpcSectionMem, gworlds: &[PpcGWorldRecord], dialog: &PpcNavigationDialogState) {
    ppc_with_open_window_manager_port(memory, |memory| {
        let mut frame = Frame {
            pen: Pen::new(memory, gworlds, PPC_MAIN_GWORLD),
            top: dialog.bounds.0,
            left: dialog.bounds.1,
        };
        draw_window(&mut frame, dialog);
        draw_popup(&mut frame, dialog);
        for (index, left) in SHORTCUT_BUTTONS.into_iter().enumerate() {
            draw_shortcut_button(&mut frame, left, index);
        }
        draw_list(&mut frame, dialog);
        if dialog.get {
            frame.push_button(SHOW_PREVIEW_BUTTON, b"Show Preview", false, false);
        } else {
            draw_name_row(&mut frame, dialog);
        }
        let h = dialog.frame_height();
        frame.hline(h - 44, 3, FRAME_WIDTH - 4, grey(161));
        frame.hline(h - 43, 4, FRAME_WIDTH - 4, grey(WHITE));
        draw_help_button(&mut frame, dialog.buttons_top());
        frame.push_button(
            dialog.cancel_rect(),
            b"Cancel",
            false,
            dialog.pressed == Some(Press::Cancel),
        );
        let label: &[u8] = if dialog.get { b"Open" } else { b"Save" };
        draw_default_button(&mut frame, dialog, label);
        draw_grow_box(&mut frame, h);
        if dialog.popup_open {
            draw_popup_menu(&mut frame, dialog);
        }
    });
}

/// The frame, its shadow, the striped title bar with the title centred in
/// it, and the content's bevel.
fn draw_window(frame: &mut Frame<'_>, dialog: &PpcNavigationDialogState) {
    let (w, h) = (FRAME_WIDTH, dialog.frame_height());
    let right = w - 1;
    // Black edge and shadow.
    frame.hline(0, 0, right, grey(BLACK));
    frame.hline(h - 1, 0, right, grey(BLACK));
    frame.vline(0, 0, h - 1, grey(BLACK));
    frame.vline(right, 0, h - 1, grey(BLACK));
    frame.vline(w, 2, h, grey(BLACK));
    frame.hline(h, 2, w, grey(BLACK));
    // Title bar.
    frame.rect((1, 1, 21, right), grey(215));
    frame.hline(1, 1, right - 2, grey(WHITE));
    frame.vline(1, 1, 19, grey(WHITE));
    frame.vline(right - 1, 2, 20, grey(178));
    frame.hline(20, 2, right - 1, grey(178));
    frame.hline(21, 0, right, grey(BLACK));
    let advance = ppc_text_bytes_advance_for_font(&dialog.title, CHARCOAL, 12);
    let text_left = (w - advance) / 2;
    for row in 0..6 {
        let v = 4 + row * 2;
        let (left_end, right_start) = (text_left - 5, text_left + advance + 3);
        frame.hline(v, 5, left_end, grey(WHITE));
        frame.hline(v, right_start, w - 7, grey(WHITE));
        frame.hline(v + 1, 6, left_end + 1, grey(148));
        frame.hline(v + 1, right_start + 1, w - 6, grey(148));
    }
    frame.text((text_left, 15), &dialog.title, BLACK_RGB, CHARCOAL, 12);
    // Content and its bevel.
    frame.rect((22, 1, h - 1, right), grey(BACKGROUND));
    frame.hline(23, 2, right - 3, grey(WHITE));
    frame.vline(2, 23, h - 4, grey(WHITE));
    frame.vline(right - 2, 24, h - 3, grey(178));
    frame.vline(right - 1, 23, h - 2, grey(118));
    frame.hline(h - 3, 3, right - 2, grey(178));
    frame.hline(h - 2, 2, right - 1, grey(118));
}

/// The location pop-up: a rounded black edge, a white light above and to
/// the left, a shaded edge below and to the right, the current folder's
/// icon and name, and up and down arrows in a part of their own.
fn draw_popup(frame: &mut Frame<'_>, dialog: &PpcNavigationDialogState) {
    let (t, l, b, r) = POPUP;
    let (b, r) = (b - 1, r - 1);
    frame.rect((t + 1, l + 1, b, r), grey(BACKGROUND));
    frame.hline(t, l + 3, r - 3, grey(BLACK));
    frame.dot(l + 2, t, grey(60));
    frame.dot(r - 2, t, grey(60));
    frame.dot(l + 1, t + 1, grey(BLACK));
    frame.hline(t + 1, l + 2, r - 23, grey(WHITE));
    frame.dot(r - 2, t + 1, grey(203));
    frame.dot(r - 1, t + 1, grey(BLACK));
    frame.dot(l, t + 2, grey(60));
    frame.dot(r - 1, t + 2, grey(191));
    frame.dot(r, t + 2, grey(60));
    frame.vline(l, t + 3, b - 3, grey(BLACK));
    frame.vline(r, t + 3, b - 3, grey(BLACK));
    frame.vline(l + 1, t + 2, b - 2, grey(WHITE));
    frame.vline(r - 2, t + 3, b - 3, grey(191));
    frame.vline(r - 1, t + 3, b - 3, grey(148));
    frame.dot(l, b - 2, grey(60));
    frame.hline(b - 2, r - 2, r - 1, grey(148));
    frame.dot(r, b - 2, grey(60));
    frame.dot(l + 1, b - 1, grey(BLACK));
    frame.hline(b - 1, l + 2, r - 22, grey(191));
    frame.dot(r - 21, b - 1, grey(203));
    frame.hline(b - 1, r - 20, r - 2, grey(148));
    frame.dot(r - 1, b - 1, grey(BLACK));
    frame.dot(l + 2, b, grey(60));
    frame.hline(b, l + 3, r - 3, grey(BLACK));
    frame.dot(r - 2, b, grey(60));
    // The arrows' part.
    let part = r - 21;
    frame.vline(part, t + 2, b - 2, grey(191));
    frame.vline(part + 2, t + 2, b - 3, grey(WHITE));
    frame.hline(t + 2, part + 2, r - 3, grey(WHITE));
    frame.hline(b - 2, part + 3, r - 3, grey(191));
    let centre = part + 10;
    for (row, half) in [0, 1, 2, 3].into_iter().enumerate() {
        frame.hline(t + 5 + row as i16, centre - half, centre + half, grey(BLACK));
        frame.hline(t + 14 - row as i16, centre - half, centre + half, grey(BLACK));
    }
    // The folder's icon and name.
    let name = dialog
        .folder_chain
        .first()
        .map_or(&b"Desktop"[..], |(_, name)| name.as_slice());
    draw_folder_icon(frame, (l + 7, t + 3), false);
    frame.text((l + 24, t + 14), name, BLACK_RGB, CHARCOAL, 12);
}

fn draw_popup_menu(frame: &mut Frame<'_>, dialog: &PpcNavigationDialogState) {
    let (t, l, b, r) = popup_menu_rect(dialog);
    frame.rect((t, l, b, r), grey(BACKGROUND));
    frame.hline(t, l, r - 1, grey(BLACK));
    frame.hline(b - 1, l, r - 1, grey(BLACK));
    frame.vline(l, t, b - 1, grey(BLACK));
    frame.vline(r - 1, t, b - 1, grey(BLACK));
    frame.vline(r, t + 2, b, grey(BLACK));
    frame.hline(b, l + 2, r, grey(BLACK));
    for (row, (_, name)) in dialog.folder_chain.iter().take(10).enumerate() {
        let top = t + 1 + row as i16 * POPUP_MENU_ROW_HEIGHT;
        draw_folder_icon(frame, (l + 7, top + 1), false);
        frame.text((l + 24, top + 14), name, BLACK_RGB, CHARCOAL, 12);
    }
}

/// A bevel button 36 by 26 with a menu arrow, holding one of the three
/// shortcut icons: Shortcuts, Favorites, and Recent, which is dimmed.
fn draw_shortcut_button(frame: &mut Frame<'_>, l: i16, index: usize) {
    let t = SHORTCUT_TOP;
    if index == 2 {
        frame.vline(l, t, t + 24, grey(191));
        frame.vline(l + 35, t, t + 25, grey(178));
        frame.hline(t + 25, l + 1, l + 35, grey(178));
        draw_recent_icon(frame, (l + 10, t + 5));
    } else {
        draw_bevel(frame, (t, l, t + 26, l + 36));
        if index == 0 {
            draw_shortcuts_icon(frame, (l + 10, t + 5));
        } else {
            draw_favorites_icon(frame, (l + 10, t + 5));
        }
    }
    let shade = if index == 2 { grey(178) } else { grey(BLACK) };
    frame.hline(t + 19, l + 27, l + 31, shade);
    frame.hline(t + 20, l + 28, l + 30, shade);
    frame.dot(l + 29, t + 21, shade);
}

/// Mac OS 8.5's bevel button: 133 at the left, a white light inside it,
/// and 178, 148 and 85 down the right and along the bottom.
fn draw_bevel(frame: &mut Frame<'_>, (t, l, b, r): (i16, i16, i16, i16)) {
    let (b, r) = (b - 1, r - 1);
    frame.rect((t, l + 1, b, r), grey(215));
    frame.vline(l, t, b - 1, grey(133));
    frame.hline(t, l + 1, r - 2, grey(215));
    frame.dot(r - 1, t, grey(191));
    frame.hline(t + 1, l + 2, r - 3, grey(WHITE));
    frame.vline(l + 2, t + 1, b - 3, grey(WHITE));
    frame.vline(r - 2, t + 2, b - 2, grey(178));
    frame.hline(b - 2, l + 3, r - 2, grey(178));
    frame.vline(r - 1, t + 1, b - 1, grey(148));
    frame.hline(b - 1, l + 2, r - 1, grey(148));
    frame.dot(l + 1, b - 1, grey(191));
    frame.vline(r, t, b, grey(85));
    frame.hline(b, l + 1, r, grey(85));
    frame.dot(l, b, grey(118));
}

fn draw_help_button(frame: &mut Frame<'_>, top: i16) {
    let (t, l) = (top, HELP_LEFT);
    draw_bevel(frame, (t, l, t + 20, l + 21));
    // A white disc with a black ring and a question mark.
    let (cv, ch) = (t + 9, l + 10);
    for dv in -6i16..=6 {
        for dh in -6i16..=6 {
            let d = dv * dv + dh * dh;
            if d <= 30 {
                frame.dot(ch + dh, cv + dv, grey(WHITE));
            } else if d <= 42 {
                frame.dot(ch + dh, cv + dv, grey(BLACK));
            }
        }
    }
    frame.text((ch - 3, cv + 5), b"?", BLACK_RGB, CHARCOAL, 12);
}

fn draw_default_button(frame: &mut Frame<'_>, dialog: &PpcNavigationDialogState, label: &[u8]) {
    let (t, l, b, r) = dialog.default_rect();
    let global = (frame.top + t, frame.left + l, frame.top + b, frame.left + r);
    draw_default_ring(&mut frame.pen, global);
    draw_push_button(
        &mut frame.pen,
        global,
        label,
        false,
        dialog.pressed == Some(Press::Default),
    );
}

/// The list: its header, with the sorted column darker and the sort
/// direction button at the right, its rows, and a scroll bar.
fn draw_list(frame: &mut Frame<'_>, dialog: &PpcNavigationDialogState) {
    let bottom = dialog.list_bottom() - 1;
    let (l, r) = (LIST_LEFT, LIST_RIGHT - 1);
    // Header.
    frame.hline(LIST_TOP, l, NAME_COLUMN_RIGHT - 2, grey(37));
    frame.dot(NAME_COLUMN_RIGHT - 1, LIST_TOP, grey(60));
    frame.hline(LIST_TOP, NAME_COLUMN_RIGHT, r - 1, grey(133));
    frame.dot(r, LIST_TOP, grey(118));
    frame.vline(l, LIST_TOP, HEADER_BOTTOM - 2, grey(37));
    frame.dot(l, HEADER_BOTTOM - 1, grey(60));
    // Name, the sorted column.
    let name_right = NAME_COLUMN_RIGHT - 1;
    frame.rect((LIST_TOP + 1, l + 1, HEADER_BOTTOM - 2, name_right - 1), grey(161));
    frame.hline(LIST_TOP + 1, l + 1, name_right - 3, grey(118));
    frame.vline(l + 1, LIST_TOP + 1, HEADER_BOTTOM - 3, grey(118));
    frame.dot(name_right - 2, LIST_TOP + 1, grey(148));
    frame.vline(name_right - 1, LIST_TOP + 2, HEADER_BOTTOM - 2, grey(191));
    frame.vline(name_right, LIST_TOP + 1, HEADER_BOTTOM - 2, grey(102));
    frame.dot(l + 1, HEADER_BOTTOM - 2, grey(148));
    frame.hline(HEADER_BOTTOM - 2, l + 2, name_right - 1, grey(191));
    frame.hline(HEADER_BOTTOM - 1, l + 1, name_right, grey(102));
    frame.text((l + 42, LIST_TOP + 14), b"Name", BLACK_RGB, GENEVA, 10);
    // Date Modified, and the sort direction button.
    for (left, right) in [(NAME_COLUMN_RIGHT, SCROLL_LEFT - 1), (SCROLL_LEFT - 1, r)] {
        frame.rect((LIST_TOP + 1, left + 1, HEADER_BOTTOM - 2, right - 1), grey(215));
        frame.vline(left, LIST_TOP + 1, HEADER_BOTTOM - 2, grey(133));
        frame.hline(LIST_TOP + 1, left + 1, right - 2, grey(WHITE));
        frame.vline(left + 1, LIST_TOP + 1, HEADER_BOTTOM - 3, grey(WHITE));
        frame.vline(right - 1, LIST_TOP + 1, HEADER_BOTTOM - 2, grey(161));
        frame.hline(HEADER_BOTTOM - 2, left + 2, right - 1, grey(161));
        frame.vline(right, LIST_TOP + 1, HEADER_BOTTOM - 1, grey(85));
    }
    frame.hline(HEADER_BOTTOM - 1, NAME_COLUMN_RIGHT, SCROLL_LEFT - 1, grey(85));
    frame.hline(HEADER_BOTTOM - 1, SCROLL_LEFT, r, grey(BLACK));
    frame.text(
        (NAME_COLUMN_RIGHT + 8, LIST_TOP + 14),
        b"Date Modified",
        BLACK_RGB,
        GENEVA,
        10,
    );
    // The sort direction: a stepped triangle, each dark step lit below.
    let button = SCROLL_LEFT - 1;
    for step in 0..4i16 {
        let v = LIST_TOP + 6 + step * 2;
        let left = button + 8 - step;
        frame.dot(left, v, grey(102));
        if step == 0 {
            frame.dot(left + 1, v, grey(118));
        } else {
            frame.dot(left + 1, v, grey(60));
            frame.hline(v, left + 2, left + 2 * step, grey(if step == 1 { 60 } else { 37 }));
            frame.dot(left + 2 * step + 1, v, grey(BLACK));
        }
        if step < 3 {
            frame.hline(v + 1, left, left + 2 * step + 1, grey(203));
        }
    }
    // Body: the name column in the window's grey, the date column lighter.
    frame.rect((HEADER_BOTTOM, l + 1, bottom, NAME_COLUMN_RIGHT), grey(BACKGROUND));
    frame.rect((HEADER_BOTTOM, NAME_COLUMN_RIGHT, bottom, SCROLL_LEFT), grey(241));
    frame.vline(l, HEADER_BOTTOM, bottom, grey(BLACK));
    frame.hline(bottom, l, r, grey(BLACK));
    let rows = dialog.visible_rows();
    for row in 0..rows {
        let top = HEADER_BOTTOM + row as i16 * ROW_HEIGHT;
        frame.hline(top + ROW_HEIGHT - 1, l + 1, SCROLL_LEFT - 1, grey(WHITE));
        let index = dialog.first_row + row;
        let Some(entry) = dialog.entries.get(index) else {
            continue;
        };
        let selected = dialog.selected == Some(index);
        let text_left = if dialog.get { 55 } else { 57 };
        let mut color = BLACK_RGB;
        if selected {
            let width = ppc_text_bytes_advance_for_font(&entry.name, GENEVA, 10);
            frame.rect(
                (top + 3, text_left - 5, top + 15, text_left + width + 4),
                grey(BLACK),
            );
            color = PPC_RGB_WHITE;
        } else if !dialog.get && !entry.is_directory {
            color = GREY_TEXT;
        }
        if dialog.get && entry.is_directory {
            draw_disclosure_triangle(frame, (23, top + 4));
        }
        if entry.is_directory {
            draw_folder_icon(frame, (35, top + 3), false);
        } else {
            draw_document_icon(frame, (37, top + 2));
        }
        frame.text((text_left, top + 14), &entry.name, color, GENEVA, 10);
    }
    draw_scroll_bar(frame, dialog, bottom);
}

fn draw_scroll_bar(frame: &mut Frame<'_>, dialog: &PpcNavigationDialogState, bottom: i16) {
    let (l, r) = (SCROLL_LEFT, LIST_RIGHT - 1);
    frame.vline(l, HEADER_BOTTOM, bottom, grey(BLACK));
    frame.vline(r, HEADER_BOTTOM, bottom, grey(BLACK));
    frame.rect((HEADER_BOTTOM, l + 1, bottom, r), grey(241));
    let scrolls = dialog.entries.len() > dialog.visible_rows();
    let arrow = if scrolls { grey(BLACK) } else { grey(161) };
    frame.hline(HEADER_BOTTOM + 14, l + 1, r - 1, grey(118));
    frame.hline(bottom - 15, l + 1, r - 1, grey(118));
    let mid = l + 7;
    for row in 0..4i16 {
        frame.hline(HEADER_BOTTOM + 5 + row, mid - row, mid + 1 + row, arrow);
        frame.hline(bottom - 9 + row, mid - 3 + row, mid + 4 - row, arrow);
    }
    if scrolls {
        // The thumb, in proportion to where the first row is.
        let track = (HEADER_BOTTOM + 15, bottom - 15);
        let span = (track.1 - track.0 - 16).max(0);
        let last = dialog.entries.len() - dialog.visible_rows();
        let top = track.0 + (span as usize * dialog.first_row / last.max(1)) as i16;
        draw_bevel(frame, (top, l + 1, top + 16, r));
    }
}

/// Each NavGetFile folder's disclosure triangle, pointing right: blue,
/// lighter at the top, edged in black and shadowed below.
fn draw_disclosure_triangle(frame: &mut Frame<'_>, (h, v): (i16, i16)) {
    for row in 0..11i16 {
        let width = if row <= 5 { row } else { 10 - row };
        frame.dot(h, v + row, grey(BLACK));
        for column in 1..width {
            let shade = if column == 1 {
                rgb(203, 230, 252)
            } else if row >= 6 && column == width - 1 {
                rgb(133, 133, 161)
            } else {
                rgb(181, 221, 252)
            };
            frame.dot(h + column, v + row, shade);
        }
        if width > 0 {
            frame.dot(h + width, v + row, grey(BLACK));
        }
        if row >= 6 {
            frame.dot(h + width + 1, v + row, grey(161));
            frame.dot(h + width + 2, v + row, grey(203));
        }
    }
    frame.dot(h + 1, v + 11, grey(203));
}

/// A small folder in Mac OS 8.5's blue-violet.
fn draw_folder_icon(frame: &mut Frame<'_>, (h, v): (i16, i16), _open: bool) {
    let (edge, fill, light) = (rgb(80, 80, 102), rgb(181, 181, 252), rgb(203, 203, 252));
    frame.hline(v, h + 1, h + 5, edge);
    frame.dot(h, v + 1, edge);
    frame.dot(h + 6, v + 1, edge);
    frame.hline(v + 1, h + 7, h + 13, edge);
    frame.vline(h, v + 2, v + 11, edge);
    frame.vline(h + 14, v + 2, v + 11, edge);
    frame.hline(v + 12, h + 1, h + 13, edge);
    frame.rect((v + 2, h + 1, v + 12, h + 14), fill);
    frame.hline(v + 3, h + 1, h + 13, edge);
    frame.hline(v + 4, h + 1, h + 13, light);
    frame.hline(v + 1, h + 1, h + 5, light);
}

/// A small document with its corner folded.
fn draw_document_icon(frame: &mut Frame<'_>, (h, v): (i16, i16)) {
    frame.rect((v + 1, h + 1, v + 15, h + 11), grey(WHITE));
    frame.hline(v, h, h + 7, grey(BLACK));
    frame.vline(h, v, v + 15, grey(BLACK));
    frame.hline(v + 15, h, h + 11, grey(BLACK));
    frame.vline(h + 11, v + 4, v + 15, grey(BLACK));
    for step in 0..4 {
        frame.dot(h + 8 + step, v + step + 1, grey(BLACK));
    }
    frame.vline(h + 7, v, v + 4, grey(BLACK));
    frame.hline(v + 4, h + 7, h + 11, grey(BLACK));
}

/// Shortcuts: a hand pointing right.
fn draw_shortcuts_icon(frame: &mut Frame<'_>, (h, v): (i16, i16)) {
    let (skin, shade, edge) = (rgb(252, 218, 197), rgb(230, 181, 129), rgb(66, 0, 0));
    frame.rect((v + 4, h + 4, v + 12, h + 10), skin);
    frame.rect((v + 4, h + 10, v + 6, h + 16), skin);
    frame.hline(v + 3, h + 4, h + 15, edge);
    frame.hline(v + 6, h + 10, h + 15, edge);
    frame.vline(h + 16, v + 4, v + 5, edge);
    frame.vline(h + 3, v + 4, v + 11, edge);
    frame.hline(v + 12, h + 4, h + 9, edge);
    frame.vline(h + 10, v + 7, v + 11, edge);
    frame.hline(v + 8, h + 7, h + 9, shade);
    frame.hline(v + 10, h + 7, h + 9, shade);
    frame.rect((v + 5, h, v + 12, h + 3), grey(BLACK));
}

/// Favorites: two upright books, magenta and blue.
fn draw_favorites_icon(frame: &mut Frame<'_>, (h, v): (i16, i16)) {
    frame.rect((v, h + 2, v + 15, h + 7), rgb(252, 98, 252));
    frame.vline(h + 2, v, v + 14, rgb(215, 0, 218));
    frame.vline(h + 6, v, v + 14, rgb(140, 0, 144));
    frame.rect((v + 1, h + 7, v + 15, h + 12), rgb(181, 221, 252));
    frame.vline(h + 7, v + 1, v + 14, rgb(203, 230, 252));
    frame.vline(h + 11, v + 1, v + 14, rgb(0, 114, 181));
    frame.hline(v + 15, h + 2, h + 12, rgb(0, 0, 130));
}

/// Recent: a clock face, dimmed.
fn draw_recent_icon(frame: &mut Frame<'_>, (h, v): (i16, i16)) {
    let (cv, ch) = (v + 7, h + 7);
    for dv in -7i16..=7 {
        for dh in -7i16..=7 {
            let d = dv * dv + dh * dh;
            if d <= 40 {
                frame.dot(ch + dh, cv + dv, rgb(252, 247, 209));
            } else if d <= 56 {
                frame.dot(ch + dh, cv + dv, grey(203));
            }
        }
    }
    frame.vline(ch, cv - 5, cv, grey(148));
    for step in 1..4 {
        frame.dot(ch + step, cv + step, grey(148));
    }
}

/// The name row of NavPutFile: "Name:", the field with its focus ring and
/// the text selected in pale blue, and the New folder button.
fn draw_name_row(frame: &mut Frame<'_>, dialog: &PpcNavigationDialogState) {
    frame.text((14, 237), b"Name:", BLACK_RGB, CHARCOAL, 12);
    let (t, l, b, r) = NAME_FIELD;
    let (b, r) = (b - 1, r - 1);
    let ring = rgb(133, 133, 161);
    frame.hline(t - 2, l - 1, r + 1, ring);
    frame.hline(t - 1, l - 2, r + 2, ring);
    frame.hline(b + 1, l - 2, r + 2, ring);
    frame.hline(b + 2, l - 1, r + 1, ring);
    frame.vline(l - 2, t, b, ring);
    frame.vline(l - 1, t, b, ring);
    frame.vline(r + 1, t, b, ring);
    frame.vline(r + 2, t, b, ring);
    frame.hline(t, l, r, grey(BLACK));
    frame.hline(b, l, r, grey(BLACK));
    frame.vline(l, t, b, grey(BLACK));
    frame.vline(r, t, b, grey(BLACK));
    frame.rect((t + 1, l + 1, b, r), grey(WHITE));
    let start = dialog.sel_start.min(dialog.name.len());
    let end = dialog.sel_end.min(dialog.name.len()).max(start);
    let text_left = l + 4;
    let advance = |bytes: &[u8]| ppc_text_bytes_advance_for_font(bytes, CHARCOAL, 12);
    if start < end {
        let (from, to) = if start == 0 && end == dialog.name.len() {
            (l + 3, r - 3)
        } else {
            (
                text_left + advance(&dialog.name[..start]),
                text_left + advance(&dialog.name[..end]),
            )
        };
        frame.rect((t + 3, from, b - 2, to + 1), rgb(203, 230, 252));
    }
    frame.text((text_left, t + 15), &dialog.name, BLACK_RGB, CHARCOAL, 12);
    if start == end {
        let caret = text_left + advance(&dialog.name[..start]) - 1;
        frame.vline(caret, t + 4, b - 4, grey(BLACK));
    }
    // New, with a small folder after its title.
    frame.push_button(NEW_BUTTON, b"", false, false);
    frame.text((NEW_BUTTON.1 + 16, NEW_BUTTON.0 + 14), b"New", BLACK_RGB, CHARCOAL, 12);
    draw_folder_icon(frame, (NEW_BUTTON.1 + 47, NEW_BUTTON.0 + 3), false);
}

/// The size box's three ridges at the bottom right, white with a shadow
/// below and to the right of each white pixel, relative to the frame's
/// bottom right corner.
const GROW_WHITE: [(i16, i16); 24] = [
    (-5, -16), (-4, -16), (-6, -15), (-7, -14), (-8, -13), (-9, -12), (-5, -12), (-4, -12),
    (-10, -11), (-6, -11), (-11, -10), (-7, -10), (-12, -9), (-8, -9), (-13, -8), (-9, -8),
    (-5, -8), (-4, -8), (-14, -7), (-10, -7), (-6, -7), (-15, -6), (-11, -6), (-7, -6),
];

fn draw_grow_box(frame: &mut Frame<'_>, h: i16) {
    let corner = (FRAME_WIDTH - 1, h);
    for &(dh, dv) in &GROW_WHITE {
        if !GROW_WHITE.contains(&(dh + 1, dv + 1)) && dv + 1 < -5 {
            frame.dot(corner.0 + dh + 1, corner.1 + dv + 1, grey(161));
        }
    }
    for &(dh, dv) in &GROW_WHITE {
        frame.dot(corner.0 + dh, corner.1 + dv, grey(WHITE));
    }
    for dh in [-15, -11, -7] {
        frame.dot(corner.0 + dh, corner.1 - 5, grey(203));
        frame.dot(corner.0 + dh + 1, corner.1 - 5, grey(161));
    }
}
