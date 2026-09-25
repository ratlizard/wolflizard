//! Typed Control Manager dispatch for PowerPC imports.

use super::*;

pub(super) struct PpcControlDispatchContext<'a> {
    pub(super) binding: &'a PpcImportBinding,
    pub(super) cpu: &'a mut PpcCpu,
    pub(super) memory: &'a mut PpcSectionMem,
    pub(super) process_memory_manager: &'a mut ProcessNativeMemoryManager,
    pub(super) heap_cursor: &'a mut u32,
    pub(super) heap_limit: u32,
    pub(super) last_mem_error: &'a mut i16,
    pub(super) handles: &'a mut Vec<PpcHandleRecord>,
    pub(super) controls: &'a mut Vec<PpcControlRecord>,
    pub(super) gworlds: &'a [PpcGWorldRecord],
    pub(super) screen_clut: &'a [[u16; 3]; 256],
    pub(super) current_gworld: u32,
    pub(super) toolbox_startup: &'a mut PpcToolboxStartupState,
    pub(super) input: PpcInputSnapshot,
    pub(super) vfs_resources: &'a mut [PpcVfsResourceRecord],
    pub(super) current_resource_refnum: i16,
    pub(super) last_resource_error: &'a mut i16,
}

pub(super) fn dispatch_control_import(
    context: PpcControlDispatchContext<'_>,
) -> Option<PpcImportAction> {
    let PpcControlDispatchContext {
        binding,
        cpu,
        memory,
        process_memory_manager,
        heap_cursor,
        heap_limit,
        last_mem_error,
        handles,
        controls,
        gworlds,
        screen_clut,
        current_gworld,
        toolbox_startup,
        input,
        vfs_resources,
        current_resource_refnum,
        last_resource_error,
    } = context;

    match binding.dispatcher_target {
        PpcImportDispatcherTarget::DrawControls => {
            let window = cpu.gpr[3];
            if memory.read_u16_be(window + PPC_CWINDOW_WINDOW_KIND_OFFSET) == Some(2) {
                // Dialog controls are represented by the live DITL, whose
                // renderer also translates dialog-local item coordinates.
                let _ = ppc_draw_dialog(
                    memory,
                    handles,
                    controls,
                    gworlds,
                    screen_clut,
                    vfs_resources,
                    current_resource_refnum,
                    window,
                );
            } else {
                let _ = ppc_draw_window_controls(
                    memory,
                    handles,
                    controls,
                    gworlds,
                    vfs_resources,
                    current_resource_refnum,
                    window,
                );
            }
            Some(PpcImportAction::ReturnPreserve)
        }
        PpcImportDispatcherTarget::SetControlTitle => {
            let mut allocator = PpcProcessAllocatorView {
                memory_manager: process_memory_manager,
            };
            ppc_set_control_title(
                cpu,
                Some(&mut allocator),
                memory,
                heap_cursor,
                heap_limit,
                last_mem_error,
                handles,
            );
            Some(PpcImportAction::ReturnPreserve)
        }
        PpcImportDispatcherTarget::SetControlValue => {
            let control_handle = cpu.gpr[3];
            if let Some(control) = memory
                .read_u32_be(control_handle)
                .filter(|control| *control != 0)
            {
                let _ = memory.write_u16_be(
                    control.wrapping_add(PPC_CONTROL_VALUE_OFFSET),
                    cpu.gpr[4] as u16,
                );
                // Popup CDEF records repurpose contrlMin for the menu ID and
                // contrlMax for the title width (MTE, pp. 5-25--5-27), so
                // applying the ordinary range clamp would turn a valid item
                // value into a menu ID/title-width value.
                let is_popup = controls.iter().any(|record| {
                    record.handle == control_handle
                        && (1008..=1023).contains(&(record.proc_id & 0x0fff))
                });
                if !is_popup {
                    ppc_clamp_control_value(memory, control);
                }
                let _ = ppc_draw_control(
                    memory,
                    handles,
                    controls,
                    gworlds,
                    vfs_resources,
                    current_resource_refnum,
                    control_handle,
                );
            }
            Some(PpcImportAction::ReturnPreserve)
        }
        PpcImportDispatcherTarget::HiliteControl => {
            let control_handle = cpu.gpr[3];
            if let Some(control) = memory
                .read_u32_be(control_handle)
                .filter(|control| *control != 0)
            {
                // Macintosh Toolbox Essentials (1992), p. 5-98: byte 17 of
                // ControlRecord is contrlHilite; 255 means inactive.
                let _ = memory.write_u8(control + 17, cpu.gpr[4] as u8);
                let owner = memory.read_u32_be(control + 4).unwrap_or(0);
                let dialog = if owner != 0 { owner } else { current_gworld };
                if !ppc_draw_control(
                    memory,
                    handles,
                    controls,
                    gworlds,
                    vfs_resources,
                    current_resource_refnum,
                    control_handle,
                ) {
                    let _ = ppc_draw_dialog(
                        memory,
                        handles,
                        controls,
                        gworlds,
                        screen_clut,
                        vfs_resources,
                        current_resource_refnum,
                        dialog,
                    );
                }
            }
            Some(PpcImportAction::ReturnPreserve)
        }
        PpcImportDispatcherTarget::LegacyControl(operation) => ppc_dispatch_legacy_control(
            operation,
            cpu,
            process_memory_manager,
            memory,
            heap_cursor,
            heap_limit,
            last_mem_error,
            handles,
            controls,
            gworlds,
            screen_clut,
            toolbox_startup,
            input,
            vfs_resources,
            current_resource_refnum,
            last_resource_error,
        ),
        PpcImportDispatcherTarget::AppearanceControl(operation) => {
            use super::appearance_controls::*;
            use PpcAppearanceControlOperation as Op;
            let result = |err: i16| Some(PpcImportAction::Return(ppc_i16_result(err)));
            match operation {
                Op::CreateRootControl => {
                    let mut allocator = PpcProcessAllocatorView {
                        memory_manager: process_memory_manager,
                    };
                    let err = ppc_create_root_control(
                        cpu,
                        Some(&mut allocator),
                        memory,
                        heap_cursor,
                        heap_limit,
                        last_mem_error,
                        handles,
                        controls,
                    );
                    result(err)
                }
                Op::EmbedControl => result(ppc_embed_control(memory, cpu.gpr[3], cpu.gpr[4])),
                Op::ActivateControl | Op::DeactivateControl => {
                    let active = operation == Op::ActivateControl;
                    match ppc_set_control_family_active(memory, cpu.gpr[3], active) {
                        Ok(family) => {
                            for handle in family {
                                let _ = ppc_draw_control(
                                    memory,
                                    handles,
                                    controls,
                                    gworlds,
                                    vfs_resources,
                                    current_resource_refnum,
                                    handle,
                                );
                            }
                            result(PPC_NO_ERR)
                        }
                        Err(err) => result(err),
                    }
                }
                // FindControlUnderMouse(inWhere, inWindow, VAR outPart):
                // ControlHandle, the part code in the VAR parameter.
                Op::FindControlUnderMouse => {
                    let v = (cpu.gpr[3] >> 16) as u16 as i16;
                    let h = cpu.gpr[3] as u16 as i16;
                    let (handle, part) =
                        ppc_find_control_at_point(memory, controls, cpu.gpr[4], v, h)
                            .unwrap_or((0, 0));
                    if cpu.gpr[5] != 0 {
                        let _ = memory.write_u16_be(cpu.gpr[5], part as u16);
                    }
                    Some(PpcImportAction::Return(handle))
                }
                // HandleControlClick(inControl, inWhere, inModifiers,
                // inAction) is TrackControl with a modifiers word; no control
                // modelled here reads the modifiers.
                Op::HandleControlClick => {
                    cpu.gpr[5] = cpu.gpr[6];
                    ppc_dispatch_legacy_control(
                        PpcLegacyControlOperation::TrackControl,
                        cpu,
                        process_memory_manager,
                        memory,
                        heap_cursor,
                        heap_limit,
                        last_mem_error,
                        handles,
                        controls,
                        gworlds,
                        screen_clut,
                        toolbox_startup,
                        input,
                        vfs_resources,
                        current_resource_refnum,
                        last_resource_error,
                    )
                }
                Op::HandleControlKey => result(0),
                Op::IdleControls => Some(PpcImportAction::ReturnPreserve),
                Op::GetKeyboardFocus => {
                    if cpu.gpr[4] != 0 {
                        let _ = memory.write_u32_be(cpu.gpr[4], 0);
                    }
                    result(PPC_NO_ERR)
                }
                Op::SetControlData => result(ppc_set_control_data(memory, cpu)),
                Op::GetControlData => result(ppc_get_control_data(memory, cpu)),
                // SetThemeWindowBackground(inWindow, inBrush, inUpdate): the
                // window's background becomes the theme's, which later erases
                // use. Only the window background is distinguished here.
                Op::SetThemeWindowBackground => {
                    let window = cpu.gpr[3];
                    if window != 0 {
                        let color = ppc_theme_rgb(ppc_ui_theme(gworlds).provider().palette().window_background);
                        let _ = ppc_write_rgb_color(memory, window + PPC_CGRAF_PORT_RGB_BK_COLOR_OFFSET, color);
                        let _ = memory.write_u32_be(window + PPC_CGRAF_PORT_BK_PIXPAT_OFFSET, 0);
                    }
                    result(PPC_NO_ERR)
                }
            }
        }
        _ => None,
    }
}

pub(super) fn ppc_set_control_title(
    cpu: &PpcCpu,
    allocator: Option<&mut PpcProcessAllocatorView<'_>>,
    memory: &mut PpcSectionMem,
    heap_cursor: &mut u32,
    heap_limit: u32,
    last_mem_error: &mut i16,
    handles: &mut Vec<PpcHandleRecord>,
) {
    let control_handle = cpu.gpr[3];
    let title_ptr = cpu.gpr[4];
    let title = ppc_read_pstring_bytes(memory, title_ptr).unwrap_or_default();
    if control_handle == 0 || title_ptr == 0 {
        *last_mem_error = PPC_PARAM_ERR;
        return;
    }
    let result = ppc_allocator_view_resize_handle(
        allocator,
        memory,
        heap_cursor,
        heap_limit,
        last_mem_error,
        handles,
        control_handle,
        PPC_CONTROL_RECORD_SIZE,
    );
    if result != PPC_NO_ERR {
        *last_mem_error = result;
        return;
    }
    let Some(control) = memory
        .read_u32_be(control_handle)
        .filter(|control| *control != 0)
    else {
        *last_mem_error = PPC_PARAM_ERR;
        return;
    };
    let wrote = ppc_write_pstring_bytes(
        memory,
        control.wrapping_add(PPC_CONTROL_TITLE_OFFSET),
        &title,
    );
    // Macintosh Toolbox Essentials (1992), pp. 5-73 and 5-96: a control's
    // Str255 title begins at byte 40 of its relocatable ControlRecord.
    *last_mem_error = if wrote { PPC_NO_ERR } else { PPC_PARAM_ERR };
}

#[allow(clippy::too_many_arguments)]
fn ppc_dispatch_popup_track_control(
    cpu: &PpcCpu,
    memory: &mut PpcSectionMem,
    handles: &[PpcHandleRecord],
    controls: &[PpcControlRecord],
    gworlds: &[PpcGWorldRecord],
    screen_clut: &[[u16; 3]; 256],
    vfs_resources: &[PpcVfsResourceRecord],
    current_resource_refnum: i16,
    startup: &mut PpcToolboxStartupState,
    input: PpcInputSnapshot,
    control_handle: u32,
    start_v: i16,
    start_h: i16,
) -> Option<PpcImportAction> {
    let record = controls.iter().find(|record| {
        record.handle == control_handle && (1008..=1023).contains(&(record.proc_id & 0x0fff))
    })?;
    // TrackControl's CDEF must not begin a retained popup session for an
    // invisible, inactive, or missed control. Keep this guard in the helper
    // as well as at the dispatcher call site because direct PPC callers can
    // enter this adapter without first running FindControl. Macintosh
    // Toolbox Essentials (1992), pp. 5-67--5-69.
    ppc_control_part_at_point(memory, controls, control_handle, start_v, start_h)?;
    let control = ppc_control_ptr(memory, control_handle)?;
    let owner = memory.read_u32_be(control + PPC_CONTROL_OWNER_OFFSET)?;
    let surface = ppc_live_quickdraw_surface(memory, gworlds, owner)?;
    let menu_id = record.popup_menu_id;
    let current_menu_list = ppc_current_menu_list(memory);
    let menu_handle = ppc_get_menu_handle(memory, current_menu_list, menu_id);
    if menu_handle == 0 {
        return Some(PpcImportAction::Return(0));
    }

    // TrackControl receives a window-local start point, while the Menu
    // Manager's PopUpMenuSelect contract is expressed in global screen
    // coordinates. Reuse the standard retained popup session so CDEF-backed
    // controls get the same disabled/separator hit testing, save-under, and
    // repaint behavior as a direct PopUpMenuSelect call.
    let (control_top, control_left, _, _) =
        ppc_read_rect(memory, control + PPC_CONTROL_RECT_OFFSET)?;
    let (global_h, global_v) =
        surface.local_point((i32::from(control_left), i32::from(control_top)));
    let mut popup_cpu = cpu.clone();
    popup_cpu.gpr[3] = menu_handle;
    popup_cpu.gpr[4] = u32::from(ppc_i32_to_i16_saturating(global_v) as u16);
    popup_cpu.gpr[5] = u32::from(ppc_i32_to_i16_saturating(global_h) as u16);
    popup_cpu.gpr[6] = memory
        .read_u16_be(control + PPC_CONTROL_VALUE_OFFSET)
        .unwrap_or(1) as u32;
    let menu_color_bytes = ppc_menu_color_table_bytes(memory, handles);
    let menu_colors = MenuColorTable::new(&menu_color_bytes);
    let action = ppc_dispatch_pop_up_menu_select(
        &popup_cpu,
        memory,
        gworlds,
        screen_clut,
        menu_colors,
        startup,
        input,
        vfs_resources,
        current_resource_refnum,
    );
    Some(match action {
        PpcImportAction::Return(result) => {
            let item = result as u16 as i16;
            if item > 0 && ppc_menu_item_is_selectable(memory, menu_handle, item) {
                let _ = memory.write_u16_be(control + PPC_CONTROL_VALUE_OFFSET, item as u16);
                let _ = ppc_draw_control(
                    memory,
                    handles,
                    controls,
                    gworlds,
                    vfs_resources,
                    current_resource_refnum,
                    control_handle,
                );
                PpcImportAction::Return(ppc_i16_result(10))
            } else {
                PpcImportAction::Return(0)
            }
        }
        PpcImportAction::ReturnWithExtraCycles(result, extra_cycles) => {
            let item = result as u16 as i16;
            if item > 0 && ppc_menu_item_is_selectable(memory, menu_handle, item) {
                let _ = memory.write_u16_be(control + PPC_CONTROL_VALUE_OFFSET, item as u16);
                let _ = ppc_draw_control(
                    memory,
                    handles,
                    controls,
                    gworlds,
                    vfs_resources,
                    current_resource_refnum,
                    control_handle,
                );
                PpcImportAction::ReturnWithExtraCycles(ppc_i16_result(10), extra_cycles)
            } else {
                PpcImportAction::ReturnWithExtraCycles(0, extra_cycles)
            }
        }
        action => action,
    })
}

#[allow(clippy::too_many_arguments)]
pub(super) fn ppc_dispatch_legacy_control(
    operation: PpcLegacyControlOperation,
    cpu: &mut PpcCpu,
    process_memory_manager: &mut ProcessNativeMemoryManager,
    memory: &mut PpcSectionMem,
    heap_cursor: &mut u32,
    heap_limit: u32,
    last_mem_error: &mut i16,
    handles: &mut Vec<PpcHandleRecord>,
    controls: &mut Vec<PpcControlRecord>,
    gworlds: &[PpcGWorldRecord],
    screen_clut: &[[u16; 3]; 256],
    toolbox_startup: &mut PpcToolboxStartupState,
    input: PpcInputSnapshot,
    vfs_resources: &mut [PpcVfsResourceRecord],
    current_resource_refnum: i16,
    last_resource_error: &mut i16,
) -> Option<PpcImportAction> {
    match operation {
        PpcLegacyControlOperation::NewControl => {
            let ref_con = ppc_parameter_area_slot_addr(cpu.gpr[1], PPC_NATIVE_PARAMETER_GPR_COUNT)
                .and_then(|address| memory.read_u32_be(address))
                .unwrap_or(0);
            let mut allocator = PpcProcessAllocatorView {
                memory_manager: process_memory_manager,
            };
            let handle = ppc_new_control_values(
                Some(&mut allocator),
                memory,
                heap_cursor,
                heap_limit,
                last_mem_error,
                handles,
                controls,
                cpu.gpr[3],
                cpu.gpr[4],
                cpu.gpr[5],
                cpu.gpr[6] != 0,
                cpu.gpr[7] as u16 as i16,
                cpu.gpr[8] as u16 as i16,
                cpu.gpr[9] as u16 as i16,
                cpu.gpr[10] as u16 as i16,
                ref_con,
            );
            ppc_initialize_popup_control(
                memory,
                controls,
                vfs_resources,
                current_resource_refnum,
                handle,
            );
            if handle != 0 && cpu.gpr[6] != 0 {
                let _ = ppc_draw_control(
                    memory,
                    handles,
                    controls,
                    gworlds,
                    vfs_resources,
                    current_resource_refnum,
                    handle,
                );
            }
            Some(PpcImportAction::Return(handle))
        }
        PpcLegacyControlOperation::GetNewControl => {
            let resource_id = cpu.gpr[3] as u16 as i16;
            let owner = cpu.gpr[4];
            let Some(index) = ppc_vfs_resource_index(
                vfs_resources,
                current_resource_refnum,
                u32::from_be_bytes(*b"CNTL"),
                resource_id,
                false,
            ) else {
                *last_resource_error = PPC_RES_NOT_FOUND_ERR;
                return Some(PpcImportAction::Return(0));
            };
            let bytes = vfs_resources[index].data.clone();
            let mut allocator = PpcProcessAllocatorView {
                memory_manager: process_memory_manager,
            };
            let Some((bounds, title_ptr)) = ppc_materialize_control_resource_parameters(
                Some(&mut allocator),
                memory,
                heap_cursor,
                heap_limit,
                &bytes,
            ) else {
                *last_resource_error = PPC_PARAM_ERR;
                return Some(PpcImportAction::Return(0));
            };
            let handle = ppc_new_control_values(
                Some(&mut allocator),
                memory,
                heap_cursor,
                heap_limit,
                last_mem_error,
                handles,
                controls,
                owner,
                bounds,
                title_ptr,
                bytes[10] != 0,
                i16::from_be_bytes([bytes[8], bytes[9]]),
                i16::from_be_bytes([bytes[14], bytes[15]]),
                i16::from_be_bytes([bytes[12], bytes[13]]),
                i16::from_be_bytes([bytes[16], bytes[17]]),
                u32::from_be_bytes([bytes[18], bytes[19], bytes[20], bytes[21]]),
            );
            *last_resource_error = if handle == 0 {
                PPC_PARAM_ERR
            } else {
                PPC_NO_ERR
            };
            ppc_initialize_popup_control(
                memory,
                controls,
                vfs_resources,
                current_resource_refnum,
                handle,
            );
            if handle != 0 && bytes[10] != 0 {
                let _ = ppc_draw_control(
                    memory,
                    handles,
                    controls,
                    gworlds,
                    vfs_resources,
                    current_resource_refnum,
                    handle,
                );
            }
            Some(PpcImportAction::Return(handle))
        }
        PpcLegacyControlOperation::DisposeControl => {
            let mut allocator = PpcProcessAllocatorView {
                memory_manager: process_memory_manager,
            };
            ppc_dispose_control(
                Some(&mut allocator),
                None,
                memory,
                heap_cursor,
                heap_limit,
                last_mem_error,
                handles,
                controls,
                cpu.gpr[3],
            );
            Some(PpcImportAction::ReturnPreserve)
        }
        PpcLegacyControlOperation::KillControls => {
            let owner = cpu.gpr[3];
            let control_handles = ppc_window_control_handles(memory, owner);
            let mut allocator = PpcProcessAllocatorView {
                memory_manager: process_memory_manager,
            };
            for handle in control_handles {
                ppc_dispose_control(
                    Some(&mut allocator),
                    None,
                    memory,
                    heap_cursor,
                    heap_limit,
                    last_mem_error,
                    handles,
                    controls,
                    handle,
                );
            }
            let _ = memory.write_u32_be(owner.wrapping_add(PPC_CWINDOW_CONTROL_LIST_OFFSET), 0);
            Some(PpcImportAction::ReturnPreserve)
        }
        PpcLegacyControlOperation::GetControlValue
        | PpcLegacyControlOperation::GetControlMinimum
        | PpcLegacyControlOperation::GetControlMaximum => {
            let offset = match operation {
                PpcLegacyControlOperation::GetControlMinimum => PPC_CONTROL_MIN_OFFSET,
                PpcLegacyControlOperation::GetControlMaximum => PPC_CONTROL_MAX_OFFSET,
                PpcLegacyControlOperation::GetControlValue => PPC_CONTROL_VALUE_OFFSET,
                _ => unreachable!(),
            };
            let value = ppc_control_ptr(memory, cpu.gpr[3])
                .and_then(|control| memory.read_u16_be(control.wrapping_add(offset)))
                .unwrap_or(0) as i16;
            Some(PpcImportAction::Return(ppc_i16_result(value)))
        }
        // Macintosh Toolbox Essentials (1992), 5-110 to 5-113: the control's
        // reference value (contrlRfCon) and default action procedure
        // (contrlAction), read and written in the record.
        PpcLegacyControlOperation::GetControlReference
        | PpcLegacyControlOperation::GetControlAction => {
            let offset = if operation == PpcLegacyControlOperation::GetControlAction {
                PPC_CONTROL_ACTION_OFFSET
            } else {
                PPC_CONTROL_REF_CON_OFFSET
            };
            let value = ppc_control_ptr(memory, cpu.gpr[3])
                .and_then(|control| memory.read_u32_be(control.wrapping_add(offset)))
                .unwrap_or(0);
            Some(PpcImportAction::Return(value))
        }
        PpcLegacyControlOperation::SetControlReference
        | PpcLegacyControlOperation::SetControlAction => {
            let offset = if operation == PpcLegacyControlOperation::SetControlAction {
                PPC_CONTROL_ACTION_OFFSET
            } else {
                PPC_CONTROL_REF_CON_OFFSET
            };
            if let Some(control) = ppc_control_ptr(memory, cpu.gpr[3]) {
                let _ = memory.write_u32_be(control.wrapping_add(offset), cpu.gpr[4]);
            }
            Some(PpcImportAction::ReturnPreserve)
        }
        PpcLegacyControlOperation::GetControlTitle => {
            if let Some(control) = ppc_control_ptr(memory, cpu.gpr[3]) {
                let title =
                    ppc_read_pstring_bytes(memory, control.wrapping_add(PPC_CONTROL_TITLE_OFFSET))
                        .unwrap_or_default();
                let _ = ppc_write_pstring_bytes(memory, cpu.gpr[4], &title);
            } else if cpu.gpr[4] != 0 {
                let _ = memory.write_u8(cpu.gpr[4], 0);
            }
            Some(PpcImportAction::ReturnPreserve)
        }
        PpcLegacyControlOperation::SetControlMinimum
        | PpcLegacyControlOperation::SetControlMaximum => {
            if let Some(control) = ppc_control_ptr(memory, cpu.gpr[3]) {
                let value = cpu.gpr[4] as u16 as i16;
                let offset = if operation == PpcLegacyControlOperation::SetControlMinimum {
                    PPC_CONTROL_MIN_OFFSET
                } else {
                    PPC_CONTROL_MAX_OFFSET
                };
                let _ = memory.write_u16_be(control.wrapping_add(offset), value as u16);
                ppc_clamp_control_value(memory, control);
                let _ = ppc_draw_control(
                    memory,
                    handles,
                    controls,
                    gworlds,
                    vfs_resources,
                    current_resource_refnum,
                    cpu.gpr[3],
                );
            }
            Some(PpcImportAction::ReturnPreserve)
        }
        PpcLegacyControlOperation::ShowControl | PpcLegacyControlOperation::HideControl => {
            if let Some(control) = ppc_control_ptr(memory, cpu.gpr[3]) {
                let visible = operation == PpcLegacyControlOperation::ShowControl;
                let _ = memory.write_u8(
                    control.wrapping_add(PPC_CONTROL_VISIBLE_OFFSET),
                    if visible { 0xff } else { 0 },
                );
                if visible {
                    let _ = ppc_draw_control(
                        memory,
                        handles,
                        controls,
                        gworlds,
                        vfs_resources,
                        current_resource_refnum,
                        cpu.gpr[3],
                    );
                }
            }
            Some(PpcImportAction::ReturnPreserve)
        }
        PpcLegacyControlOperation::MoveControl | PpcLegacyControlOperation::SizeControl => {
            if let Some(control) = ppc_control_ptr(memory, cpu.gpr[3]) {
                let rect_addr = control.wrapping_add(PPC_CONTROL_RECT_OFFSET);
                if let Some((top, left, bottom, right)) = ppc_read_rect(memory, rect_addr) {
                    let width = right.saturating_sub(left);
                    let height = bottom.saturating_sub(top);
                    let result = if operation == PpcLegacyControlOperation::MoveControl {
                        let new_left = cpu.gpr[4] as u16 as i16;
                        let new_top = cpu.gpr[5] as u16 as i16;
                        ppc_write_rect(
                            memory,
                            rect_addr,
                            new_top,
                            new_left,
                            new_top.saturating_add(height),
                            new_left.saturating_add(width),
                        )
                    } else {
                        let new_width = cpu.gpr[4] as u16 as i16;
                        let new_height = cpu.gpr[5] as u16 as i16;
                        ppc_write_rect(
                            memory,
                            rect_addr,
                            top,
                            left,
                            top.saturating_add(new_height),
                            left.saturating_add(new_width),
                        )
                    };
                    if result.is_some() {
                        let _ = ppc_draw_control(
                            memory,
                            handles,
                            controls,
                            gworlds,
                            vfs_resources,
                            current_resource_refnum,
                            cpu.gpr[3],
                        );
                    }
                }
            }
            Some(PpcImportAction::ReturnPreserve)
        }
        PpcLegacyControlOperation::FindControl => {
            let v = (cpu.gpr[3] >> 16) as u16 as i16;
            let h = cpu.gpr[3] as u16 as i16;
            let (handle, part) =
                ppc_find_control_at_point(memory, controls, cpu.gpr[4], v, h).unwrap_or((0, 0));
            if cpu.gpr[5] != 0 {
                memory.write_u32_be(cpu.gpr[5], handle)?;
            }
            // An application CDEF names its own parts: Cythera's scroll
            // bars answer with their arrows and page regions, which its list
            // code tracks itself.
            if handle != 0 && super::dispatch_defproc::ppc_control_has_app_cdef(handle) {
                super::dispatch_defproc::ppc_note_app_cdef_test(handle, cpu.gpr[3], cpu.gpr[5]);
            }
            Some(PpcImportAction::Return(ppc_i16_result(part)))
        }
        PpcLegacyControlOperation::TestControl => {
            let v = (cpu.gpr[4] >> 16) as u16 as i16;
            let h = cpu.gpr[4] as u16 as i16;
            let part = ppc_control_part_at_point(memory, controls, cpu.gpr[3], v, h).unwrap_or(0);
            if crate::trap::dispatch::trace_input_enabled() {
                eprintln!(
                    "[INPUT] PPC TestControl control=${:08X} point=({}, {}) -> {}",
                    cpu.gpr[3], v, h, part
                );
            }
            if part != 0 && super::dispatch_defproc::ppc_control_has_app_cdef(cpu.gpr[3]) {
                super::dispatch_defproc::ppc_note_app_cdef_test(cpu.gpr[3], cpu.gpr[4], 0);
            }
            Some(PpcImportAction::Return(ppc_i16_result(part)))
        }
        PpcLegacyControlOperation::TrackControl => {
            // Macintosh Toolbox Essentials (1992), pp. 5-79--5-80:
            // -1 selects contrlAction; a second -1 invokes the popup CDEF.
            let action_proc = if cpu.gpr[5] == u32::MAX {
                ppc_control_ptr(memory, cpu.gpr[3])
                    .and_then(|control| memory.read_u32_be(control + 32))
                    .unwrap_or(0)
            } else {
                cpu.gpr[5]
            };
            let v = (cpu.gpr[4] >> 16) as u16 as i16;
            let h = cpu.gpr[4] as u16 as i16;
            let part = ppc_control_part_at_point(memory, controls, cpu.gpr[3], v, h).unwrap_or(0);
            if crate::trap::dispatch::trace_input_enabled() {
                eprintln!(
                    "[INPUT] PPC TrackControl control=${:08X} point=({}, {}) action=${:08X} -> {}",
                    cpu.gpr[3], v, h, cpu.gpr[5], part
                );
            }
            if part != 0 && action_proc == u32::MAX {
                if let Some(action) = ppc_dispatch_popup_track_control(
                    cpu,
                    memory,
                    handles,
                    controls,
                    gworlds,
                    screen_clut,
                    vfs_resources,
                    current_resource_refnum,
                    toolbox_startup,
                    input,
                    cpu.gpr[3],
                    v,
                    h,
                ) {
                    return Some(action);
                }
            }
            let slider = controls
                .iter()
                .find(|record| record.handle == cpu.gpr[3])
                .is_some_and(|record| {
                    super::appearance_controls::ppc_is_slider_proc_id(record.proc_id & 0x0fff)
                });
            if part == 129 && slider {
                // The thumb follows the pointer while the button is down and
                // the value is left where it is released.
                if let Some(control) = ppc_control_ptr(memory, cpu.gpr[3]) {
                    let owner = memory.read_u32_be(control + PPC_CONTROL_OWNER_OFFSET).unwrap_or(0);
                    let origin = memory
                        .read_u32_be(owner.wrapping_add(2))
                        .and_then(|pixmap| memory.read_u32_be(pixmap))
                        .and_then(|pixmap| ppc_read_rect(memory, pixmap.wrapping_add(6)))
                        .map_or((0, 0), |(top, left, _, _)| (top, left));
                    let pointer = (
                        input.mouse_v.wrapping_add(origin.0),
                        input.mouse_h.wrapping_add(origin.1),
                    );
                    if let Some(rect) = ppc_read_rect(memory, control + PPC_CONTROL_RECT_OFFSET) {
                        let min = memory.read_u16_be(control + PPC_CONTROL_MIN_OFFSET).unwrap_or(0) as i16;
                        let max = memory.read_u16_be(control + PPC_CONTROL_MAX_OFFSET).unwrap_or(0) as i16;
                        let value =
                            super::appearance_controls::ppc_slider_value_at(rect, pointer, min, max);
                        if memory.read_u16_be(control + PPC_CONTROL_VALUE_OFFSET) != Some(value as u16) {
                            let _ = memory.write_u16_be(control + PPC_CONTROL_VALUE_OFFSET, value as u16);
                            let _ = ppc_draw_control(
                                memory,
                                handles,
                                controls,
                                gworlds,
                                vfs_resources,
                                current_resource_refnum,
                                cpu.gpr[3],
                            );
                        }
                    }
                    if input.mouse_button {
                        return Some(PpcImportAction::Yield(u64::MAX));
                    }
                }
                return Some(PpcImportAction::Return(ppc_i16_result(129)));
            }
            if part == 129 {
                if let Some(control) = ppc_control_ptr(memory, cpu.gpr[3]) {
                    if let Some((top, left, bottom, right)) =
                        ppc_read_rect(memory, control + PPC_CONTROL_RECT_OFFSET)
                    {
                        let vertical = bottom.saturating_sub(top) >= right.saturating_sub(left);
                        let axis_start = if vertical { top } else { left };
                        let axis_end = if vertical { bottom } else { right };
                        let arrow = (axis_end - axis_start).clamp(1, 16);
                        let track_start = axis_start.saturating_add(arrow);
                        let track_end = axis_end.saturating_sub(arrow);
                        let track = i32::from(track_end.saturating_sub(track_start)).max(1);
                        let thumb = 8i32.min(track);
                        let travel = track.saturating_sub(thumb).max(1);
                        let min = memory
                            .read_u16_be(control + PPC_CONTROL_MIN_OFFSET)
                            .unwrap_or(0) as i16;
                        let max = memory
                            .read_u16_be(control + PPC_CONTROL_MAX_OFFSET)
                            .unwrap_or(0) as i16;
                        let coord = if vertical { v } else { h };
                        let rel_coord =
                            (i32::from(coord) - i32::from(track_start)).clamp(0, travel);
                        let span = i32::from(max).saturating_sub(i32::from(min));
                        let new_val =
                            (i32::from(min) + (rel_coord * span + travel / 2) / travel) as i16;
                        let _ =
                            memory.write_u16_be(control + PPC_CONTROL_VALUE_OFFSET, new_val as u16);
                        let _ = ppc_draw_control(
                            memory,
                            handles,
                            controls,
                            gworlds,
                            vfs_resources,
                            current_resource_refnum,
                            cpu.gpr[3],
                        );
                    }
                }
                return Some(PpcImportAction::Return(ppc_i16_result(129)));
            }
            if part == 0 || action_proc == 0 || action_proc == u32::MAX {
                return Some(PpcImportAction::Return(ppc_i16_result(part)));
            }
            let restore_rtoc = cpu.gpr[2];
            let final_pc = cpu.lr;
            let target = ppc_resolve_callback_target(memory, action_proc, restore_rtoc, None)?;
            install_powerpc_call_arguments(cpu, memory, &[cpu.gpr[3], part as u16 as u32])?;
            Some(
                GuestCallEffect::call_guest(
                    GuestCallRequest::new(GuestCallTarget {
                        isa: GuestIsa::PowerPc,
                        entry: target.entry,
                        rtoc: target.rtoc,
                    }),
                    GuestCallContinuation::to_powerpc(
                        PPC_GUEST_CALL_RETURN_PC,
                        final_pc,
                        restore_rtoc,
                        PpcNativeReturnGpr3::Set(ppc_i16_result(part)),
                    ),
                )
                .into_ppc_import_action()?,
            )
        }
        PpcLegacyControlOperation::DrawOneControl => {
            let _ = ppc_draw_control(
                memory,
                handles,
                controls,
                gworlds,
                vfs_resources,
                current_resource_refnum,
                cpu.gpr[3],
            );
            Some(PpcImportAction::ReturnPreserve)
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn ppc_new_control_values(
    allocator: Option<&mut PpcProcessAllocatorView<'_>>,
    memory: &mut PpcSectionMem,
    heap_cursor: &mut u32,
    heap_limit: u32,
    last_mem_error: &mut i16,
    handles: &mut Vec<PpcHandleRecord>,
    controls: &mut Vec<PpcControlRecord>,
    owner: u32,
    bounds_ptr: u32,
    title_ptr: u32,
    visible: bool,
    value: i16,
    min: i16,
    max: i16,
    proc_id: i16,
    ref_con: u32,
) -> u32 {
    let Some(bounds) = ppc_read_rect(memory, bounds_ptr) else {
        *last_mem_error = PPC_PARAM_ERR;
        return 0;
    };
    let title = ppc_read_pstring_bytes(memory, title_ptr).unwrap_or_default();
    ppc_new_control_record_values(
        allocator,
        memory,
        heap_cursor,
        heap_limit,
        last_mem_error,
        handles,
        controls,
        owner,
        bounds,
        &title,
        visible,
        value,
        min,
        max,
        proc_id,
        ref_con,
    )
}

#[allow(clippy::too_many_arguments)]
pub(super) fn ppc_new_control_record_values(
    allocator: Option<&mut PpcProcessAllocatorView<'_>>,
    memory: &mut PpcSectionMem,
    heap_cursor: &mut u32,
    heap_limit: u32,
    last_mem_error: &mut i16,
    handles: &mut Vec<PpcHandleRecord>,
    controls: &mut Vec<PpcControlRecord>,
    owner: u32,
    bounds: (i16, i16, i16, i16),
    title: &[u8],
    visible: bool,
    value: i16,
    min: i16,
    max: i16,
    proc_id: i16,
    ref_con: u32,
) -> u32 {
    if owner == 0
        || memory
            .read_u32_be(owner.wrapping_add(PPC_CWINDOW_CONTROL_LIST_OFFSET))
            .is_none()
    {
        *last_mem_error = PPC_PARAM_ERR;
        return 0;
    }
    let handle = ppc_allocator_view_allocate_handle(
        allocator,
        memory,
        heap_cursor,
        heap_limit,
        last_mem_error,
        handles,
        PPC_CONTROL_RECORD_SIZE,
        true,
    );
    let Some(control) = ppc_control_ptr(memory, handle) else {
        *last_mem_error = PPC_MEM_FULL_ERR;
        return 0;
    };
    let old_head = memory
        .read_u32_be(owner.wrapping_add(PPC_CWINDOW_CONTROL_LIST_OFFSET))
        .unwrap_or(0);
    // Popup CDEF records repurpose contrlMin for the menu ID and contrlMax
    // for the title width, so their value is a menu item number rather than
    // an ordinary min/max control value. Macintosh Toolbox Essentials (1992),
    // pp. 5-25--5-27.
    // Icon and picture controls take the ID of the resource they show as
    // their value, with a maximum of 1 (Universal Interfaces 3.4.2,
    // Controls.h, "ICON CONTROL (CDEF 20)" and "PICTURE CONTROL (CDEF
    // 19)"), so pinning it loses the ID: Cythera's Preferences create their
    // speaker icons this way, with a maximum of 1.
    let initial_value = if (1008..=1023).contains(&(proc_id & 0x0fff))
        || matches!(proc_id & 0x0fff, 304 | 305 | 320..=323)
    {
        value
    } else {
        value.clamp(min.min(max), min.max(max))
    };
    let wrote = memory
        .write_u32_be(control.wrapping_add(PPC_CONTROL_NEXT_OFFSET), old_head)
        .is_some()
        && memory
            .write_u32_be(control.wrapping_add(PPC_CONTROL_OWNER_OFFSET), owner)
            .is_some()
        && ppc_write_rect(
            memory,
            control + PPC_CONTROL_RECT_OFFSET,
            bounds.0,
            bounds.1,
            bounds.2,
            bounds.3,
        )
        .is_some()
        && memory
            .write_u8(
                control.wrapping_add(PPC_CONTROL_VISIBLE_OFFSET),
                if visible { 0xff } else { 0 },
            )
            .is_some()
        && memory
            .write_u8(control.wrapping_add(PPC_CONTROL_HILITE_OFFSET), 0)
            .is_some()
        && memory
            .write_u16_be(
                control.wrapping_add(PPC_CONTROL_VALUE_OFFSET),
                initial_value as u16,
            )
            .is_some()
        && memory
            .write_u16_be(control.wrapping_add(PPC_CONTROL_MIN_OFFSET), min as u16)
            .is_some()
        && memory
            .write_u16_be(control.wrapping_add(PPC_CONTROL_MAX_OFFSET), max as u16)
            .is_some()
        && memory
            .write_u32_be(control.wrapping_add(PPC_CONTROL_REF_CON_OFFSET), ref_con)
            .is_some()
        && ppc_write_pstring_bytes(
            memory,
            control.wrapping_add(PPC_CONTROL_TITLE_OFFSET),
            title,
        )
        && memory
            .write_u32_be(owner.wrapping_add(PPC_CWINDOW_CONTROL_LIST_OFFSET), handle)
            .is_some();
    if !wrote {
        *last_mem_error = PPC_PARAM_ERR;
        return 0;
    }
    let popup = (1008..=1023).contains(&(proc_id & 0x0fff));
    // Macintosh Toolbox Essentials (1992), pp. 5-79--5-80.
    let _ = memory.write_u32_be(control + 32, if popup { u32::MAX } else { 0 });
    super::dispatch_defproc::ppc_register_control_proc(handle, proc_id as i16);
    controls.retain(|record| record.handle != handle);
    controls.push(PpcControlRecord {
        handle,
        pointer: control,
        proc_id,
        popup_menu_id: if popup { min } else { 0 },
        popup_title_width: popup.then_some(max),
    });
    *last_mem_error = PPC_NO_ERR;
    handle
}

// GetNewControl converts popup creation parameters into the live item range.
// The resource value is title style, not the selected item; menu ID and title
// width are retained in PpcControlRecord. Macintosh Toolbox Essentials (1992),
// Creating Pop-Up Menus, pp. 5-25--5-27.
pub(super) fn ppc_initialize_popup_control(
    memory: &mut PpcSectionMem,
    controls: &[PpcControlRecord],
    resources: &[PpcVfsResourceRecord],
    current_resource_refnum: i16,
    handle: u32,
) {
    let Some(record) = controls.iter().find(|record| {
        record.handle == handle && (1008..=1023).contains(&(record.proc_id & 0x0fff))
    }) else {
        return;
    };
    let Some(control) = ppc_control_ptr(memory, handle) else {
        return;
    };
    let menu_list = ppc_current_menu_list(memory);
    let menu = ppc_get_menu_handle(memory, menu_list, record.popup_menu_id);
    let count = if menu != 0 {
        usize::from(ppc_count_menu_items(memory, menu))
    } else {
        ppc_vfs_resource_index(
            resources,
            current_resource_refnum,
            u32::from_be_bytes(*b"MENU"),
            record.popup_menu_id,
            false,
        )
        .and_then(|index| ppc_decode_menu_items(&resources[index].data))
        .map_or(0, |(_, _, items)| items.len())
    };
    let count = count.min(i16::MAX as usize) as u16;
    let _ = memory.write_u16_be(control + PPC_CONTROL_VALUE_OFFSET, u16::from(count != 0));
    let _ = memory.write_u16_be(control + PPC_CONTROL_MIN_OFFSET, 1);
    let _ = memory.write_u16_be(control + PPC_CONTROL_MAX_OFFSET, count);
}

fn ppc_materialize_control_resource_parameters(
    allocator: Option<&mut PpcProcessAllocatorView<'_>>,
    memory: &mut PpcSectionMem,
    heap_cursor: &mut u32,
    heap_limit: u32,
    bytes: &[u8],
) -> Option<(u32, u32)> {
    if bytes.len() < 23 {
        return None;
    }
    let title_len = usize::from(bytes[22]).min(bytes.len().saturating_sub(23));
    let scratch = ppc_allocator_view_reserve_bytes(
        allocator,
        memory,
        heap_cursor,
        heap_limit,
        u32::try_from(8 + 1 + title_len).ok()?,
        true,
    );
    if scratch == 0 {
        return None;
    }
    memory.write_bytes(scratch, &bytes[..8])?;
    memory.write_u8(scratch + 8, title_len as u8)?;
    memory.write_bytes(scratch + 9, &bytes[23..23 + title_len])?;
    Some((scratch, scratch + 8))
}

pub(super) fn ppc_control_ptr(memory: &mut PpcSectionMem, handle: u32) -> Option<u32> {
    (handle != 0)
        .then(|| memory.read_u32_be(handle))
        .flatten()
        .filter(|control| *control != 0)
}

pub(super) fn ppc_window_control_handles(memory: &mut PpcSectionMem, owner: u32) -> Vec<u32> {
    let mut out = Vec::new();
    let mut handle = memory
        .read_u32_be(owner.wrapping_add(PPC_CWINDOW_CONTROL_LIST_OFFSET))
        .unwrap_or(0);
    while handle != 0 && out.len() < 4096 && !out.contains(&handle) {
        out.push(handle);
        handle = ppc_control_ptr(memory, handle)
            .and_then(|control| memory.read_u32_be(control.wrapping_add(PPC_CONTROL_NEXT_OFFSET)))
            .unwrap_or(0);
    }
    out
}

#[allow(clippy::too_many_arguments)]
pub(super) fn ppc_dispose_control(
    allocator: Option<&mut PpcProcessAllocatorView<'_>>,
    legacy_free_handle_blocks: Option<&mut Vec<PpcHandleRecord>>,
    memory: &mut PpcSectionMem,
    heap_cursor: &mut u32,
    heap_limit: u32,
    last_mem_error: &mut i16,
    handles: &mut Vec<PpcHandleRecord>,
    controls: &mut Vec<PpcControlRecord>,
    handle: u32,
) {
    super::appearance_controls::ppc_forget_appearance_control(handle);
    let Some(control) = ppc_control_ptr(memory, handle) else {
        return;
    };
    let owner = memory
        .read_u32_be(control.wrapping_add(PPC_CONTROL_OWNER_OFFSET))
        .unwrap_or(0);
    let next = memory
        .read_u32_be(control.wrapping_add(PPC_CONTROL_NEXT_OFFSET))
        .unwrap_or(0);
    let head_addr = owner.wrapping_add(PPC_CWINDOW_CONTROL_LIST_OFFSET);
    let head = memory.read_u32_be(head_addr).unwrap_or(0);
    if head == handle {
        let _ = memory.write_u32_be(head_addr, next);
    } else {
        for candidate in ppc_window_control_handles(memory, owner) {
            let Some(candidate_ptr) = ppc_control_ptr(memory, candidate) else {
                continue;
            };
            if memory.read_u32_be(candidate_ptr.wrapping_add(PPC_CONTROL_NEXT_OFFSET))
                == Some(handle)
            {
                let _ =
                    memory.write_u32_be(candidate_ptr.wrapping_add(PPC_CONTROL_NEXT_OFFSET), next);
                break;
            }
        }
    }
    if let Some(allocator) = allocator {
        let _ = allocator.dispose_handle(
            memory,
            heap_cursor,
            heap_limit,
            last_mem_error,
            handles,
            handle,
        );
    } else if let Some(free_handle_blocks) = legacy_free_handle_blocks {
        if let Some(index) = handles.iter().position(|record| record.handle == handle) {
            let record = handles.remove(index);
            let _ = memory.write_u32_be(handle, 0);
            free_handle_blocks.push(record);
        }
    }
    controls.retain(|record| record.handle != handle);
}

pub(super) fn ppc_clamp_control_value(memory: &mut PpcSectionMem, control: u32) {
    let value = memory
        .read_u16_be(control.wrapping_add(PPC_CONTROL_VALUE_OFFSET))
        .unwrap_or(0) as i16;
    let min = memory
        .read_u16_be(control.wrapping_add(PPC_CONTROL_MIN_OFFSET))
        .unwrap_or(0) as i16;
    let max = memory
        .read_u16_be(control.wrapping_add(PPC_CONTROL_MAX_OFFSET))
        .unwrap_or(0) as i16;
    let _ = memory.write_u16_be(
        control.wrapping_add(PPC_CONTROL_VALUE_OFFSET),
        value.clamp(min.min(max), min.max(max)) as u16,
    );
}

fn ppc_control_part_at_point(
    memory: &mut PpcSectionMem,
    controls: &[PpcControlRecord],
    handle: u32,
    v: i16,
    h: i16,
) -> Option<i16> {
    let control = ppc_control_ptr(memory, handle)?;
    if memory.read_u8(control + PPC_CONTROL_VISIBLE_OFFSET)? == 0
        || memory.read_u8(control + PPC_CONTROL_HILITE_OFFSET)? >= 0xfe
    {
        return None;
    }
    let (top, left, bottom, right) = ppc_read_rect(memory, control + PPC_CONTROL_RECT_OFFSET)?;
    if v < top || v >= bottom || h < left || h >= right {
        return None;
    }
    let proc_id = controls
        .iter()
        .find(|record| record.handle == handle)
        .map_or(0, |record| record.proc_id);
    if super::appearance_controls::ppc_is_passive_appearance_proc_id(proc_id & 0x0fff) {
        return None;
    }
    if super::appearance_controls::ppc_is_slider_proc_id(proc_id & 0x0fff) {
        // An Appearance slider is all indicator.
        return Some(129);
    }
    match proc_id & 0x0fff {
        0 => Some(10),
        1 | 2 => Some(11),
        16 => {
            let vertical = bottom.saturating_sub(top) >= right.saturating_sub(left);
            let axis_start = if vertical { top } else { left };
            let axis_end = if vertical { bottom } else { right };
            let coordinate = if vertical { v } else { h };
            let arrow = (axis_end - axis_start).clamp(1, 16);
            if coordinate < axis_start.saturating_add(arrow) {
                return Some(20);
            }
            if coordinate >= axis_end.saturating_sub(arrow) {
                return Some(21);
            }
            let min = memory.read_u16_be(control + PPC_CONTROL_MIN_OFFSET)? as i16;
            let max = memory.read_u16_be(control + PPC_CONTROL_MAX_OFFSET)? as i16;
            let value = memory.read_u16_be(control + PPC_CONTROL_VALUE_OFFSET)? as i16;
            let track_start = axis_start.saturating_add(arrow);
            let track_end = axis_end.saturating_sub(arrow);
            let track = i32::from(track_end.saturating_sub(track_start)).max(1);
            let thumb = 8i32.min(track);
            let span = i32::from(max).saturating_sub(i32::from(min)).max(1);
            let relative = i32::from(value)
                .saturating_sub(i32::from(min))
                .clamp(0, span);
            let thumb_start = i32::from(track_start)
                + relative.saturating_mul(track.saturating_sub(thumb)) / span;
            let coordinate = i32::from(coordinate);
            if coordinate < thumb_start {
                Some(22)
            } else if coordinate < thumb_start + thumb {
                Some(129)
            } else {
                Some(23)
            }
        }
        _ => Some(10),
    }
}

pub(super) fn ppc_find_control_at_point(
    memory: &mut PpcSectionMem,
    controls: &[PpcControlRecord],
    owner: u32,
    v: i16,
    h: i16,
) -> Option<(u32, i16)> {
    ppc_window_control_handles(memory, owner)
        .into_iter()
        .find_map(|handle| {
            ppc_control_part_at_point(memory, controls, handle, v, h).map(|part| (handle, part))
        })
}

pub(super) fn ppc_popup_control_selected_text(
    memory: &mut PpcSectionMem,
    vfs_resources: &[PpcVfsResourceRecord],
    current_resource_refnum: i16,
    menu_id: i16,
    selected: usize,
) -> Vec<u8> {
    // A menu created by NewMenu has no MENU resource to consult. The live
    // MenuHandle is also authoritative after SetItem/AppendMenu mutate an
    // existing menu, so prefer its guest bytes and use the resource only as
    // the fallback for resource-backed menus that have not been materialized.
    let current_menu_list = ppc_current_menu_list(memory);
    let menu_handle = ppc_get_menu_handle(memory, current_menu_list, menu_id);
    if let Ok(item_number) = i16::try_from(selected) {
        if let Some((item_address, item_length)) = ppc_menu_item(memory, menu_handle, item_number) {
            return ppc_memory_read_bytes(
                memory,
                item_address.saturating_add(1),
                u32::from(item_length),
            )
            .unwrap_or_default();
        }
    }

    ppc_vfs_resource_index(
        vfs_resources,
        current_resource_refnum,
        u32::from_be_bytes(*b"MENU"),
        menu_id,
        false,
    )
    .and_then(|index| ppc_decode_menu_items(&vfs_resources[index].data))
    .and_then(|(_, _, items)| selected.checked_sub(1).and_then(|i| items.get(i).cloned()))
    .map(|item| item.text)
    .unwrap_or_default()
}

pub(super) fn ppc_popup_control_display_title(
    title: &[u8],
    available_width: i16,
    text_font: i16,
    text_size: i16,
) -> Vec<u8> {
    if available_width <= 0 {
        return Vec::new();
    }
    if ppc_text_bytes_advance_for_font(title, text_font, text_size) <= available_width {
        return title.to_vec();
    }

    let ellipsis = b"...";
    let ellipsis_width = ppc_text_bytes_advance_for_font(ellipsis, text_font, text_size);
    if ellipsis_width > available_width {
        return Vec::new();
    }

    let mut prefix = Vec::new();
    let mut prefix_width = 0i16;
    for byte in title {
        let byte_width = ppc_text_byte_advance_for_font(*byte, text_font, text_size);
        if prefix_width
            .saturating_add(byte_width)
            .saturating_add(ellipsis_width)
            > available_width
        {
            break;
        }
        prefix.push(*byte);
        prefix_width = prefix_width.saturating_add(byte_width);
    }
    prefix.extend_from_slice(ellipsis);
    prefix
}

pub(super) fn ppc_draw_control(
    memory: &mut PpcSectionMem,
    handles: &[PpcHandleRecord],
    controls: &[PpcControlRecord],
    gworlds: &[PpcGWorldRecord],
    vfs_resources: &[PpcVfsResourceRecord],
    current_resource_refnum: i16,
    handle: u32,
) -> bool {
    ppc_draw_control_inner(
        memory,
        handles,
        controls,
        gworlds,
        vfs_resources,
        current_resource_refnum,
        handle,
        false,
    )
}

#[allow(clippy::too_many_arguments)]
pub(super) fn ppc_draw_window_controls(
    memory: &mut PpcSectionMem,
    handles: &[PpcHandleRecord],
    controls: &[PpcControlRecord],
    gworlds: &[PpcGWorldRecord],
    vfs_resources: &[PpcVfsResourceRecord],
    current_resource_refnum: i16,
    window: u32,
) -> bool {
    // Macintosh Toolbox Essentials (1992), pp. 5-87--5-88: DrawControls
    // draws every visible control in reverse order of creation. NewControl
    // prepends to wControlList, so walking the list tail-first preserves the
    // documented overlap order (the first-created control is frontmost).
    let head = memory
        .read_u32_be(window.wrapping_add(PPC_CWINDOW_CONTROL_LIST_OFFSET))
        .unwrap_or(0);
    let control_handles = crate::control_manager::control_draw_order(head, |handle| {
        ppc_control_ptr(memory, handle)
            .and_then(|control| memory.read_u32_be(control.wrapping_add(PPC_CONTROL_NEXT_OFFSET)))
    });
    let mut drew = false;
    for handle in control_handles {
        drew |= ppc_draw_control(
            memory,
            handles,
            controls,
            gworlds,
            vfs_resources,
            current_resource_refnum,
            handle,
        );
    }
    drew
}

pub(super) fn ppc_blit_theme_bitmap(
    memory: &mut PpcSectionMem,
    gworlds: &[PpcGWorldRecord],
    port: u32,
    top: i16,
    left: i16,
    bitmap: &ThemeBitmap,
) -> bool {
    ppc_blit_theme_bitmap_masked(memory, gworlds, port, top, left, bitmap, None)
}

pub(super) fn ppc_blit_theme_bitmap_masked(
    memory: &mut PpcSectionMem,
    gworlds: &[PpcGWorldRecord],
    port: u32,
    top: i16,
    left: i16,
    bitmap: &ThemeBitmap,
    transparent: Option<Rgb8>,
) -> bool {
    let Some(surface) = ppc_live_quickdraw_surface(memory, gworlds, port) else {
        return false;
    };
    let clip_storage = memory
        .read_u32_be(port.wrapping_add(PPC_CGRAF_PORT_CLIP_RGN_OFFSET))
        .and_then(|clip_rgn| ppc_region_storage(memory, clip_rgn));
    let vis_storage = memory
        .read_u32_be(port.wrapping_add(PPC_CGRAF_PORT_VIS_RGN_OFFSET))
        .and_then(|vis_rgn| ppc_region_storage(memory, vis_rgn));
    let mut pixels = HashMap::new();
    let rgba = bitmap.rgba();
    let mut wrote = false;
    for y in 0..bitmap.height() {
        for x in 0..bitmap.width() {
            let offset = ((y * bitmap.width() + x) * 4) as usize;
            let rgb = Rgb8 {
                r: rgba[offset],
                g: rgba[offset + 1],
                b: rgba[offset + 2],
            };
            if transparent == Some(rgb) {
                continue;
            }
            let pixel = *pixels.entry((rgb.r, rgb.g, rgb.b)).or_insert_with(|| {
                ppc_quickdraw_surface_color_pixel(
                    memory,
                    surface,
                    PpcRgbColor {
                        red: u16::from(rgb.r) * 0x0101,
                        green: u16::from(rgb.g) * 0x0101,
                        blue: u16::from(rgb.b) * 0x0101,
                    },
                )
                .unwrap_or(0)
            });
            let port_h = i32::from(left) + x as i32;
            let port_v = i32::from(top) + y as i32;
            let point = surface.local_point((port_h, port_v));
            if ppc_local_point_in_port_regions(
                surface,
                point,
                vis_storage.as_deref(),
                clip_storage.as_deref(),
            ) {
                wrote |= ppc_quickdraw_write_raw_pixel(memory, surface.front_buffer, point, pixel);
            }
        }
    }
    wrote
}

#[allow(clippy::too_many_arguments)]
pub(super) fn ppc_draw_control_inner(
    memory: &mut PpcSectionMem,
    _handles: &[PpcHandleRecord],
    controls: &[PpcControlRecord],
    gworlds: &[PpcGWorldRecord],
    vfs_resources: &[PpcVfsResourceRecord],
    current_resource_refnum: i16,
    handle: u32,
    draw_dialog_popup: bool,
) -> bool {
    let Some(control) = ppc_control_ptr(memory, handle) else {
        return false;
    };
    if memory
        .read_u8(control + PPC_CONTROL_VISIBLE_OFFSET)
        .unwrap_or(0)
        == 0
    {
        return true;
    }
    // A control with an application CDEF is drawn by that CDEF, after the
    // import that would have drawn it (drawCntl, whole control).
    if super::dispatch_defproc::ppc_control_has_app_cdef(handle) {
        super::dispatch_defproc::ppc_note_app_cdef_message(
            handle,
            super::dispatch_defproc::CDEF_DRAW,
            0,
        );
        return true;
    }
    let owner = memory
        .read_u32_be(control + PPC_CONTROL_OWNER_OFFSET)
        .unwrap_or(0);
    let Some((top, left, bottom, right)) = ppc_read_rect(memory, control + PPC_CONTROL_RECT_OFFSET)
    else {
        return false;
    };
    let palette = ppc_ui_theme(gworlds).provider().palette();
    let record = controls.iter().find(|record| record.handle == handle);
    let proc_id = record.map_or(0, |record| record.proc_id) & 0x0fff;
    if matches!(proc_id, 320 | 321) {
        // An icon control's value is the ID of the 'cicn' or 'ICON' it
        // shows (Universal Interfaces 3.4.2, Controls.h, "ICON CONTROL (CDEF
        // 20)"), drawn into its rectangle. Cythera's Preferences put a
        // speaker at each end of its volume sliders this way.
        let icon_id = memory
            .read_u16_be(control + PPC_CONTROL_VALUE_OFFSET)
            .unwrap_or(0) as i16;
        let resource = |res_type: &[u8; 4]| {
            ppc_vfs_resource_index(
                vfs_resources,
                current_resource_refnum,
                u32::from_be_bytes(*res_type),
                icon_id,
                false,
            )
            .and_then(|index| vfs_resources.get(index))
            .map(|record| record.data.clone())
        };
        let rect = (top, left, bottom, right);
        if let Some(data) = resource(b"cicn") {
            let _ =
                super::dispatch_cursor::ppc_plot_cicn_resource(memory, gworlds, owner, rect, &data);
        } else if let Some(data) = resource(b"ICON") {
            let _ =
                super::dispatch_cursor::ppc_plot_icon_resource(memory, gworlds, owner, rect, &data);
        }
        return true;
    }
    if matches!(proc_id, 256 | 304 | 305) {
        // A user pane draws nothing of its own. Picture controls name a
        // resource this host does not draw yet; nothing is better than a
        // stand-in, as on the 68K path.
        return true;
    }
    if matches!(proc_id, 160 | 161 | 288)
        || super::appearance_controls::ppc_is_slider_proc_id(proc_id)
    {
        return ppc_draw_appearance_control(
            memory,
            gworlds,
            owner,
            handle,
            control,
            proc_id,
            (top, left, bottom, right),
        );
    }
    let mut frame_cpu = PpcCpu::new();
    frame_cpu.gpr[3] = control + PPC_CONTROL_RECT_OFFSET;
    let is_default = ppc_ui_theme(gworlds) != UiThemeId::ClassicSystem7
        && proc_id == 0
        && memory.read_u16_be(owner + PPC_CWINDOW_WINDOW_KIND_OFFSET) == Some(2)
        && ppc_dialog_items_for_dialog(memory, _handles, owner).is_some_and(|items| {
            let index = memory
                .read_u16_be(owner + PPC_DIALOG_DEFAULT_ITEM_OFFSET)
                .unwrap_or(1);
            items
                .get(usize::from(index.saturating_sub(1)))
                .is_some_and(|item| memory.read_u32_be(item.handle) == Some(control))
        });
    let themed = ppc_draw_themed_control(
        memory,
        gworlds,
        owner,
        control,
        proc_id,
        is_default,
        (top, left, bottom, right),
    );
    let framed = if let Some(drawn) = themed {
        drawn
    } else {
        match proc_id {
            0 => {
                let slot = memory.presentation();
                let detail =
                    ppc_live_quickdraw_surface(memory, gworlds, owner).and_then(|surface| {
                        if !matches!(surface.front_buffer.depth, 8 | 16) {
                            return None;
                        }
                        let foreground = ppc_quickdraw_surface_fore_pixel(
                            memory,
                            surface,
                            ppc_theme_rgb(palette.frame_dark),
                            None,
                        )? as u32;
                        let background = ppc_quickdraw_surface_fore_pixel(
                            memory,
                            surface,
                            ppc_theme_rgb(palette.window_background),
                            None,
                        )? as u32;
                        let clip = memory
                            .read_u32_be(owner + PPC_CGRAF_PORT_CLIP_RGN_OFFSET)
                            .and_then(|rgn| ppc_region_storage(memory, rgn));
                        let vis = memory
                            .read_u32_be(owner + PPC_CGRAF_PORT_VIS_RGN_OFFSET)
                            .and_then(|rgn| ppc_region_storage(memory, rgn));
                        let fb = surface.front_buffer;
                        slot.rounded_control_corners(
                            surface.local_rect((top, left, bottom, right)),
                            crate::control_manager::STANDARD_BUTTON_OVAL.into(),
                            1,
                            fb.depth as u16,
                            Some(background),
                            foreground,
                            |x, y, lane| {
                                if x < 0
                                    || y < 0
                                    || x >= fb.width as i32
                                    || y >= fb.height as i32
                                    || !ppc_local_point_in_port_regions(
                                        surface,
                                        (x, y),
                                        vis.as_deref(),
                                        clip.as_deref(),
                                    )
                                {
                                    return None;
                                }
                                let address = fb.base_addr
                                    + y as u32 * fb.row_bytes
                                    + x as u32 * (fb.depth / 8)
                                    + lane;
                                Some((address, memory.read_u8(address)?))
                            },
                        )
                    });
                frame_cpu.gpr[4] = crate::control_manager::STANDARD_BUTTON_OVAL as u32;
                frame_cpu.gpr[5] = crate::control_manager::STANDARD_BUTTON_OVAL as u32;
                let _ = ppc_paint_round_rect(
                    &frame_cpu,
                    memory,
                    gworlds,
                    owner,
                    ppc_theme_rgb(palette.window_background),
                    None,
                );
                let framed = ppc_frame_round_rect(
                    &frame_cpu,
                    memory,
                    gworlds,
                    owner,
                    ppc_theme_rgb(palette.frame_dark),
                    None,
                );
                slot.finish_rounded_control(detail, |address| memory.read_u8(address).unwrap_or(0));
                framed
            }
            1 => {
                // Macintosh Toolbox Essentials (1992), pp. 5-15--5-16: the
                // standard checkbox CDEF draws a compact indicator at the left
                // of the control title and marks it when contrlValue is nonzero.
                let layout =
                    crate::control_manager::standard_checkbox_layout((top, left, bottom, right));
                let (box_top, box_left, box_bottom, box_right) = layout.indicator;
                let indicator_size = box_bottom.saturating_sub(box_top);
                let mut wrote = ppc_paint_rect_bounds(
                    memory,
                    gworlds,
                    owner,
                    layout.indicator,
                    ppc_theme_rgb(palette.window_background),
                    None,
                );
                wrote |= ppc_line_to(
                    memory,
                    gworlds,
                    owner,
                    (box_left, box_top),
                    (box_right.saturating_sub(1), box_top),
                    ppc_theme_rgb(palette.frame_dark),
                    None,
                );
                wrote |= ppc_line_to(
                    memory,
                    gworlds,
                    owner,
                    (box_right.saturating_sub(1), box_top),
                    (box_right.saturating_sub(1), box_bottom.saturating_sub(1)),
                    ppc_theme_rgb(palette.frame_dark),
                    None,
                );
                wrote |= ppc_line_to(
                    memory,
                    gworlds,
                    owner,
                    (box_right.saturating_sub(1), box_bottom.saturating_sub(1)),
                    (box_left, box_bottom.saturating_sub(1)),
                    ppc_theme_rgb(palette.frame_dark),
                    None,
                );
                wrote |= ppc_line_to(
                    memory,
                    gworlds,
                    owner,
                    (box_left, box_bottom.saturating_sub(1)),
                    (box_left, box_top),
                    ppc_theme_rgb(palette.frame_dark),
                    None,
                );
                if memory
                    .read_u16_be(control + PPC_CONTROL_VALUE_OFFSET)
                    .unwrap_or(0)
                    != 0
                {
                    crate::control_manager::for_each_standard_checkbox_mark_pixel(
                        indicator_size,
                        |x, y| {
                            wrote |= ppc_line_to(
                                memory,
                                gworlds,
                                owner,
                                (box_left.saturating_add(x), box_top.saturating_add(y)),
                                (box_left.saturating_add(x), box_top.saturating_add(y)),
                                ppc_theme_rgb(palette.frame_dark),
                                None,
                            );
                        },
                    );
                }
                wrote
            }
            2 => {
                // Inside Macintosh Volume I (1985), p. I-322: radioButProc is a
                // round indicator whose on state is a small filled black circle.
                let layout = crate::control_manager::standard_radio_button_layout((
                    top, left, bottom, right,
                ));
                let indicator = layout.indicator;
                let mut wrote = ppc_draw_oval_bounds(
                    memory,
                    gworlds,
                    owner,
                    indicator,
                    ppc_theme_rgb(palette.window_background),
                    None,
                    false,
                );
                wrote |= ppc_draw_oval_bounds(
                    memory,
                    gworlds,
                    owner,
                    indicator,
                    ppc_theme_rgb(palette.frame_dark),
                    None,
                    true,
                );
                if memory
                    .read_u16_be(control + PPC_CONTROL_VALUE_OFFSET)
                    .unwrap_or(0)
                    != 0
                {
                    let (dot_top, dot_left, dot_bottom, dot_right) = indicator;
                    wrote |= ppc_draw_oval_bounds(
                        memory,
                        gworlds,
                        owner,
                        (
                            dot_top.saturating_add(3),
                            dot_left.saturating_add(3),
                            dot_bottom.saturating_sub(3),
                            dot_right.saturating_sub(3),
                        ),
                        ppc_theme_rgb(palette.frame_dark),
                        None,
                        false,
                    );
                }
                wrote
            }
            16 => {
                // Both CPU adapters submit the same ControlRecord state to the
                // architecture-neutral presentation provider. Only this final
                // guest-framebuffer blit remains adapter-specific.
                let value = memory
                    .read_u16_be(control + PPC_CONTROL_VALUE_OFFSET)
                    .unwrap_or(0) as i16;
                let min = memory
                    .read_u16_be(control + PPC_CONTROL_MIN_OFFSET)
                    .unwrap_or(0) as i16;
                let max = memory
                    .read_u16_be(control + PPC_CONTROL_MAX_OFFSET)
                    .unwrap_or(0) as i16;
                let hilite = memory
                    .read_u8(control + PPC_CONTROL_HILITE_OFFSET)
                    .unwrap_or(0);
                let bitmap = render_scrollbar_bitmap(
                    ppc_ui_theme(gworlds),
                    right.saturating_sub(left),
                    bottom.saturating_sub(top),
                    value,
                    min,
                    max,
                    hilite,
                );
                ppc_blit_theme_bitmap(memory, gworlds, owner, top, left, &bitmap)
            }
            1008..=1023 => {
                // The standard pop-up CDEF is resource ID 63, whose proc IDs
                // occupy 63 << 4 through 63 << 4 | 15. Its contrlMin field is
                // the MENU resource ID and contrlValue is the one-based item.
                let dialog_bounds = memory
                    .read_u16_be(owner + PPC_CWINDOW_WINDOW_KIND_OFFSET)
                    .filter(|kind| *kind == 2)
                    .and_then(|_| ppc_dialog_global_bounds(memory, gworlds, owner));
                if dialog_bounds.is_some() && !draw_dialog_popup {
                    return true;
                }
                // Draw in the owning port so its visible and clipping regions
                // also apply to controls outside or partly outside a dialog.
                // Macintosh Toolbox Essentials (1992), Display Rectangles, ch. 6.
                let (draw_owner, (draw_top, draw_left, draw_bottom, draw_right)) =
                    (owner, (top, left, bottom, right));
                // popupMenuProc reserves `contrlMax` pixels for the label before
                // the button. A fixed-width popup uses the rest of the control
                // rect as its stable button width; omitting this offset makes
                // the PPC renderer paint the button over its label and gives its
                // selected title an incorrectly large content area. Macintosh
                // Toolbox Essentials (1992), pp. 5-25--5-27.
                let title_width = record
                    .and_then(|record| record.popup_title_width)
                    .unwrap_or(0)
                    .max(0);
                let draw_left = draw_left.saturating_add(title_width);
                let draw_top = draw_top.saturating_add(1);
                let draw_bottom = draw_bottom.saturating_sub(2);
                let draw_right = draw_right.saturating_sub(1);
                let mut wrote;
                let enabled = memory
                    .read_u8(control + PPC_CONTROL_HILITE_OFFSET)
                    .unwrap_or(0)
                    != 255;
                if ppc_draw_themed_control_rect(
                    memory,
                    gworlds,
                    draw_owner,
                    (draw_top, draw_left, draw_bottom, draw_right),
                    crate::ui_theme::ControlKind::PopupButton,
                    enabled,
                    false,
                ) {
                    wrote = true;
                } else {
                    wrote = ppc_paint_rect_bounds(
                        memory,
                        gworlds,
                        draw_owner,
                        (draw_top, draw_left, draw_bottom, draw_right),
                        ppc_theme_rgb(palette.window_background),
                        None,
                    );
                    for (start, end) in [
                        (
                            (draw_left, draw_top),
                            (draw_right.saturating_sub(1), draw_top),
                        ),
                        (
                            (draw_right.saturating_sub(1), draw_top),
                            (draw_right.saturating_sub(1), draw_bottom.saturating_sub(1)),
                        ),
                        (
                            (draw_right.saturating_sub(1), draw_bottom.saturating_sub(1)),
                            (draw_left, draw_bottom.saturating_sub(1)),
                        ),
                        (
                            (draw_left, draw_bottom.saturating_sub(1)),
                            (draw_left, draw_top),
                        ),
                    ] {
                        wrote |= ppc_line_to(
                            memory,
                            gworlds,
                            draw_owner,
                            start,
                            end,
                            ppc_theme_rgb(palette.frame_dark),
                            None,
                        );
                    }
                    let arrow_left = draw_right.saturating_sub(18).max(draw_left);
                    wrote |= ppc_line_to(
                        memory,
                        gworlds,
                        draw_owner,
                        (arrow_left, draw_top),
                        (arrow_left, draw_bottom.saturating_sub(1)),
                        ppc_theme_rgb(palette.frame_dark),
                        None,
                    );
                    let arrow_h = draw_right.saturating_sub(9);
                    let center_v =
                        draw_top.saturating_add(draw_bottom.saturating_sub(draw_top) / 2);
                    for offset in 0..3i16 {
                        wrote |= ppc_line_to(
                            memory,
                            gworlds,
                            draw_owner,
                            (
                                arrow_h.saturating_sub(offset),
                                center_v.saturating_sub(3 - offset),
                            ),
                            (
                                arrow_h.saturating_add(offset),
                                center_v.saturating_sub(3 - offset),
                            ),
                            ppc_theme_rgb(palette.frame_dark),
                            None,
                        );
                        wrote |= ppc_line_to(
                            memory,
                            gworlds,
                            draw_owner,
                            (
                                arrow_h.saturating_sub(offset),
                                center_v.saturating_add(3 - offset),
                            ),
                            (
                                arrow_h.saturating_add(offset),
                                center_v.saturating_add(3 - offset),
                            ),
                            ppc_theme_rgb(palette.frame_dark),
                            None,
                        );
                    }
                }
                let selected = memory
                    .read_u16_be(control + PPC_CONTROL_VALUE_OFFSET)
                    .unwrap_or(0) as usize;
                let menu_id = record.map_or(0, |record| record.popup_menu_id);
                let selected_text = ppc_popup_control_selected_text(
                    memory,
                    vfs_resources,
                    current_resource_refnum,
                    menu_id,
                    selected,
                );
                let text_left = draw_left.saturating_add(5);
                let text_right = draw_right.saturating_sub(19).max(text_left);
                let display_title = ppc_popup_control_display_title(
                    &selected_text,
                    text_right.saturating_sub(text_left),
                    PPC_QD_TEXT_FONT_DEFAULT,
                    PPC_QD_TEXT_SIZE_SYSTEM,
                );
                if !display_title.is_empty() {
                    let _ = ppc_draw_text_bytes(
                        memory,
                        gworlds,
                        draw_owner,
                        (text_left, draw_top.saturating_add(14)),
                        PPC_QD_TEXT_FONT_DEFAULT,
                        PPC_QD_TEXT_SIZE_SYSTEM,
                        PPC_QD_TEXT_MODE_SRC_OR,
                        ppc_theme_rgb(palette.frame_dark),
                        None,
                        &display_title,
                    );
                }
                wrote
            }
            _ => ppc_frame_rect(
                &frame_cpu,
                memory,
                gworlds,
                owner,
                ppc_theme_rgb(palette.frame_dark),
                None,
                PPC_RGB_WHITE,
                &[0xff; 8],
            ),
        }
    };
    let title =
        ppc_read_pstring_bytes(memory, control + PPC_CONTROL_TITLE_OFFSET).unwrap_or_default();
    if !title.is_empty() {
        let title = title
            .into_iter()
            .flat_map(|byte| {
                if byte == 0xc9 {
                    vec![b'.', b'.', b'.']
                } else {
                    vec![byte]
                }
            })
            .collect::<Vec<_>>();
        let advance = ppc_text_bytes_advance_for_font(
            &title,
            PPC_QD_TEXT_FONT_DEFAULT,
            PPC_QD_TEXT_SIZE_SYSTEM,
        );
        let metrics = get_font_metrics(PPC_QD_TEXT_FONT_DEFAULT, PPC_QD_TEXT_SIZE_SYSTEM);
        let (centered_h, centered_v) = crate::control_manager::centered_control_label_origin(
            (top, left, bottom, right),
            advance,
            metrics.ascent,
            metrics.descent,
        );
        let popup = (1008..=1023).contains(&proc_id);
        let (title_h, title) = if popup {
            // Popup CDEF labels are right-aligned immediately before the
            // button's reserved title-width region, matching the 68K
            // draw_popup_control_label path. Keep the label out of the
            // selected-item content area.
            let title_width = record
                .and_then(|record| record.popup_title_width)
                .unwrap_or(0)
                .max(0);
            let popup_left = left.saturating_add(title_width);
            let text_right = popup_left.saturating_sub(6);
            let available_width = text_right.saturating_sub(left);
            (
                text_right.saturating_sub(advance).max(left),
                ppc_popup_control_display_title(
                    &title,
                    available_width,
                    PPC_QD_TEXT_FONT_DEFAULT,
                    PPC_QD_TEXT_SIZE_SYSTEM,
                ),
            )
        } else {
            let title_h = match proc_id {
                0 => centered_h,
                1 => {
                    crate::control_manager::standard_checkbox_layout((top, left, bottom, right))
                        .label_left
                }
                2 => {
                    crate::control_manager::standard_radio_button_layout((top, left, bottom, right))
                        .label_left
                }
                _ => left.saturating_add(16),
            };
            (title_h, title)
        };
        let title_v = centered_v.min(bottom.saturating_sub(1));
        let _ = ppc_draw_text_bytes(
            memory,
            gworlds,
            owner,
            (title_h, title_v),
            PPC_QD_TEXT_FONT_DEFAULT,
            PPC_QD_TEXT_SIZE_SYSTEM,
            PPC_QD_TEXT_MODE_SRC_OR,
            ppc_theme_rgb(palette.frame_dark),
            None,
            &title,
        );
    }
    framed
}

/// Group boxes, static text and sliders, drawn as the 68K path draws them
/// (`draw_group_box_control`, `draw_static_text_control`,
/// `draw_slider_control` in `trap/control.rs`).
fn ppc_draw_appearance_control(
    memory: &mut PpcSectionMem,
    gworlds: &[PpcGWorldRecord],
    owner: u32,
    handle: u32,
    control: u32,
    proc_id: i16,
    (top, left, bottom, right): (i16, i16, i16, i16),
) -> bool {
    let ink = ppc_theme_rgb(ppc_ui_theme(gworlds).provider().palette().frame_dark);
    let paper = PPC_RGB_WHITE;
    let line = |memory: &mut PpcSectionMem, from: (i16, i16), to: (i16, i16)| {
        ppc_line_to(memory, gworlds, owner, from, to, ink, None)
    };
    let border = |memory: &mut PpcSectionMem, (t, l, b, r): (i16, i16, i16, i16)| {
        if b > t && r > l {
            line(memory, (l, t), (r - 1, t));
            line(memory, (r - 1, t), (r - 1, b - 1));
            line(memory, (r - 1, b - 1), (l, b - 1));
            line(memory, (l, b - 1), (l, t));
        }
    };
    let title = ppc_read_pstring_bytes(memory, control + PPC_CONTROL_TITLE_OFFSET).unwrap_or_default();
    let metrics = get_font_metrics(PPC_QD_TEXT_FONT_DEFAULT, PPC_QD_TEXT_SIZE_SYSTEM);
    let text = |memory: &mut PpcSectionMem, pen: (i16, i16)| {
        ppc_draw_text_bytes(
            memory,
            gworlds,
            owner,
            pen,
            PPC_QD_TEXT_FONT_DEFAULT,
            PPC_QD_TEXT_SIZE_SYSTEM,
            PPC_QD_TEXT_MODE_SRC_OR,
            ink,
            None,
            &title,
        )
    };
    match proc_id {
        // Static text: the title wrapped into the control's rectangle, no
        // frame, no fill, in the font and justification its
        // ControlFontStyleRec asks for (flags 1 font, 2 face, 4 size, 0x40
        // just; Universal Interfaces 3.4.2 Controls.h). Cythera's slider
        // labels ask for the small system font, centred; Mac OS 8.5 draws
        // that font as Geneva 9 (measured from its Preferences window).
        288 => {
            if !title.is_empty() {
                let (mut font, mut size, mut just) =
                    (PPC_QD_TEXT_FONT_DEFAULT, PPC_QD_TEXT_SIZE_SYSTEM, 0);
                if let Some([flags, style_font, style_size, _, _, style_just]) =
                    super::appearance_controls::ppc_control_font_style(handle)
                {
                    if flags & 0x0001 != 0 {
                        match style_font {
                            // kControlFontSmallSystemFont and its bold.
                            -2 | -3 => (font, size) = (3, 9),
                            f if f > 0 => font = f,
                            _ => {}
                        }
                    }
                    if flags & 0x0004 != 0 && style_size > 0 {
                        size = style_size;
                    }
                    if flags & 0x0040 != 0 {
                        just = style_just;
                    }
                }
                ppc_draw_wrapped_text(
                    memory,
                    gworlds,
                    owner,
                    &title,
                    (top, left, bottom, right),
                    font,
                    size,
                    just,
                    PPC_QD_TEXT_MODE_SRC_OR,
                    ink,
                    None,
                );
            }
        }
        // A titled group box: a one-pixel frame whose top edge runs through
        // the title's midline, broken behind the title.
        160 | 161 => {
            let frame_top = top + (metrics.ascent + metrics.descent) / 2;
            if title.is_empty() {
                border(memory, (frame_top, left, bottom, right));
            } else {
                let text_left = left + 8;
                let width = text(memory, (text_left, top + metrics.ascent));
                line(memory, (left, frame_top), (left, bottom - 1));
                line(memory, (right - 1, frame_top), (right - 1, bottom - 1));
                line(memory, (left, bottom - 1), (right - 1, bottom - 1));
                line(memory, (left, frame_top), (text_left - 3, frame_top));
                line(memory, ((text_left + width + 3).min(right - 1), frame_top), (right - 1, frame_top));
            }
        }
        // A slider: a groove along the long axis, a tick per value when the
        // variant asks for them and the range is small, and the thumb at
        // the value.
        _ => {
            let read = |memory: &mut PpcSectionMem, offset: u32| {
                memory.read_u16_be(control + offset).unwrap_or(0) as i16
            };
            let (value, min, max) = (
                read(memory, PPC_CONTROL_VALUE_OFFSET),
                read(memory, PPC_CONTROL_MIN_OFFSET),
                read(memory, PPC_CONTROL_MAX_OFFSET),
            );
            let hilite = memory.read_u8(control + PPC_CONTROL_HILITE_OFFSET).unwrap_or(0);
            let inactive = hilite == 255 || min >= max;
            let fill = |memory: &mut PpcSectionMem, rect: (i16, i16, i16, i16)| {
                if rect.2 > rect.0 && rect.3 > rect.1 {
                    ppc_paint_rect_bounds(memory, gworlds, owner, rect, paper, None);
                }
            };
            fill(memory, (top, left, bottom, right));
            let vertical = bottom - top > right - left;
            let thumb = super::appearance_controls::PPC_SLIDER_THUMB_SIZE;
            let (start, end) = if vertical { (top, bottom) } else { (left, right) };
            let travel = (end - start - thumb).max(0);
            let range = i32::from(max) - i32::from(min);
            let relative = if range > 0 {
                (i32::from(value.clamp(min, max)) - i32::from(min)) * i32::from(travel) / range
            } else {
                0
            };
            let thumb_at = start + relative as i16;
            let (across_a, across_b) = if vertical {
                ((left + right) / 2 - 2, (left + right) / 2 + 2)
            } else {
                ((top + bottom) / 2 - 2, (top + bottom) / 2 + 2)
            };
            let groove = if vertical {
                (start + thumb / 2, across_a, end - thumb / 2, across_b)
            } else {
                (across_a, start + thumb / 2, across_b, end - thumb / 2)
            };
            border(memory, groove);
            if !inactive {
                if proc_id & 2 != 0 && range > 0 && range <= 16 {
                    for step in 0..=range {
                        let at = start + thumb / 2 + (step * i32::from(travel) / range) as i16;
                        if vertical {
                            line(memory, (across_b + 2, at), ((across_b + 5).min(right), at));
                        } else {
                            line(memory, (at, across_b + 2), (at, (across_b + 5).min(bottom)));
                        }
                    }
                }
                let knob = if vertical {
                    (thumb_at, left, thumb_at + thumb, right.min(across_b + 1).max(left + 4))
                } else {
                    (top, thumb_at, bottom.min(across_b + 1).max(top + 4), thumb_at + thumb)
                };
                fill(memory, knob);
                border(memory, knob);
                if vertical {
                    let middle = (knob.0 + knob.2) / 2;
                    line(memory, (knob.1 + 2, middle), (knob.3 - 3, middle));
                } else {
                    let middle = (knob.1 + knob.3) / 2;
                    line(memory, (middle, knob.0 + 2), (middle, knob.2 - 3));
                }
            }
        }
    }
    true
}
