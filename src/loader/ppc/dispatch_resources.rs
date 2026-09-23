use super::*;

pub(super) struct PpcResourceDispatchContext<'a> {
    pub(super) binding: &'a PpcImportBinding,
    pub(super) cpu: &'a mut PpcCpu,
    pub(super) memory: &'a mut PpcSectionMem,
    pub(super) process_memory_manager: &'a mut ProcessNativeMemoryManager,
    pub(super) heap_cursor: &'a mut u32,
    pub(super) heap_limit: u32,
    pub(super) last_mem_error: &'a mut i16,
    pub(super) handles: &'a mut Vec<PpcHandleRecord>,
    pub(super) resource_files: &'a mut Vec<PpcResourceFileRecord>,
    pub(super) vfs_resource_files: &'a mut ProcessVfsResourceFileRecords,
    pub(super) vfs_resources: &'a mut Vec<PpcVfsResourceRecord>,
    pub(super) current_resource_refnum: &'a mut i16,
    pub(super) resource_policy: &'a SharedProcessResourcePolicy,
    pub(super) last_resource_error: &'a mut i16,
}

pub(super) fn dispatch_resource_import(
    context: PpcResourceDispatchContext<'_>,
) -> Option<PpcImportAction> {
    let PpcResourceDispatchContext {
        binding,
        cpu,
        memory,
        process_memory_manager,
        heap_cursor,
        heap_limit,
        last_mem_error,
        handles,
        resource_files,
        vfs_resource_files,
        vfs_resources,
        current_resource_refnum,
        resource_policy,
        last_resource_error,
    } = context;

    match binding.dispatcher_target {
        PpcImportDispatcherTarget::SetResLoad => {
            // Inside Macintosh Volume I (1985), I-118: SetResLoad controls
            // whether subsequent Resource Manager lookups load resource data.
            resource_policy.set_res_load(cpu.gpr[3] != 0);
            Some(PpcImportAction::ReturnPreserve)
        }
        PpcImportDispatcherTarget::LoadResource => {
            // Inside Macintosh Volume I (1985), I-120: LoadResource fills an
            // empty resource handle and reports resNotFound for other handles.
            let handle = cpu.gpr[3];
            if let Some(index) = vfs_resources
                .iter()
                .position(|resource| resource.handle == handle)
            {
                let _ = ppc_materialize_vfs_resource_handle(
                    process_memory_manager,
                    memory,
                    heap_cursor,
                    heap_limit,
                    last_mem_error,
                    handles,
                    vfs_resources,
                    index,
                    true,
                    last_resource_error,
                );
            } else {
                *last_resource_error = PPC_RES_NOT_FOUND_ERR;
            }
            Some(PpcImportAction::ReturnPreserve)
        }
        PpcImportDispatcherTarget::GetResource => {
            if ppc_hle_trace_enabled() {
                eprintln!(
                    "[PPC-TRACE] GetResource type='{}' id={} current_refnum={}",
                    format_ppc_fourcc(cpu.gpr[3]),
                    cpu.gpr[4] as i16,
                    *current_resource_refnum
                );
            }
            Some(PpcImportAction::Return(ppc_get_resource(
                cpu,
                process_memory_manager,
                memory,
                heap_cursor,
                heap_limit,
                last_mem_error,
                handles,
                vfs_resources,
                *current_resource_refnum,
                false,
                resource_policy.res_load(),
                last_resource_error,
            )))
        }
        PpcImportDispatcherTarget::Get1Resource => {
            if ppc_hle_trace_enabled() {
                eprintln!(
                    "[PPC-TRACE] Get1Resource type='{}' id={} current_refnum={}",
                    format_ppc_fourcc(cpu.gpr[3]),
                    cpu.gpr[4] as i16,
                    *current_resource_refnum
                );
            }
            Some(PpcImportAction::Return(ppc_get_resource(
                cpu,
                process_memory_manager,
                memory,
                heap_cursor,
                heap_limit,
                last_mem_error,
                handles,
                vfs_resources,
                *current_resource_refnum,
                true,
                resource_policy.res_load(),
                last_resource_error,
            )))
        }
        PpcImportDispatcherTarget::GetNamedResource
        | PpcImportDispatcherTarget::Get1NamedResource => {
            let current_only = matches!(
                binding.dispatcher_target,
                PpcImportDispatcherTarget::Get1NamedResource
            );
            Some(PpcImportAction::Return(ppc_get_named_resource(
                cpu,
                process_memory_manager,
                memory,
                heap_cursor,
                heap_limit,
                last_mem_error,
                handles,
                vfs_resources,
                *current_resource_refnum,
                current_only,
                resource_policy.res_load(),
                last_resource_error,
            )))
        }
        PpcImportDispatcherTarget::GetIndResource | PpcImportDispatcherTarget::Get1IndResource => {
            let current_only = matches!(
                binding.dispatcher_target,
                PpcImportDispatcherTarget::Get1IndResource
            );
            Some(PpcImportAction::Return(ppc_get_ind_resource(
                cpu,
                process_memory_manager,
                memory,
                heap_cursor,
                heap_limit,
                last_mem_error,
                handles,
                vfs_resources,
                *current_resource_refnum,
                current_only,
                resource_policy.res_load(),
                last_resource_error,
            )))
        }
        PpcImportDispatcherTarget::CountResources => Some(PpcImportAction::Return(ppc_i16_result(
            ppc_count_resources(
                cpu,
                vfs_resources,
                *current_resource_refnum,
                false,
                last_resource_error,
            ),
        ))),
        PpcImportDispatcherTarget::Count1Resources => Some(PpcImportAction::Return(
            ppc_i16_result(ppc_count_resources(
                cpu,
                vfs_resources,
                *current_resource_refnum,
                true,
                last_resource_error,
            )),
        )),
        PpcImportDispatcherTarget::UniqueID => {
            Some(PpcImportAction::Return(ppc_i16_result(ppc_unique_id(
                cpu,
                vfs_resources,
                *current_resource_refnum,
                false,
                last_resource_error,
            ))))
        }
        PpcImportDispatcherTarget::Unique1ID => {
            Some(PpcImportAction::Return(ppc_i16_result(ppc_unique_id(
                cpu,
                vfs_resources,
                *current_resource_refnum,
                true,
                last_resource_error,
            ))))
        }
        PpcImportDispatcherTarget::ReleaseResource => {
            ppc_release_resource(
                cpu,
                process_memory_manager,
                memory,
                heap_cursor,
                heap_limit,
                last_mem_error,
                handles,
                vfs_resources,
                last_resource_error,
            );
            Some(PpcImportAction::ReturnPreserve)
        }
        PpcImportDispatcherTarget::DetachResource => {
            ppc_detach_resource(
                cpu,
                process_memory_manager,
                vfs_resources,
                last_resource_error,
            );
            Some(PpcImportAction::ReturnPreserve)
        }
        PpcImportDispatcherTarget::GetIndString => {
            ppc_get_ind_string(
                cpu,
                process_memory_manager,
                memory,
                heap_cursor,
                heap_limit,
                last_mem_error,
                handles,
                vfs_resources,
                *current_resource_refnum,
                last_resource_error,
            );
            Some(PpcImportAction::ReturnPreserve)
        }
        // FUNCTION GetString (stringID: Integer): StringHandle;
        // Inside Macintosh: Text (1993), 5-49 (lines 15621-15642).
        // GetString is the Text Utilities wrapper around
        // GetResource('STR ', stringID), including its NIL-on-miss behavior.
        PpcImportDispatcherTarget::GetString => {
            let string_id = cpu.gpr[3];
            cpu.gpr[3] = u32::from_be_bytes(*b"STR ");
            cpu.gpr[4] = string_id;
            Some(PpcImportAction::Return(ppc_get_resource(
                cpu,
                process_memory_manager,
                memory,
                heap_cursor,
                heap_limit,
                last_mem_error,
                handles,
                vfs_resources,
                *current_resource_refnum,
                false,
                resource_policy.res_load(),
                last_resource_error,
            )))
        }
        PpcImportDispatcherTarget::GetResAttrs => Some(PpcImportAction::Return(ppc_i16_result(
            ppc_get_res_attrs(cpu, vfs_resources, last_resource_error),
        ))),
        PpcImportDispatcherTarget::SetResAttrs => {
            ppc_set_res_attrs(cpu, vfs_resource_files, vfs_resources, last_resource_error);
            Some(PpcImportAction::ReturnPreserve)
        }
        PpcImportDispatcherTarget::GetResInfo => {
            ppc_get_res_info(cpu, memory, vfs_resources, last_resource_error);
            Some(PpcImportAction::ReturnPreserve)
        }
        PpcImportDispatcherTarget::GetResourceSizeOnDisk => {
            // More Macintosh Toolbox (1993), 1-105: this reports the exact
            // on-disk resource size even when SetResLoad left its handle empty.
            let size = vfs_resources
                .iter()
                .find(|resource| resource.handle == cpu.gpr[3])
                .and_then(|resource| i32::try_from(resource.data.len()).ok());
            if let Some(size) = size {
                *last_resource_error = PPC_NO_ERR;
                Some(PpcImportAction::Return(size as u32))
            } else {
                *last_resource_error = PPC_RES_NOT_FOUND_ERR;
                Some(PpcImportAction::Return(u32::MAX))
            }
        }
        PpcImportDispatcherTarget::SetResInfo => {
            ppc_set_res_info(
                cpu,
                memory,
                vfs_resource_files,
                vfs_resources,
                last_resource_error,
            );
            Some(PpcImportAction::ReturnPreserve)
        }
        PpcImportDispatcherTarget::HomeResFile => Some(PpcImportAction::Return(ppc_i16_result(
            ppc_home_res_file(cpu, vfs_resources, last_resource_error),
        ))),
        PpcImportDispatcherTarget::UpdateResFile => {
            ppc_update_res_file(
                cpu,
                memory,
                handles,
                resource_files,
                vfs_resource_files,
                vfs_resources,
                *current_resource_refnum,
                last_resource_error,
            );
            Some(PpcImportAction::ReturnPreserve)
        }
        PpcImportDispatcherTarget::AddResource => {
            *last_resource_error = ppc_add_resource(
                cpu,
                process_memory_manager,
                memory,
                handles,
                resource_files,
                vfs_resource_files,
                vfs_resources,
                *current_resource_refnum,
            );
            Some(PpcImportAction::ReturnPreserve)
        }
        PpcImportDispatcherTarget::ChangedResource => {
            ppc_changed_resource(cpu, vfs_resource_files, vfs_resources, last_resource_error);
            Some(PpcImportAction::ReturnPreserve)
        }
        PpcImportDispatcherTarget::WriteResource => {
            ppc_write_resource(
                cpu,
                memory,
                handles,
                vfs_resource_files,
                vfs_resources,
                last_resource_error,
            );
            Some(PpcImportAction::ReturnPreserve)
        }
        PpcImportDispatcherTarget::RemoveResource => {
            ppc_remove_resource(
                cpu,
                process_memory_manager,
                vfs_resource_files,
                vfs_resources,
                *current_resource_refnum,
                last_resource_error,
            );
            Some(PpcImportAction::ReturnPreserve)
        }
        PpcImportDispatcherTarget::ReadPartialResource => {
            ppc_read_partial_resource(cpu, memory, vfs_resources, last_resource_error);
            Some(PpcImportAction::ReturnPreserve)
        }
        PpcImportDispatcherTarget::CloseResFile => {
            ppc_close_res_file(
                cpu,
                process_memory_manager,
                memory,
                heap_cursor,
                heap_limit,
                last_mem_error,
                handles,
                resource_files,
                vfs_resource_files,
                vfs_resources,
                current_resource_refnum,
                last_resource_error,
            );
            Some(PpcImportAction::ReturnPreserve)
        }
        PpcImportDispatcherTarget::GetPicture => {
            let picture = ppc_get_picture(
                cpu,
                process_memory_manager,
                memory,
                heap_cursor,
                heap_limit,
                last_mem_error,
                last_resource_error,
                handles,
                vfs_resources,
                *current_resource_refnum,
            );
            Some(PpcImportAction::Return(picture))
        }
        PpcImportDispatcherTarget::GetIconSuite => {
            let result = ppc_get_icon_suite(
                cpu,
                process_memory_manager,
                memory,
                heap_cursor,
                heap_limit,
                last_mem_error,
                handles,
                vfs_resources,
                *current_resource_refnum,
                resource_policy.res_load(),
                last_resource_error,
            );
            Some(PpcImportAction::Return(ppc_i16_result(result)))
        }
        PpcImportDispatcherTarget::GetIcon | PpcImportDispatcherTarget::GetPattern => {
            let resource_id = cpu.gpr[3];
            cpu.gpr[3] = if matches!(
                binding.dispatcher_target,
                PpcImportDispatcherTarget::GetIcon
            ) {
                u32::from_be_bytes(*b"ICON")
            } else {
                u32::from_be_bytes(*b"PAT ")
            };
            cpu.gpr[4] = resource_id;
            Some(PpcImportAction::Return(ppc_get_resource(
                cpu,
                process_memory_manager,
                memory,
                heap_cursor,
                heap_limit,
                last_mem_error,
                handles,
                vfs_resources,
                *current_resource_refnum,
                false,
                resource_policy.res_load(),
                last_resource_error,
            )))
        }
        PpcImportDispatcherTarget::GetIndPattern => {
            ppc_get_ind_pattern(
                cpu,
                memory,
                vfs_resources,
                *current_resource_refnum,
                last_resource_error,
            );
            Some(PpcImportAction::ReturnPreserve)
        }
        // Macintosh Toolbox Essentials (1992), 5-97 and 4-99: a control or
        // window without colours of its own has the default auxiliary
        // record, whose colour table is the standard one; both calls then
        // return it and TRUE. One shared default record serves every caller.
        PpcImportDispatcherTarget::GetAuxiliaryControlRecord
        | PpcImportDispatcherTarget::GetAuxWin => {
            let window = binding.dispatcher_target == PpcImportDispatcherTarget::GetAuxWin;
            let record = ppc_default_aux_record(
                window,
                process_memory_manager,
                memory,
                heap_cursor,
                last_mem_error,
                handles,
            );
            let out = cpu.gpr[4];
            if out != 0 {
                let _ = memory.write_u32_be(out, record);
            }
            Some(PpcImportAction::Return(u32::from(record != 0)))
        }
        // Mac OS 8 Menu Manager: an item's command ID comes from the menu's
        // 'xmnu' resource. After a version word and an item count, each item
        // has an entry: a bare zero word for one without a command, otherwise
        // 30 bytes whose command ID is the long at +2 -- the parse the 68K
        // path uses for Cythera's 'xmnu' 129 (Save 5, Quit 10). Zero means no
        // command ID.
        PpcImportDispatcherTarget::GetMenuItemCommandID => {
            let item = cpu.gpr[4] as u16 as i16;
            let menu_id = memory
                .read_u32_be(cpu.gpr[3])
                .filter(|ptr| *ptr != 0)
                .and_then(|menu| memory.read_u16_be(menu))
                .map(|id| id as i16);
            let xmnu = u32::from_be_bytes(*b"xmnu");
            let command = menu_id
                .and_then(|menu_id| {
                    vfs_resources
                        .iter()
                        .find(|record| record.res_type == xmnu && record.res_id == menu_id)
                })
                .and_then(|record| {
                    let data = &record.data;
                    let word = |at: usize| data.get(at..at + 2).map(|b| u16::from_be_bytes([b[0], b[1]]));
                    let count = word(2)?.min(255);
                    let mut offset = 4usize;
                    for index in 1..=count {
                        if word(offset)? == 0 {
                            offset += 2;
                            continue;
                        }
                        if index as i16 == item {
                            let b = data.get(offset + 2..offset + 6)?;
                            return Some(u32::from_be_bytes([b[0], b[1], b[2], b[3]]));
                        }
                        offset += 30;
                    }
                    None
                })
                .unwrap_or(0);
            if cpu.gpr[5] != 0 {
                let _ = memory.write_u32_be(cpu.gpr[5], command);
            }
            Some(PpcImportAction::Return(0))
        }
        PpcImportDispatcherTarget::NewPixPat => Some(PpcImportAction::Return(ppc_new_pix_pat(
            process_memory_manager,
            memory,
            heap_cursor,
            last_mem_error,
            handles,
        ))),
        // Imaging With QuickDraw (1994), 4-102: PixPatChanged marks the
        // expanded pattern stale, which is patXValid = -1; nothing is cached
        // here that would need rebuilding.
        PpcImportDispatcherTarget::PixPatChanged => {
            if let Some(pattern) = memory.read_u32_be(cpu.gpr[3]).filter(|ptr| *ptr != 0) {
                let _ = memory.write_u16_be(pattern.wrapping_add(14), 0xFFFF);
            }
            Some(PpcImportAction::ReturnPreserve)
        }
        // Imaging With QuickDraw (1994), 4-101: DisposePixPat releases the
        // pattern and its PixMap, data and expanded-data handles. The handles
        // are left to the heap here, as GetPixPat's copies are.
        PpcImportDispatcherTarget::DisposePixPat => Some(PpcImportAction::ReturnPreserve),
        PpcImportDispatcherTarget::GetPixPat => Some(PpcImportAction::Return(ppc_get_pix_pat(
            cpu,
            process_memory_manager,
            memory,
            heap_cursor,
            last_mem_error,
            handles,
            vfs_resources,
            *current_resource_refnum,
            last_resource_error,
        ))),
        PpcImportDispatcherTarget::GetIntlResource => {
            // Inside Macintosh: Text (1993), 6-90 through 6-91: selectors
            // 0, 1, 2, 4, and 5 select the current script's itl resources.
            let Some(last_byte) = (cpu.gpr[3] as u16 as i16)
                .try_into()
                .ok()
                .filter(|selector: &u8| matches!(*selector, 0 | 1 | 2 | 4 | 5))
            else {
                *last_resource_error = PPC_RES_NOT_FOUND_ERR;
                return Some(PpcImportAction::Return(0));
            };
            cpu.gpr[3] = u32::from_be_bytes([b'i', b't', b'l', b'0' + last_byte]);
            cpu.gpr[4] = 0;
            Some(PpcImportAction::Return(ppc_get_resource(
                cpu,
                process_memory_manager,
                memory,
                heap_cursor,
                heap_limit,
                last_mem_error,
                handles,
                vfs_resources,
                *current_resource_refnum,
                false,
                resource_policy.res_load(),
                last_resource_error,
            )))
        }
        _ => None,
    }
}

fn ppc_get_picture(
    cpu: &mut PpcCpu,
    process_memory_manager: &mut ProcessNativeMemoryManager,
    memory: &mut PpcSectionMem,
    heap_cursor: &mut u32,
    _heap_limit: u32,
    last_mem_error: &mut i16,
    last_resource_error: &mut i16,
    handles: &mut Vec<PpcHandleRecord>,
    vfs_resources: &mut [PpcVfsResourceRecord],
    current_resource_refnum: i16,
) -> u32 {
    let picture_id = cpu.gpr[3] as u16 as i16;
    let pict_type = u32::from_be_bytes(*b"PICT");
    if let Some(index) = ppc_vfs_resource_index(
        vfs_resources,
        current_resource_refnum,
        pict_type,
        picture_id,
        false,
    ) {
        if vfs_resources[index].handle == 0 {
            let data = vfs_resources[index].data.clone();
            let handle = ppc_process_alloc_handle_with_bytes(
                process_memory_manager,
                memory,
                heap_cursor,
                last_mem_error,
                handles,
                &data,
            );
            if handle == 0 {
                *last_mem_error = PPC_MEM_FULL_ERR;
                *last_resource_error = PPC_RES_NOT_FOUND_ERR;
                return 0;
            }
            vfs_resources[index].handle = handle;
        }
        *last_mem_error = PPC_NO_ERR;
        *last_resource_error = PPC_NO_ERR;
        if ppc_hle_trace_enabled() {
            let record = &vfs_resources[index];
            eprintln!(
                "[PPC-TRACE] GetPicture({}) current_ref={} -> handle=${:08X} home_ref={} path=\"{}\" size={}",
                picture_id,
                current_resource_refnum,
                record.handle,
                record.ref_num,
                record.path,
                record.data.len()
            );
        }
        return vfs_resources[index].handle;
    }
    let handle = ppc_process_alloc_handle_with_bytes(
        process_memory_manager,
        memory,
        heap_cursor,
        last_mem_error,
        handles,
        &minimal_pict_bytes(),
    );
    if handle == 0 {
        *last_mem_error = PPC_MEM_FULL_ERR;
        *last_resource_error = PPC_RES_NOT_FOUND_ERR;
    } else {
        *last_mem_error = PPC_NO_ERR;
        *last_resource_error = PPC_RES_NOT_FOUND_ERR;
    }
    if ppc_hle_trace_enabled() {
        eprintln!(
            "[PPC-TRACE] GetPicture({}) current_ref={} -> fallback handle=${:08X} err={}",
            picture_id, current_resource_refnum, handle, PPC_RES_NOT_FOUND_ERR
        );
    }
    handle
}

fn ppc_get_ind_pattern(
    cpu: &PpcCpu,
    memory: &mut PpcSectionMem,
    vfs_resources: &[PpcVfsResourceRecord],
    current_resource_refnum: i16,
    last_resource_error: &mut i16,
) {
    let destination = cpu.gpr[3];
    let pattern_list_id = cpu.gpr[4] as u16 as i16;
    let pattern_index = cpu.gpr[5] as u16 as usize;
    if destination == 0 || pattern_index == 0 || !ppc_memory_can_write_bytes(memory, destination, 8)
    {
        *last_resource_error = PPC_PARAM_ERR;
        return;
    }
    let Some(resource_index) = ppc_vfs_resource_index(
        vfs_resources,
        current_resource_refnum,
        u32::from_be_bytes(*b"PAT#"),
        pattern_list_id,
        false,
    ) else {
        *last_resource_error = PPC_RES_NOT_FOUND_ERR;
        return;
    };
    let bytes = &vfs_resources[resource_index].data;
    let count = bytes
        .get(..2)
        .and_then(|bytes| bytes.try_into().ok())
        .map(u16::from_be_bytes)
        .unwrap_or(0) as usize;
    let pattern_offset = pattern_index
        .checked_sub(1)
        .and_then(|index| index.checked_mul(8))
        .and_then(|offset| offset.checked_add(2));
    let Some(pattern) = pattern_offset
        .filter(|_| pattern_index <= count)
        .and_then(|offset| bytes.get(offset..offset.saturating_add(8)))
    else {
        *last_resource_error = PPC_RES_NOT_FOUND_ERR;
        return;
    };
    // Imaging With QuickDraw (1994), pp. 3-127--3-128 and 3-141: PAT#
    // begins with a big-endian count followed by packed eight-byte Pattern
    // records, addressed with one-based indices by GetIndPattern.
    *last_resource_error = if memory.write_bytes(destination, pattern).is_some() {
        PPC_NO_ERR
    } else {
        PPC_PARAM_ERR
    };
}

thread_local! {
    static DEFAULT_AUX_RECORDS: std::cell::RefCell<[u32; 2]> = const { std::cell::RefCell::new([0; 2]) };
}

/// The default AuxCtlRec (22 bytes, acCTable at +8) or AuxWinRec (28 bytes,
/// awCTable at +8), each with a colour table of the standard parts: a
/// control's frame, body, text and thumb; a window's content, frame, text,
/// hilite and title bar.
fn ppc_default_aux_record(
    window: bool,
    process_memory_manager: &mut ProcessNativeMemoryManager,
    memory: &mut PpcSectionMem,
    heap_cursor: &mut u32,
    last_mem_error: &mut i16,
    handles: &mut Vec<PpcHandleRecord>,
) -> u32 {
    let slot = usize::from(window);
    let existing = DEFAULT_AUX_RECORDS.with(|records| records.borrow()[slot]);
    if existing != 0 && memory.read_u32_be(existing).is_some_and(|ptr| ptr != 0) {
        return existing;
    }
    let black = [0u16, 0, 0];
    let white = [0xFFFFu16, 0xFFFF, 0xFFFF];
    let parts: &[[u16; 3]] = if window {
        &[white, black, black, black, white]
    } else {
        &[black, white, black, white]
    };
    let mut table = Vec::new();
    table.extend_from_slice(&0u32.to_be_bytes());
    table.extend_from_slice(&0u16.to_be_bytes());
    table.extend_from_slice(&((parts.len() - 1) as u16).to_be_bytes());
    for (index, rgb) in parts.iter().enumerate() {
        table.extend_from_slice(&(index as u16).to_be_bytes());
        for component in rgb {
            table.extend_from_slice(&component.to_be_bytes());
        }
    }
    let table = ppc_process_alloc_handle_with_bytes(
        process_memory_manager,
        memory,
        heap_cursor,
        last_mem_error,
        handles,
        &table,
    );
    let mut record = vec![0u8; if window { 28 } else { 22 }];
    record[8..12].copy_from_slice(&table.to_be_bytes());
    let record = ppc_process_alloc_handle_with_bytes(
        process_memory_manager,
        memory,
        heap_cursor,
        last_mem_error,
        handles,
        &record,
    );
    if table == 0 || record == 0 {
        return 0;
    }
    DEFAULT_AUX_RECORDS.with(|records| records.borrow_mut()[slot] = record);
    record
}

/// NewPixPat: a pattern built at run time, with real handles in its patMap,
/// patData, patXData and patXMap fields rather than the resource offsets a
/// `GetPixPat` copy keeps. Imaging With QuickDraw (1994), 4-100 to 4-101:
/// patType 1 (full colour), a PixMap like NewPixMap's, an empty data handle
/// the application sizes and fills, patXValid -1, pat1Data 50% grey.
/// PixPat: patType 0, patMap 2, patData 6, patXData 10, patXValid 14,
/// patXMap 16, pat1Data 20.
fn ppc_new_pix_pat(
    process_memory_manager: &mut ProcessNativeMemoryManager,
    memory: &mut PpcSectionMem,
    heap_cursor: &mut u32,
    last_mem_error: &mut i16,
    handles: &mut Vec<PpcHandleRecord>,
) -> u32 {
    let mut alloc = |bytes: &[u8], memory: &mut PpcSectionMem| {
        ppc_process_alloc_handle_with_bytes(
            process_memory_manager,
            memory,
            heap_cursor,
            last_mem_error,
            handles,
            bytes,
        )
    };
    // ColorTable: ctSeed, ctFlags, ctSize -1 (no entries).
    let mut color_table = Vec::new();
    color_table.extend_from_slice(&0u32.to_be_bytes());
    color_table.extend_from_slice(&0u16.to_be_bytes());
    color_table.extend_from_slice(&0xFFFFu16.to_be_bytes());
    let table = alloc(&color_table, memory);
    // PixMap, 50 bytes: rowBytes with the PixMap flag, 72 dpi, 8-bit
    // chunky indexed, one component of 8 bits, the table above.
    let mut pix_map = vec![0u8; 50];
    pix_map[4..6].copy_from_slice(&0x8000u16.to_be_bytes());
    pix_map[22..26].copy_from_slice(&0x0048_0000u32.to_be_bytes());
    pix_map[26..30].copy_from_slice(&0x0048_0000u32.to_be_bytes());
    pix_map[32..34].copy_from_slice(&8u16.to_be_bytes());
    pix_map[34..36].copy_from_slice(&1u16.to_be_bytes());
    pix_map[36..38].copy_from_slice(&8u16.to_be_bytes());
    pix_map[42..46].copy_from_slice(&table.to_be_bytes());
    let pat_map = alloc(&pix_map, memory);
    let pat_data = alloc(&[], memory);
    let pat_x_data = alloc(&[], memory);
    let pat_x_map = alloc(&vec![0u8; 50], memory);
    let mut pattern = vec![0u8; 28];
    pattern[0..2].copy_from_slice(&1u16.to_be_bytes());
    pattern[2..6].copy_from_slice(&pat_map.to_be_bytes());
    pattern[6..10].copy_from_slice(&pat_data.to_be_bytes());
    pattern[10..14].copy_from_slice(&pat_x_data.to_be_bytes());
    pattern[14..16].copy_from_slice(&0xFFFFu16.to_be_bytes());
    pattern[16..20].copy_from_slice(&pat_x_map.to_be_bytes());
    pattern[20..28].copy_from_slice(&[0xAA, 0x55, 0xAA, 0x55, 0xAA, 0x55, 0xAA, 0x55]);
    let handle = alloc(&pattern, memory);
    if handle == 0 || pat_map == 0 || pat_data == 0 || table == 0 {
        *last_mem_error = PPC_MEM_FULL_ERR;
        return 0;
    }
    *last_mem_error = PPC_NO_ERR;
    handle
}

fn ppc_get_pix_pat(
    cpu: &PpcCpu,
    process_memory_manager: &mut ProcessNativeMemoryManager,
    memory: &mut PpcSectionMem,
    heap_cursor: &mut u32,
    last_mem_error: &mut i16,
    handles: &mut Vec<PpcHandleRecord>,
    vfs_resources: &[PpcVfsResourceRecord],
    current_resource_refnum: i16,
    last_resource_error: &mut i16,
) -> u32 {
    let pattern_id = cpu.gpr[3] as u16 as i16;
    let pattern_type = u32::from_be_bytes(*b"ppat");
    let Some(index) = ppc_vfs_resource_index(
        vfs_resources,
        current_resource_refnum,
        pattern_type,
        pattern_id,
        false,
    ) else {
        *last_resource_error = PPC_RES_NOT_FOUND_ERR;
        if ppc_hle_trace_enabled() {
            eprintln!(
                "[PPC-TRACE] GetPixPat({}) current_ref={} -> NULL err={}",
                pattern_id, current_resource_refnum, PPC_RES_NOT_FOUND_ERR
            );
        }
        return 0;
    };

    // Imaging With QuickDraw (1994), pp. 4-88 and 4-103: GetPixPat obtains
    // the requested 'ppat' resource, then returns a newly allocated copy of
    // the compiled PixPat/PixMap/image/ColorTable compound structure. Keeping
    // its documented offsets intact also lets Color QuickDraw consume the
    // pattern without tying its lifetime to the Resource Manager's handle.
    let data = &vfs_resources[index].data;
    let handle = ppc_process_alloc_handle_with_bytes(
        process_memory_manager,
        memory,
        heap_cursor,
        last_mem_error,
        handles,
        data,
    );
    if handle == 0 {
        *last_mem_error = PPC_MEM_FULL_ERR;
        *last_resource_error = PPC_RES_NOT_FOUND_ERR;
        return 0;
    }
    *last_mem_error = PPC_NO_ERR;
    *last_resource_error = PPC_NO_ERR;
    if ppc_hle_trace_enabled() {
        let record = &vfs_resources[index];
        eprintln!(
            "[PPC-TRACE] GetPixPat({}) current_ref={} -> handle=${:08X} home_ref={} path=\"{}\" size={}",
            pattern_id,
            current_resource_refnum,
            handle,
            record.ref_num,
            record.path,
            record.data.len()
        );
    }
    handle
}

#[allow(clippy::too_many_arguments)]
fn ppc_get_icon_suite(
    cpu: &PpcCpu,
    process_memory_manager: &mut ProcessNativeMemoryManager,
    memory: &mut PpcSectionMem,
    heap_cursor: &mut u32,
    heap_limit: u32,
    last_mem_error: &mut i16,
    handles: &mut Vec<PpcHandleRecord>,
    vfs_resources: &mut Vec<PpcVfsResourceRecord>,
    current_resource_refnum: i16,
    resource_load_enabled: bool,
    last_resource_error: &mut i16,
) -> i16 {
    let output = cpu.gpr[3];
    if memory.read_u32_be(output).is_none() {
        *last_mem_error = PPC_PARAM_ERR;
        return PPC_PARAM_ERR;
    }
    let resource_id = cpu.gpr[4] as u16 as i16;
    let selector = cpu.gpr[5];
    let resource_types = [
        (0x0000_0001, u32::from_be_bytes(*b"ICN#")),
        (0x0000_0002, u32::from_be_bytes(*b"icl4")),
        (0x0000_0004, u32::from_be_bytes(*b"icl8")),
        (0x0000_0100, u32::from_be_bytes(*b"ics#")),
        (0x0000_0200, u32::from_be_bytes(*b"ics4")),
        (0x0000_0400, u32::from_be_bytes(*b"ics8")),
        (0x0001_0000, u32::from_be_bytes(*b"icm#")),
        (0x0002_0000, u32::from_be_bytes(*b"icm4")),
        (0x0004_0000, u32::from_be_bytes(*b"icm8")),
    ];
    let mut entries = Vec::new();
    for (selector_bit, resource_type) in resource_types {
        if selector & selector_bit == 0 {
            continue;
        }
        let mut resource_cpu = PpcCpu::new();
        resource_cpu.gpr[3] = resource_type;
        resource_cpu.gpr[4] = resource_id as u16 as u32;
        let handle = ppc_get_resource(
            &mut resource_cpu,
            process_memory_manager,
            memory,
            heap_cursor,
            heap_limit,
            last_mem_error,
            handles,
            vfs_resources,
            current_resource_refnum,
            false,
            resource_load_enabled,
            last_resource_error,
        );
        if handle != 0 {
            entries.push((resource_type, handle));
        }
    }

    // Icon suites are opaque to applications. Keep a compact guest-memory
    // record so the other Icon Utilities imports can select family members
    // without application-specific state: magic, default label, count, then
    // resource type/Handle pairs.
    let mut bytes = Vec::with_capacity(10 + entries.len() * 8);
    bytes.extend_from_slice(&PPC_ICON_SUITE_MAGIC.to_be_bytes());
    bytes.extend_from_slice(&0u32.to_be_bytes());
    bytes.extend_from_slice(&(entries.len() as u16).to_be_bytes());
    for (resource_type, handle) in entries {
        bytes.extend_from_slice(&resource_type.to_be_bytes());
        bytes.extend_from_slice(&handle.to_be_bytes());
    }
    let suite = ppc_process_alloc_handle_with_bytes(
        process_memory_manager,
        memory,
        heap_cursor,
        last_mem_error,
        handles,
        &bytes,
    );
    if suite == 0 {
        let _ = memory.write_u32_be(output, 0);
        *last_mem_error = PPC_MEM_FULL_ERR;
        return PPC_MEM_FULL_ERR;
    }
    let _ = memory.write_u32_be(output, suite);
    *last_mem_error = PPC_NO_ERR;
    *last_resource_error = PPC_NO_ERR;
    PPC_NO_ERR
}
