//! The Appearance Manager's Control Manager calls on the PowerPC path.
//!
//! Ported from the 68K `ControlDispatch` ($AA73) arm in `trap/control.rs`,
//! which gives the frames and says what is modelled and what is not. In
//! brief: the embedding hierarchy is real (CreateRootControl makes an
//! invisible root user pane, EmbedControl records a container, and
//! Activate/DeactivateControl walk what is embedded); keyboard focus is not
//! modelled, so GetKeyboardFocus answers nil and HandleControlKey
//! kControlNoPart; Get/SetControlData keep kControlFontStyleTag and refuse
//! every other tag with errDataNotSupported. Cythera's Preferences window is
//! built this way when Gestalt('appr') reports the Appearance Manager.
use super::*;
use std::cell::RefCell;
use std::collections::HashMap;

/// The Appearance calls bound here (Controls.h, Universal Interfaces 3.3.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PpcAppearanceControlOperation {
    CreateRootControl,
    EmbedControl,
    ActivateControl,
    DeactivateControl,
    FindControlUnderMouse,
    HandleControlClick,
    HandleControlKey,
    IdleControls,
    GetKeyboardFocus,
    SetControlData,
    GetControlData,
    SetThemeWindowBackground,
}

pub(super) fn ppc_appearance_control_symbol_op(symbol: &str) -> Option<PpcAppearanceControlOperation> {
    use PpcAppearanceControlOperation as Op;
    Some(match symbol {
        "CreateRootControl" => Op::CreateRootControl,
        "EmbedControl" => Op::EmbedControl,
        "ActivateControl" => Op::ActivateControl,
        "DeactivateControl" => Op::DeactivateControl,
        "FindControlUnderMouse" => Op::FindControlUnderMouse,
        "HandleControlClick" => Op::HandleControlClick,
        "HandleControlKey" => Op::HandleControlKey,
        "IdleControls" => Op::IdleControls,
        "GetKeyboardFocus" => Op::GetKeyboardFocus,
        "SetControlData" => Op::SetControlData,
        "GetControlData" => Op::GetControlData,
        "SetThemeWindowBackground" => Op::SetThemeWindowBackground,
        _ => return None,
    })
}

// Control Manager error codes, MacErrors.h.
const ERR_DATA_NOT_SUPPORTED: i16 = -30581;
const ERR_ROOT_ALREADY_EXISTS: i16 = -30587;
const ERR_DATA_SIZE_MISMATCH: i16 = -30591;
const ERR_CANT_EMBED_INTO_SELF: i16 = -30594;
const ERR_CANT_EMBED_ROOT: i16 = -30595;
const CONTROL_HANDLE_INVALID_ERR: i16 = -30599;
/// kControlUserPaneProc: the root is an invisible user pane, so drawing and
/// hit-testing skip it without knowing it is special.
const CONTROL_USER_PANE_PROC: i16 = 256;
/// kControlFontStyleTag and the size of its ControlFontStyleRec.
const CONTROL_FONT_STYLE_TAG: [u8; 4] = *b"font";
const CONTROL_FONT_STYLE_SIZE: u32 = 24;

/// kControlSliderProc (48) and its variants: bit 0 live feedback, bit 1 tick
/// marks, bit 2 reverse direction, bit 3 non-directional.
pub(super) fn ppc_is_slider_proc_id(proc_id: i16) -> bool {
    (48..=63).contains(&proc_id)
}

/// Appearance controls that show something and take no click: group boxes,
/// static text, pictures and icons. FindControl walks past them to what lies
/// underneath, which in Cythera's Preferences is a slider whose end label
/// overlaps it.
pub(super) fn ppc_is_passive_appearance_proc_id(proc_id: i16) -> bool {
    matches!(proc_id, 160 | 161 | 288 | 304 | 305 | 320 | 321)
}

/// Thumb length along a slider's track, in pixels.
pub(super) const PPC_SLIDER_THUMB_SIZE: i16 = 12;

thread_local! {
    /// Window -> its root control.
    static ROOTS: RefCell<HashMap<u32, u32>> = RefCell::new(HashMap::new());
    /// Control -> the container it is embedded in.
    static PARENTS: RefCell<HashMap<u32, u32>> = RefCell::new(HashMap::new());
    /// (control, part, tag) -> the data set for it.
    static TAGGED: RefCell<HashMap<(u32, i16, [u8; 4]), Vec<u8>>> = RefCell::new(HashMap::new());
}

pub(super) fn ppc_window_root_control(window: u32) -> Option<u32> {
    ROOTS.with(|roots| roots.borrow().get(&window).copied())
}

/// Forget a disposed control's place in the hierarchy.
pub(super) fn ppc_forget_appearance_control(handle: u32) {
    ROOTS.with(|roots| roots.borrow_mut().retain(|_, root| *root != handle));
    PARENTS.with(|parents| parents.borrow_mut().retain(|control, _| *control != handle));
    TAGGED.with(|tagged| tagged.borrow_mut().retain(|key, _| key.0 != handle));
}

/// `control` and everything embedded in it, at any depth.
fn ppc_control_family(control: u32) -> Vec<u32> {
    PARENTS.with(|parents| {
        let parents = parents.borrow();
        let mut family = vec![control];
        let mut index = 0;
        while index < family.len() {
            let container = family[index];
            for (child, parent) in parents.iter() {
                if *parent == container && !family.contains(child) {
                    family.push(*child);
                }
            }
            index += 1;
        }
        family
    })
}

/// CreateRootControl(inWindow, VAR outControl): OSErr. On
/// errRootAlreadyExists the existing root is still written back, because a
/// caller that ignores the OSErr would otherwise keep whatever was in its
/// local as a control.
#[allow(clippy::too_many_arguments)]
pub(super) fn ppc_create_root_control(
    cpu: &PpcCpu,
    allocator: Option<&mut PpcProcessAllocatorView<'_>>,
    memory: &mut PpcSectionMem,
    heap_cursor: &mut u32,
    heap_limit: u32,
    last_mem_error: &mut i16,
    handles: &mut Vec<PpcHandleRecord>,
    controls: &mut Vec<PpcControlRecord>,
) -> i16 {
    let (window, out_control) = (cpu.gpr[3], cpu.gpr[4]);
    let (root, err) = if window == 0 {
        (0, PPC_PARAM_ERR)
    } else if let Some(existing) = ppc_window_root_control(window) {
        (existing, ERR_ROOT_ALREADY_EXISTS)
    } else {
        let bounds = ppc_read_rect(memory, window.wrapping_add(16)).unwrap_or((0, 0, 0, 0));
        let handle = ppc_new_control_record_values(
            allocator,
            memory,
            heap_cursor,
            heap_limit,
            last_mem_error,
            handles,
            controls,
            window,
            bounds,
            &[],
            false,
            0,
            0,
            1,
            CONTROL_USER_PANE_PROC,
            0,
        );
        if handle != 0 {
            ROOTS.with(|roots| roots.borrow_mut().insert(window, handle));
        }
        (handle, if handle != 0 { PPC_NO_ERR } else { PPC_MEM_FULL_ERR })
    };
    if out_control != 0 {
        let _ = memory.write_u32_be(out_control, root);
    }
    err
}

/// EmbedControl(inControl, inContainer): OSErr. Only the two structural
/// errors are enforced, as on 68K.
pub(super) fn ppc_embed_control(memory: &mut PpcSectionMem, control: u32, container: u32) -> i16 {
    let control_ptr = ppc_control_ptr(memory, control);
    let container_ptr = ppc_control_ptr(memory, container);
    let control_is_root = control_ptr
        .and_then(|ptr| memory.read_u32_be(ptr + PPC_CONTROL_OWNER_OFFSET))
        .and_then(ppc_window_root_control)
        == Some(control);
    if control_ptr.is_none() || container_ptr.is_none() {
        CONTROL_HANDLE_INVALID_ERR
    } else if control == container {
        ERR_CANT_EMBED_INTO_SELF
    } else if control_is_root {
        ERR_CANT_EMBED_ROOT
    } else {
        PARENTS.with(|parents| parents.borrow_mut().insert(control, container));
        PPC_NO_ERR
    }
}

/// ActivateControl / DeactivateControl: the control and everything
/// embedded in it become active (hilite 0) or inactive (255). Returns the
/// controls to redraw, or the error.
pub(super) fn ppc_set_control_family_active(
    memory: &mut PpcSectionMem,
    control: u32,
    active: bool,
) -> Result<Vec<u32>, i16> {
    if ppc_control_ptr(memory, control).is_none() {
        return Err(CONTROL_HANDLE_INVALID_ERR);
    }
    let family = ppc_control_family(control);
    for handle in &family {
        if let Some(ptr) = ppc_control_ptr(memory, *handle) {
            let _ = memory.write_u8(ptr + PPC_CONTROL_HILITE_OFFSET, if active { 0 } else { 255 });
        }
    }
    Ok(family)
}

/// SetControlData(inControl, inPart, inTagName, inSize, inData): OSErr.
pub(super) fn ppc_set_control_data(memory: &mut PpcSectionMem, cpu: &PpcCpu) -> i16 {
    let (control, part, tag, size, data) = (
        cpu.gpr[3],
        cpu.gpr[4] as u16 as i16,
        cpu.gpr[5].to_be_bytes(),
        cpu.gpr[6],
        cpu.gpr[7],
    );
    if ppc_control_ptr(memory, control).is_none() {
        CONTROL_HANDLE_INVALID_ERR
    } else if tag != CONTROL_FONT_STYLE_TAG {
        ERR_DATA_NOT_SUPPORTED
    } else if size != CONTROL_FONT_STYLE_SIZE {
        ERR_DATA_SIZE_MISMATCH
    } else if data == 0 {
        PPC_PARAM_ERR
    } else {
        let bytes = (0..size)
            .map(|offset| memory.read_u8(data + offset).unwrap_or(0))
            .collect();
        TAGGED.with(|tagged| tagged.borrow_mut().insert((control, part, tag), bytes));
        PPC_NO_ERR
    }
}

/// A control's ControlFontStyleRec, as SetControlData('font') left it:
/// flags, font, size, style, mode, just (Universal Interfaces 3.4.2
/// Controls.h). None when it was never given one.
pub(super) fn ppc_control_font_style(control: u32) -> Option<[i16; 6]> {
    TAGGED.with(|tagged| {
        tagged
            .borrow()
            .get(&(control, 0, CONTROL_FONT_STYLE_TAG))
            .filter(|bytes| bytes.len() >= 12)
            .map(|bytes| std::array::from_fn(|i| i16::from_be_bytes([bytes[2 * i], bytes[2 * i + 1]])))
    })
}

/// GetControlData(inControl, inPart, inTagName, inBufferSize, outBuffer,
/// VAR outActualSize): OSErr. A control never given a font style has one of
/// all zeroes, "use the window's font".
pub(super) fn ppc_get_control_data(memory: &mut PpcSectionMem, cpu: &PpcCpu) -> i16 {
    let (control, part, tag, buffer_size, buffer, actual_size) = (
        cpu.gpr[3],
        cpu.gpr[4] as u16 as i16,
        cpu.gpr[5].to_be_bytes(),
        cpu.gpr[6],
        cpu.gpr[7],
        cpu.gpr[8],
    );
    if ppc_control_ptr(memory, control).is_none() {
        return CONTROL_HANDLE_INVALID_ERR;
    }
    if tag != CONTROL_FONT_STYLE_TAG {
        return ERR_DATA_NOT_SUPPORTED;
    }
    if buffer_size < CONTROL_FONT_STYLE_SIZE {
        return ERR_DATA_SIZE_MISMATCH;
    }
    let stored = TAGGED
        .with(|tagged| tagged.borrow().get(&(control, part, tag)).cloned())
        .unwrap_or_else(|| vec![0; CONTROL_FONT_STYLE_SIZE as usize]);
    if buffer != 0 {
        for (offset, byte) in stored.iter().enumerate() {
            let _ = memory.write_u8(buffer + offset as u32, *byte);
        }
    }
    if actual_size != 0 {
        let _ = memory.write_u32_be(actual_size, CONTROL_FONT_STYLE_SIZE);
    }
    PPC_NO_ERR
}

/// A slider's value for the pointer at `coordinate` along its long axis:
/// the thumb centred on the pointer, pinned to the track.
pub(super) fn ppc_slider_value_at(
    (top, left, bottom, right): (i16, i16, i16, i16),
    (v, h): (i16, i16),
    min: i16,
    max: i16,
) -> i16 {
    let vertical = bottom - top > right - left;
    let (start, end, coordinate) = if vertical {
        (top, bottom, v)
    } else {
        (left, right, h)
    };
    let travel = i32::from((end - start - PPC_SLIDER_THUMB_SIZE).max(0));
    let range = i32::from(max) - i32::from(min);
    if travel == 0 || range <= 0 {
        return min;
    }
    let position = (i32::from(coordinate) - i32::from(start) - i32::from(PPC_SLIDER_THUMB_SIZE / 2))
        .clamp(0, travel);
    (i32::from(min) + (position * range + travel / 2) / travel) as i16
}
