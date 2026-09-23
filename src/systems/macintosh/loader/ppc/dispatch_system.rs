use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PpcSystemCompatibilityOperation {
    BuildDdPwds,
    CtbGetCtbVersion,
    CallComponentUpp,
    DiBadMount,
    DiLoad,
    DiUnload,
    Debugger,
    Dequeue,
    Enqueue,
    FindNextComponent,
    GetNextProcess,
    GetScript,
    GetScriptManagerVariable,
    GetScriptVariable,
    GetSysBeepVolume,
    IuCompString,
    IuDateString,
    InitCrm,
    InitCtbUtilities,
    KeyTranslate,
    LaunchApplication,
    LmGetCurApName,
    LmGetSysFontFam,
    LmGetSysFontSize,
    MidiAddPort,
    MidiRemovePort,
    MidiSignOut,
    MidiWritePacket,
    Munger,
    NmRemove,
    ObscureCursor,
    OpenDefaultComponent,
    ResetAlertStage,
    SetFrontProcess,
    StyledLineBreak,
    SystemEdit,
    TruncText,
    UpperString,
}

pub(crate) fn ppc_munger_compatibility(
    cpu: &PpcCpu,
    allocator: Option<&mut PpcProcessAllocatorView<'_>>,
    memory: &mut PpcSectionMem,
    heap_cursor: &mut u32,
    heap_limit: u32,
    last_mem_error: &mut i16,
    handles: &mut Vec<PpcHandleRecord>,
) -> u32 {
    let handle = cpu.gpr[3];
    let offset = cpu.gpr[4] as i32;
    let search_len = cpu.gpr[6] as i32;
    let replacement_len = cpu.gpr[8] as i32;
    if offset < 0 || search_len < 0 || replacement_len < 0 {
        return u32::MAX;
    }
    let Some(mut contents) = ppc_handle_bytes(memory, handles, handle) else {
        *last_mem_error = PPC_NIL_HANDLE_ERR;
        return u32::MAX;
    };
    let start = offset as usize;
    if start > contents.len() {
        return u32::MAX;
    }
    let Some(needle) = ppc_memory_read_bytes(memory, cpu.gpr[5], search_len as u32)
        .or_else(|| (search_len == 0).then(Vec::new))
    else {
        return u32::MAX;
    };
    let position = if needle.is_empty() {
        Some(start)
    } else {
        contents[start..]
            .windows(needle.len())
            .position(|window| window == needle)
            .map(|relative| start + relative)
    };
    let Some(position) = position else {
        return u32::MAX;
    };
    if cpu.gpr[7] == 0 && replacement_len == 0 {
        return u32::try_from(position).unwrap_or(u32::MAX);
    }
    let Some(replacement) = ppc_memory_read_bytes(memory, cpu.gpr[7], replacement_len as u32)
        .or_else(|| (replacement_len == 0).then(Vec::new))
    else {
        return u32::MAX;
    };
    contents.splice(position..position + needle.len(), replacement);
    let result = ppc_allocator_view_resize_handle(
        allocator,
        memory,
        heap_cursor,
        heap_limit,
        last_mem_error,
        handles,
        handle,
        u32::try_from(contents.len()).unwrap_or(u32::MAX),
    );
    *last_mem_error = result;
    if result != PPC_NO_ERR {
        return u32::MAX;
    }
    let Some(ptr) = memory.read_u32_be(handle) else {
        return u32::MAX;
    };
    if memory.write_bytes(ptr, &contents).is_none() {
        *last_mem_error = PPC_PARAM_ERR;
        return u32::MAX;
    }
    u32::try_from(position).unwrap_or(u32::MAX)
}

pub(crate) fn ppc_enqueue_compatibility(
    memory: &mut PpcSectionMem,
    element: u32,
    header: u32,
) -> bool {
    if element == 0
        || header == 0
        || !ppc_memory_can_write_bytes(memory, element, 4)
        || !ppc_memory_can_write_bytes(memory, header, 10)
    {
        return false;
    }
    let tail = memory.read_u32_be(header + 6).unwrap_or(0);
    let _ = memory.write_u32_be(element, 0);
    if tail == 0 {
        let _ = memory.write_u32_be(header + 2, element);
    } else {
        let _ = memory.write_u32_be(tail, element);
    }
    memory.write_u32_be(header + 6, element).is_some()
}

pub(crate) fn ppc_dequeue_compatibility(
    memory: &mut PpcSectionMem,
    element: u32,
    header: u32,
) -> i16 {
    if element == 0 || header == 0 || !ppc_memory_can_write_bytes(memory, header, 10) {
        return PPC_PARAM_ERR;
    }
    let mut previous = 0;
    let mut current = memory.read_u32_be(header + 2).unwrap_or(0);
    while current != 0 {
        let next = memory.read_u32_be(current).unwrap_or(0);
        if current == element {
            if previous == 0 {
                let _ = memory.write_u32_be(header + 2, next);
            } else {
                let _ = memory.write_u32_be(previous, next);
            }
            if memory.read_u32_be(header + 6) == Some(element) {
                let _ = memory.write_u32_be(header + 6, previous);
            }
            let _ = memory.write_u32_be(element, 0);
            return PPC_NO_ERR;
        }
        previous = current;
        current = next;
    }
    -1
}

#[allow(clippy::too_many_arguments)]
/// StyledLineBreak(textPtr, textLen, textStart, textEnd, flags, textWidth,
/// textOffset): Inside Macintosh: Text (1993), p. 5-79. Measures the
/// run from textStart in the current port's font; if it fits, returns
/// smBreakOverflow (2) with textOffset at textEnd, otherwise breaks after the
/// last space that fits (smBreakWord, 0), or inside the word when this is the
/// line's first run (smBreakChar, 1), and always past textStart: a caller
/// that loops until the offset moves would otherwise never stop. The same
/// choices as the 68K ScriptUtil selector.
pub(crate) fn ppc_styled_line_break(
    cpu: &PpcCpu,
    memory: &mut PpcSectionMem,
    font: i16,
    size: i16,
    style: u8,
) -> u8 {
    let text_ptr = cpu.gpr[3];
    let text_len = (cpu.gpr[4] as i32).max(0) as u32;
    let text_start = ((cpu.gpr[5] as i32).max(0) as u32).min(text_len);
    let text_end = ((cpu.gpr[6] as i32).max(0) as u32).max(text_start).min(text_len);
    let (width_ptr, offset_ptr) = (cpu.gpr[8], cpu.gpr[9]);
    let first_run_on_line = offset_ptr != 0 && memory.read_u32_be(offset_ptr).unwrap_or(0) != 0;
    let width_raw = if width_ptr != 0 {
        memory.read_u32_be(width_ptr).unwrap_or(0)
    } else {
        0x7FFF
    };
    // textWidth is Fixed; a value with no integer half is taken as pixels.
    let available = if width_raw & 0xFFFF_0000 != 0 {
        (width_raw as i32) >> 16
    } else {
        width_raw as i32
    }
    .max(0);
    let byte = |memory: &mut PpcSectionMem, offset: u32| {
        memory.read_u8(text_ptr.wrapping_add(offset)).unwrap_or(0)
    };
    let width_of = |memory: &mut PpcSectionMem, start: u32, end: u32| {
        let bytes: Vec<u8> = (start..end).map(|offset| byte(memory, offset)).collect();
        i32::from(ppc_text_width_bytes(font, size, style, &bytes))
    };
    let is_space = |value: u8| matches!(value, b' ' | b'\t' | b'\r' | b'\n');
    let run_width = width_of(memory, text_start, text_end);
    let (result, offset, consumed) = if run_width <= available {
        (2u8, text_end, run_width)
    } else {
        let mut fit = text_start;
        while fit < text_end && width_of(memory, text_start, fit + 1) <= available {
            fit += 1;
        }
        let mut word_break = None;
        let mut offset = text_start;
        while offset < fit {
            if is_space(byte(memory, offset)) {
                let mut after = offset + 1;
                while after < text_end && is_space(byte(memory, after)) {
                    after += 1;
                }
                word_break = Some(after);
                offset = after;
            } else {
                offset += 1;
            }
        }
        if let Some(word_offset) = word_break {
            (0, word_offset, width_of(memory, text_start, word_offset))
        } else {
            let char_offset = if first_run_on_line && fit > text_start {
                fit
            } else {
                (text_start + 1).min(text_end)
            };
            let code = if first_run_on_line { 1 } else { 0 };
            (code, char_offset, width_of(memory, text_start, char_offset))
        }
    };
    if offset_ptr != 0 {
        let _ = memory.write_u32_be(offset_ptr, offset);
    }
    if width_ptr != 0 {
        let remaining = available.saturating_sub(consumed).clamp(0, 0x7FFF);
        let value = if width_raw & 0xFFFF_0000 != 0 {
            (remaining << 16) as u32
        } else {
            remaining as u32
        };
        let _ = memory.write_u32_be(width_ptr, value);
    }
    result
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn ppc_dispatch_system_compatibility(
    operation: PpcSystemCompatibilityOperation,
    cpu: &mut PpcCpu,
    process_memory_manager: &mut ProcessNativeMemoryManager,
    memory: &mut PpcSectionMem,
    heap_cursor: &mut u32,
    heap_limit: u32,
    last_mem_error: &mut i16,
    handles: &mut Vec<PpcHandleRecord>,
    launched_app_path: Option<&str>,
) -> PpcImportAction {
    match operation {
        PpcSystemCompatibilityOperation::Munger => {
            let mut allocator = PpcProcessAllocatorView {
                memory_manager: process_memory_manager,
            };
            PpcImportAction::Return(ppc_munger_compatibility(
                cpu,
                Some(&mut allocator),
                memory,
                heap_cursor,
                heap_limit,
                last_mem_error,
                handles,
            ))
        }
        PpcSystemCompatibilityOperation::Enqueue => {
            let _ = ppc_enqueue_compatibility(memory, cpu.gpr[3], cpu.gpr[4]);
            PpcImportAction::ReturnPreserve
        }
        PpcSystemCompatibilityOperation::Dequeue => PpcImportAction::Return(ppc_i16_result(
            ppc_dequeue_compatibility(memory, cpu.gpr[3], cpu.gpr[4]),
        )),
        // ObscureCursor has no effect on the cursor level. It hides the cursor
        // only until the user moves the mouse, which the host injects each frame.
        // PROCEDURE ObscureCursor;
        // Inside Macintosh Volume I, I-168.
        PpcSystemCompatibilityOperation::ObscureCursor => PpcImportAction::ReturnPreserve,
        PpcSystemCompatibilityOperation::UpperString => {
            if let Some(bytes) = ppc_read_pstring_bytes(memory, cpu.gpr[3]) {
                let upper = bytes
                    .into_iter()
                    .map(|byte| byte.to_ascii_uppercase())
                    .collect::<Vec<_>>();
                let _ = ppc_write_pstring_bytes(memory, cpu.gpr[3], &upper);
            }
            PpcImportAction::ReturnPreserve
        }
        PpcSystemCompatibilityOperation::IuCompString => {
            let lhs = ppc_read_pstring_bytes(memory, cpu.gpr[3]).unwrap_or_default();
            let rhs = ppc_read_pstring_bytes(memory, cpu.gpr[4]).unwrap_or_default();
            let lhs = lhs
                .into_iter()
                .map(|byte| byte.to_ascii_uppercase())
                .collect::<Vec<_>>();
            let rhs = rhs
                .into_iter()
                .map(|byte| byte.to_ascii_uppercase())
                .collect::<Vec<_>>();
            let ordering = match lhs.cmp(&rhs) {
                std::cmp::Ordering::Less => -1i16,
                std::cmp::Ordering::Equal => 0,
                std::cmp::Ordering::Greater => 1,
            };
            PpcImportAction::Return(ppc_i16_result(ordering))
        }
        PpcSystemCompatibilityOperation::TruncText => {
            let width = usize::from(cpu.gpr[3] as u16);
            let text = cpu.gpr[4];
            let length_ptr = cpu.gpr[5];
            let Some(length) = memory.read_u16_be(length_ptr).map(usize::from) else {
                return PpcImportAction::Return(ppc_i16_result(-1));
            };
            let capacity = width / 6;
            if length <= capacity {
                return PpcImportAction::Return(0);
            }
            if capacity < 3 || !ppc_memory_can_write_bytes(memory, text, length as u32) {
                return PpcImportAction::Return(ppc_i16_result(-1));
            }
            let retained = capacity - 3;
            if cpu.gpr[6] & 0x4000 != 0 {
                let left = retained / 2;
                let right = retained - left;
                let tail = (0..right)
                    .filter_map(|index| memory.read_u8(text + (length - right + index) as u32))
                    .collect::<Vec<_>>();
                let _ = memory.write_bytes(text + left as u32, b"...");
                let _ = memory.write_bytes(text + left as u32 + 3, &tail);
            } else {
                let _ = memory.write_bytes(text + retained as u32, b"...");
            }
            let _ = memory.write_u16_be(length_ptr, capacity as u16);
            PpcImportAction::Return(1)
        }
        PpcSystemCompatibilityOperation::StyledLineBreak => {
            unreachable!("StyledLineBreak is served by ppc_styled_line_break")
        }
        PpcSystemCompatibilityOperation::GetNextProcess => {
            let psn = cpu.gpr[3];
            let current = ProcessSerialNumber::new(
                memory.read_u32_be(psn).unwrap_or(u32::MAX),
                memory.read_u32_be(psn + 4).unwrap_or(u32::MAX),
            );
            match current.next_single_process() {
                SingleProcessEnumeration::Current(current) => {
                    let _ = memory.write_u32_be(psn, current.high);
                    let _ = memory.write_u32_be(psn + 4, current.low);
                    PpcImportAction::Return(0)
                }
                SingleProcessEnumeration::End | SingleProcessEnumeration::Invalid => {
                    PpcImportAction::Return(ppc_i16_result(PPC_PROC_NOT_FOUND_ERR))
                }
            }
        }
        PpcSystemCompatibilityOperation::SetFrontProcess => PpcImportAction::Return(0),
        PpcSystemCompatibilityOperation::LmGetCurApName => {
            let name = launched_app_path
                .and_then(|path| path.rsplit('/').next())
                .unwrap_or("Systemless");
            let encoded = encode_mac_roman_lossy(name);
            let _ = ppc_write_pstring_bytes(memory, PPC_IMPORT_CUR_AP_NAME, &encoded);
            PpcImportAction::Return(PPC_IMPORT_CUR_AP_NAME)
        }
        PpcSystemCompatibilityOperation::LmGetSysFontFam => PpcImportAction::Return(0),
        PpcSystemCompatibilityOperation::LmGetSysFontSize => PpcImportAction::Return(12),
        PpcSystemCompatibilityOperation::GetSysBeepVolume => {
            let result = if memory.write_u32_be(cpu.gpr[3], 0x0100_0100).is_some() {
                PPC_NO_ERR
            } else {
                PPC_PARAM_ERR
            };
            PpcImportAction::Return(ppc_i16_result(result))
        }
        PpcSystemCompatibilityOperation::IuDateString => {
            let _ = ppc_write_pstring_bytes(memory, cpu.gpr[5], b"");
            PpcImportAction::ReturnPreserve
        }
        PpcSystemCompatibilityOperation::SystemEdit
        | PpcSystemCompatibilityOperation::DiBadMount => PpcImportAction::Return(0),
        PpcSystemCompatibilityOperation::FindNextComponent
        | PpcSystemCompatibilityOperation::OpenDefaultComponent => PpcImportAction::Return(0),
        PpcSystemCompatibilityOperation::CtbGetCtbVersion => PpcImportAction::Return(0x0200),
        PpcSystemCompatibilityOperation::GetScriptManagerVariable
        | PpcSystemCompatibilityOperation::GetScriptVariable
        | PpcSystemCompatibilityOperation::GetScript
        | PpcSystemCompatibilityOperation::KeyTranslate
        | PpcSystemCompatibilityOperation::CallComponentUpp => PpcImportAction::Return(0),
        PpcSystemCompatibilityOperation::LaunchApplication => {
            PpcImportAction::Return(ppc_i16_result(PPC_PROC_NOT_FOUND_ERR))
        }
        PpcSystemCompatibilityOperation::BuildDdPwds => PpcImportAction::ReturnPreserve,
        PpcSystemCompatibilityOperation::MidiAddPort
        | PpcSystemCompatibilityOperation::MidiRemovePort
        | PpcSystemCompatibilityOperation::MidiSignOut
        | PpcSystemCompatibilityOperation::MidiWritePacket => {
            PpcImportAction::Return(ppc_i16_result(PPC_NOT_ENOUGH_HARDWARE_ERR))
        }
        PpcSystemCompatibilityOperation::ResetAlertStage => {
            let stage = crate::dialog_manager::evaluate_reset_alert_stage();
            let _ = memory.write_u16_be(crate::memory::globals::addr::ALERT_STAGE, stage);
            PpcImportAction::ReturnPreserve
        }
        PpcSystemCompatibilityOperation::DiLoad
        | PpcSystemCompatibilityOperation::DiUnload
        | PpcSystemCompatibilityOperation::Debugger
        | PpcSystemCompatibilityOperation::InitCrm
        | PpcSystemCompatibilityOperation::InitCtbUtilities
        | PpcSystemCompatibilityOperation::NmRemove => PpcImportAction::ReturnPreserve,
    }
}
