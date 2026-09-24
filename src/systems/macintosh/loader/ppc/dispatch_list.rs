//! Typed List Manager dispatch for PowerPC imports.

use super::*;

pub(super) const PPC_LIST_VIEW_OFFSET: u32 = 0;
pub(super) const PPC_LIST_PORT_OFFSET: u32 = 8;
pub(super) const PPC_LIST_INDENT_OFFSET: u32 = 12;
pub(super) const PPC_LIST_CELL_SIZE_OFFSET: u32 = 16;
pub(super) const PPC_LIST_VISIBLE_OFFSET: u32 = 20;
pub(super) const PPC_LIST_VSCROLL_OFFSET: u32 = 28;
pub(super) const PPC_LIST_HSCROLL_OFFSET: u32 = 32;
pub(super) const PPC_LIST_SEL_FLAGS_OFFSET: u32 = 36;
pub(super) const PPC_LIST_ACTIVE_OFFSET: u32 = 37;
pub(super) const PPC_LIST_FLAGS_OFFSET: u32 = 39;
pub(super) const PPC_LIST_CLICK_TIME_OFFSET: u32 = 40;
pub(super) const PPC_LIST_CLICK_LOC_OFFSET: u32 = 44;
pub(super) const PPC_LIST_MOUSE_LOC_OFFSET: u32 = 48;
pub(super) const PPC_LIST_LAST_CLICK_OFFSET: u32 = 56;
pub(super) const PPC_LIST_DATA_BOUNDS_OFFSET: u32 = 72;
pub(super) const PPC_LIST_CELLS_OFFSET: u32 = 80;
pub(super) const PPC_LIST_MAX_INDEX_OFFSET: u32 = 84;
pub(super) const PPC_LIST_CELL_ARRAY_OFFSET: u32 = 86;
pub(super) const PPC_LIST_REC_MIN_SIZE: u32 = 88;

pub(super) struct PpcListDispatchContext<'a> {
    pub(super) binding: &'a PpcImportBinding,
    pub(super) cpu: &'a mut PpcCpu,
    pub(super) memory: &'a mut PpcSectionMem,
    pub(super) process_memory_manager: &'a mut ProcessNativeMemoryManager,
    pub(super) heap_cursor: &'a mut u32,
    pub(super) heap_limit: u32,
    pub(super) last_mem_error: &'a mut i16,
    pub(super) handles: &'a mut Vec<PpcHandleRecord>,
    pub(super) controls: &'a mut Vec<PpcControlRecord>,
    pub(super) list_manager: &'a mut ProcessListManagerState,
    pub(super) gworlds: &'a [PpcGWorldRecord],
    pub(super) vfs_resources: &'a [PpcVfsResourceRecord],
    pub(super) current_resource_refnum: i16,
    pub(super) tick_count: u32,
}

pub(super) fn dispatch_list_import(context: PpcListDispatchContext<'_>) -> Option<PpcImportAction> {
    let PpcListDispatchContext {
        binding,
        cpu,
        memory,
        process_memory_manager,
        heap_cursor,
        heap_limit,
        last_mem_error,
        handles,
        controls,
        list_manager,
        gworlds,
        vfs_resources,
        current_resource_refnum,
        tick_count,
    } = context;

    match binding.dispatcher_target {
        PpcImportDispatcherTarget::LNew => {
            // Inside Macintosh: More Macintosh Toolbox (1993), pp. 4-70--4-72:
            // construct the public ListRec, cell-data handle, bounds, default
            // cell dimensions, and variable cell-offset array.
            let mut allocator = PpcProcessAllocatorView {
                memory_manager: process_memory_manager,
            };
            let list = ppc_list_new(
                cpu,
                Some(&mut allocator),
                memory,
                heap_cursor,
                heap_limit,
                last_mem_error,
                handles,
                controls,
                list_manager,
            );
            *last_mem_error = if list == 0 {
                PPC_MEM_FULL_ERR
            } else {
                PPC_NO_ERR
            };
            // More Macintosh Toolbox (1993), p. 4-72: theProc names the
            // list's 'LDEF'. Cythera's is a shell over a procedure it keeps
            // in the refCon, which the emulator calls in its place.
            if list != 0 {
                let proc_id = cpu.gpr[6] as u16 as i16;
                let shell = vfs_resources
                    .iter()
                    .find(|record| {
                        record.res_type == u32::from_be_bytes(*b"LDEF") && record.res_id == proc_id
                    })
                    .is_some_and(|record| dispatch_defproc::ppc_ldef_is_refcon_shell(&record.data));
                dispatch_defproc::ppc_register_list_proc(list, shell);
            }
            list_manager.with_record_ref(list, |record| {
                if record.draw_enabled {
                    ppc_list_redraw(
                        memory,
                        handles,
                        controls,
                        gworlds,
                        vfs_resources,
                        current_resource_refnum,
                        record,
                    );
                }
            });
            Some(PpcImportAction::Return(list))
        }
        PpcImportDispatcherTarget::LDispose => {
            dispatch_defproc::ppc_forget_list(cpu.gpr[3]);
            if let Some(record) = list_manager.remove_record(cpu.gpr[3]) {
                let mut allocator = PpcProcessAllocatorView {
                    memory_manager: process_memory_manager,
                };
                let list_ptr = memory.read_u32_be(record.handle).unwrap_or(0);
                let control_handles = if list_ptr != 0 {
                    [
                        memory
                            .read_u32_be(list_ptr + PPC_LIST_VSCROLL_OFFSET)
                            .unwrap_or(0),
                        memory
                            .read_u32_be(list_ptr + PPC_LIST_HSCROLL_OFFSET)
                            .unwrap_or(0),
                    ]
                } else {
                    [0, 0]
                };
                for control_handle in control_handles {
                    if control_handle != 0 {
                        ppc_dispose_control(
                            Some(&mut allocator),
                            None,
                            memory,
                            heap_cursor,
                            heap_limit,
                            last_mem_error,
                            handles,
                            controls,
                            control_handle,
                        );
                    }
                }
                let _ = allocator.dispose_handle(
                    memory,
                    heap_cursor,
                    heap_limit,
                    last_mem_error,
                    handles,
                    record.cells_handle,
                );
                let _ = allocator.dispose_handle(
                    memory,
                    heap_cursor,
                    heap_limit,
                    last_mem_error,
                    handles,
                    record.handle,
                );
            }
            Some(PpcImportAction::ReturnPreserve)
        }
        PpcImportDispatcherTarget::LAddRow => {
            let count = cpu.gpr[3] as u16 as i16;
            let requested_row = cpu.gpr[4] as u16 as i16;
            let mut added_row = requested_row;
            list_manager.with_record_mut(cpu.gpr[5], |record| {
                let row = requested_row.clamp(record.data_bounds.0, record.data_bounds.2);
                added_row = row;
                if count > 0 {
                    record.cells = record
                        .cells
                        .drain()
                        .map(|((cell_row, cell_column), bytes)| {
                            let cell_row = if cell_row >= row {
                                cell_row.saturating_add(count)
                            } else {
                                cell_row
                            };
                            ((cell_row, cell_column), bytes)
                        })
                        .collect();
                    record.selected = record
                        .selected
                        .iter()
                        .map(|&(cell_row, cell_column)| {
                            let cell_row = if cell_row >= row {
                                cell_row.saturating_add(count)
                            } else {
                                cell_row
                            };
                            (cell_row, cell_column)
                        })
                        .collect();
                    record.data_bounds.2 = record.data_bounds.2.saturating_add(count);
                    ppc_list_recompute_visible(record);
                    let mut allocator = PpcProcessAllocatorView {
                        memory_manager: process_memory_manager,
                    };
                    let result = ppc_list_sync_guest_storage(
                        Some(&mut allocator),
                        memory,
                        heap_cursor,
                        heap_limit,
                        last_mem_error,
                        handles,
                        record,
                    );
                    *last_mem_error = result;
                    if record.draw_enabled {
                        ppc_list_redraw(
                            memory,
                            handles,
                            controls,
                            gworlds,
                            vfs_resources,
                            current_resource_refnum,
                            record,
                        );
                    }
                }
            });
            Some(PpcImportAction::Return(ppc_i16_result(added_row)))
        }
        PpcImportDispatcherTarget::LDelRow => {
            let count = cpu.gpr[3] as u16 as i16;
            let row = cpu.gpr[4] as u16 as i16;
            list_manager.with_record_mut(cpu.gpr[5], |record| {
                let (_, rows) = ppc_list_dimensions(record.data_bounds);
                if count == 0 || (row >= record.data_bounds.0 && row < record.data_bounds.2) {
                    // More Macintosh Toolbox (1993), p. 4-91: zero removes
                    // every row, independent of the supplied row number.
                    let (first_row, delete_rows) = if count == 0 {
                        (0, rows)
                    } else {
                        let first_row = usize::try_from(row - record.data_bounds.0).unwrap_or(0);
                        let delete_rows = usize::try_from(count.max(0))
                            .unwrap_or(0)
                            .min(rows - first_row);
                        (first_row, delete_rows)
                    };
                    let first_row = record.data_bounds.0.saturating_add(first_row as i16);
                    let after_rows = first_row.saturating_add(delete_rows as i16);
                    record.cells = record
                        .cells
                        .drain()
                        .filter_map(|((cell_row, cell_column), bytes)| {
                            if (first_row..after_rows).contains(&cell_row) {
                                None
                            } else {
                                let cell_row = if cell_row >= after_rows {
                                    cell_row.saturating_sub(delete_rows as i16)
                                } else {
                                    cell_row
                                };
                                Some(((cell_row, cell_column), bytes))
                            }
                        })
                        .collect();
                    record.selected = record
                        .selected
                        .iter()
                        .filter_map(|&(cell_row, cell_column)| {
                            if (first_row..after_rows).contains(&cell_row) {
                                None
                            } else {
                                let cell_row = if cell_row >= after_rows {
                                    cell_row.saturating_sub(delete_rows as i16)
                                } else {
                                    cell_row
                                };
                                Some((cell_row, cell_column))
                            }
                        })
                        .collect();
                    record.data_bounds.2 = record
                        .data_bounds
                        .2
                        .saturating_sub(delete_rows.min(i16::MAX as usize) as i16);
                    ppc_list_recompute_visible(record);
                    let mut allocator = PpcProcessAllocatorView {
                        memory_manager: process_memory_manager,
                    };
                    let result = ppc_list_sync_guest_storage(
                        Some(&mut allocator),
                        memory,
                        heap_cursor,
                        heap_limit,
                        last_mem_error,
                        handles,
                        record,
                    );
                    *last_mem_error = result;
                    if record.draw_enabled {
                        ppc_list_redraw(
                            memory,
                            handles,
                            controls,
                            gworlds,
                            vfs_resources,
                            current_resource_refnum,
                            record,
                        );
                    }
                }
            });
            Some(PpcImportAction::ReturnPreserve)
        }
        // More Macintosh Toolbox (1993), pp. 4-83 and 4-91: LAddColumn and
        // LDelColumn are LAddRow and LDelRow across the other axis.
        PpcImportDispatcherTarget::LAddColumn | PpcImportDispatcherTarget::LDelColumn => {
            let adding = binding.dispatcher_target == PpcImportDispatcherTarget::LAddColumn;
            let count = cpu.gpr[3] as u16 as i16;
            let column = cpu.gpr[4] as u16 as i16;
            let mut result_column = column;
            list_manager.with_record_mut(cpu.gpr[5], |record| {
                let changed = if adding {
                    let at = column.clamp(record.data_bounds.1, record.data_bounds.3);
                    result_column = at;
                    ppc_list_insert_columns(record, at, count)
                } else {
                    ppc_list_delete_columns(record, column, count)
                };
                if !changed {
                    return;
                }
                ppc_list_recompute_visible(record);
                let mut allocator = PpcProcessAllocatorView {
                    memory_manager: process_memory_manager,
                };
                *last_mem_error = ppc_list_sync_guest_storage(
                    Some(&mut allocator),
                    memory,
                    heap_cursor,
                    heap_limit,
                    last_mem_error,
                    handles,
                    record,
                );
                if record.draw_enabled {
                    ppc_list_redraw(
                        memory,
                        handles,
                        controls,
                        gworlds,
                        vfs_resources,
                        current_resource_refnum,
                        record,
                    );
                }
            });
            if adding {
                Some(PpcImportAction::Return(ppc_i16_result(result_column)))
            } else {
                Some(PpcImportAction::ReturnPreserve)
            }
        }
        // LRect(cellRect, theCell, lHandle): the cell's rectangle in the
        // rView's local coordinates, clipped to the view, and empty for a
        // cell outside the visible range (Inside Macintosh Volume IV, IV-273).
        PpcImportDispatcherTarget::LRect => {
            let rect_ptr = cpu.gpr[3];
            let v = (cpu.gpr[4] >> 16) as u16 as i16;
            let h = cpu.gpr[4] as u16 as i16;
            let rect = list_manager
                .with_record_mut(cpu.gpr[5], |record| ppc_list_cell_rect(record, v, h))
                .flatten()
                .unwrap_or((0, 0, 0, 0));
            if rect_ptr != 0 {
                let _ = ppc_write_rect(memory, rect_ptr, rect.0, rect.1, rect.2, rect.3);
            }
            Some(PpcImportAction::ReturnPreserve)
        }
        PpcImportDispatcherTarget::LGetSelect => {
            let next = cpu.gpr[3] != 0;
            let cell_ptr = cpu.gpr[4];
            let result = list_manager
                .with_record_ref(cpu.gpr[5], |record| {
                    let v = memory.read_u16_be(cell_ptr)? as i16;
                    let h = memory.read_u16_be(cell_ptr + 2)? as i16;
                    let found = if next {
                        ppc_list_cell_index(record, v, h)?;
                        record.selected.range((v, h)..).next().copied()
                    } else {
                        ppc_list_cell_index(record, v, h)?;
                        record.selected.contains(&(v, h)).then_some((v, h))
                    }?;
                    let (v, h) = found;
                    if next {
                        let _ = memory.write_u16_be(cell_ptr, v as u16);
                        let _ = memory.write_u16_be(cell_ptr + 2, h as u16);
                    }
                    Some(1)
                })
                .flatten()
                .unwrap_or(0);
            Some(PpcImportAction::Return(result))
        }
        PpcImportDispatcherTarget::LSetSelect => {
            let v = (cpu.gpr[4] >> 16) as u16 as i16;
            let h = cpu.gpr[4] as u16 as i16;
            list_manager.with_record_mut(cpu.gpr[5], |record| {
                if ppc_list_cell_index(record, v, h).is_some() {
                    if cpu.gpr[3] != 0 {
                        record.selected.insert((v, h));
                    } else {
                        record.selected.remove(&(v, h));
                    }
                    let mut allocator = PpcProcessAllocatorView {
                        memory_manager: process_memory_manager,
                    };
                    let result = ppc_list_sync_guest_storage(
                        Some(&mut allocator),
                        memory,
                        heap_cursor,
                        heap_limit,
                        last_mem_error,
                        handles,
                        record,
                    );
                    *last_mem_error = result;
                    if record.draw_enabled {
                        ppc_list_redraw(
                            memory,
                            handles,
                            controls,
                            gworlds,
                            vfs_resources,
                            current_resource_refnum,
                            record,
                        );
                    }
                }
            });
            Some(PpcImportAction::ReturnPreserve)
        }
        PpcImportDispatcherTarget::LSetCell => {
            let length = usize::from(cpu.gpr[4] as u16);
            let bytes = ppc_memory_read_bytes(memory, cpu.gpr[3], length as u32);
            let v = (cpu.gpr[5] >> 16) as u16 as i16;
            let h = cpu.gpr[5] as u16 as i16;
            if let Some(bytes) = bytes {
                list_manager.with_record_mut(cpu.gpr[6], |record| {
                    if ppc_list_cell_index(record, v, h).is_some() {
                        record.cells.insert((v, h), bytes);
                        let mut allocator = PpcProcessAllocatorView {
                            memory_manager: process_memory_manager,
                        };
                        let result = ppc_list_sync_guest_storage(
                            Some(&mut allocator),
                            memory,
                            heap_cursor,
                            heap_limit,
                            last_mem_error,
                            handles,
                            record,
                        );
                        *last_mem_error = result;
                        if record.draw_enabled {
                            ppc_list_redraw(
                                memory,
                                handles,
                                controls,
                                gworlds,
                                vfs_resources,
                                current_resource_refnum,
                                record,
                            );
                        }
                    }
                });
            }
            Some(PpcImportAction::ReturnPreserve)
        }
        // More Macintosh Toolbox (1993), 4-82: LAddToCell appends to the
        // cell's data, which LSetCell replaces.
        PpcImportDispatcherTarget::LAddToCell => {
            let length = usize::from(cpu.gpr[4] as u16);
            let bytes = ppc_memory_read_bytes(memory, cpu.gpr[3], length as u32);
            let v = (cpu.gpr[5] >> 16) as u16 as i16;
            let h = cpu.gpr[5] as u16 as i16;
            if let Some(bytes) = bytes {
                list_manager.with_record_mut(cpu.gpr[6], |record| {
                    if ppc_list_cell_index(record, v, h).is_some() {
                        record.cells.entry((v, h)).or_default().extend_from_slice(&bytes);
                        let mut allocator = PpcProcessAllocatorView {
                            memory_manager: process_memory_manager,
                        };
                        let result = ppc_list_sync_guest_storage(
                            Some(&mut allocator),
                            memory,
                            heap_cursor,
                            heap_limit,
                            last_mem_error,
                            handles,
                            record,
                        );
                        *last_mem_error = result;
                        if record.draw_enabled {
                            ppc_list_redraw(
                                memory,
                                handles,
                                controls,
                                gworlds,
                                vfs_resources,
                                current_resource_refnum,
                                record,
                            );
                        }
                    }
                });
            }
            Some(PpcImportAction::ReturnPreserve)
        }
        // More Macintosh Toolbox (1993), 4-84: LGetCellDataLocation reports
        // where a cell's data starts in the cells handle and its length,
        // laid out cell after cell in index order as the guest copy is.
        PpcImportDispatcherTarget::LGetCellDataLocation => {
            let v = (cpu.gpr[5] >> 16) as u16 as i16;
            let h = cpu.gpr[5] as u16 as i16;
            list_manager.with_record_ref(cpu.gpr[6], |record| {
                if let Some(index) = ppc_list_cell_index(record, v, h) {
                    let length_of = |cell_index: usize| {
                        ppc_list_cell_for_index(record, cell_index)
                            .and_then(|cell| record.cells.get(&cell))
                            .map_or(0, |bytes| bytes.len().min(0x7fff))
                    };
                    let offset: usize = (0..index).map(length_of).sum();
                    let _ = memory.write_u16_be(cpu.gpr[3], offset.min(0x7fff) as u16);
                    let _ = memory.write_u16_be(cpu.gpr[4], length_of(index) as u16);
                }
            });
            Some(PpcImportAction::ReturnPreserve)
        }
        PpcImportDispatcherTarget::LGetCell => {
            let length_ptr = cpu.gpr[4];
            let requested = usize::from(memory.read_u16_be(length_ptr).unwrap_or(0));
            let v = (cpu.gpr[5] >> 16) as u16 as i16;
            let h = cpu.gpr[5] as u16 as i16;
            list_manager.with_record_ref(cpu.gpr[6], |record| {
                if ppc_list_cell_index(record, v, h).is_some() {
                    let bytes = record.cells.get(&(v, h)).map(Vec::as_slice).unwrap_or(&[]);
                    // More Macintosh Toolbox (1993), pp. 4-82--4-83: dataLen is
                    // an in/out buffer capacity. A short buffer is left
                    // untouched, including its original capacity, rather than
                    // receiving a truncated cell.
                    if bytes.len() <= requested
                        && (bytes.is_empty() || memory.write_bytes(cpu.gpr[3], bytes).is_some())
                    {
                        let _ = memory.write_u16_be(length_ptr, bytes.len() as u16);
                    }
                }
            });
            Some(PpcImportAction::ReturnPreserve)
        }
        PpcImportDispatcherTarget::LClick => {
            let v = (cpu.gpr[3] >> 16) as u16 as i16;
            let h = cpu.gpr[3] as u16 as i16;
            let modifiers = cpu.gpr[4] as u16;
            let mut double_click = false;
            list_manager.with_record_mut(cpu.gpr[5], |record| {
                if let Some(list_ptr) = memory.read_u32_be(record.handle).filter(|ptr| *ptr != 0) {
                    let active = memory
                        .read_u8(list_ptr + PPC_LIST_ACTIVE_OFFSET)
                        .unwrap_or(1)
                        != 0;
                    // More Macintosh Toolbox (1993), p. 4-84: a click in one
                    // of the list's scroll bars scrolls the list. An arrow
                    // moves it a cell and the grey area a page, once per
                    // click here; the thumb is left alone (Cythera tracks its
                    // own thumb before calling LClick).
                    if let Some((d_rows, d_cols)) =
                        ppc_list_scroll_bar_click(memory, list_ptr, record, v, h)
                    {
                        if d_rows != 0 || d_cols != 0 {
                            ppc_list_set_visible_origin(
                                record,
                                record.visible.0.saturating_add(d_rows),
                                record.visible.1.saturating_add(d_cols),
                            );
                            *last_mem_error = ppc_list_sync_guest_visible(memory, record);
                            if record.draw_enabled {
                                ppc_list_redraw(
                                    memory,
                                    handles,
                                    controls,
                                    gworlds,
                                    vfs_resources,
                                    current_resource_refnum,
                                    record,
                                );
                            }
                        }
                        return;
                    }
                    if let Some(view) = ppc_read_rect(memory, list_ptr + PPC_LIST_VIEW_OFFSET)
                        .filter(|view| {
                            active && v >= view.0 && v < view.2 && h >= view.1 && h < view.3
                        })
                    {
                        let visible = ppc_read_rect(memory, list_ptr + PPC_LIST_VISIBLE_OFFSET)
                            .unwrap_or(record.data_bounds);
                        let cell_v = memory
                            .read_u16_be(list_ptr + PPC_LIST_CELL_SIZE_OFFSET)
                            .unwrap_or(1) as i16;
                        let cell_h = memory
                            .read_u16_be(list_ptr + PPC_LIST_CELL_SIZE_OFFSET + 2)
                            .unwrap_or(1) as i16;
                        let row = visible
                            .0
                            .saturating_add(v.saturating_sub(view.0) / cell_v.max(1));
                        let column = visible
                            .1
                            .saturating_add(h.saturating_sub(view.1) / cell_h.max(1));
                        if ppc_list_cell_index(record, row, column).is_some() {
                            let previous_time = memory
                                .read_u32_be(list_ptr + PPC_LIST_CLICK_TIME_OFFSET)
                                .unwrap_or(0);
                            let previous_v = memory
                                .read_u16_be(list_ptr + PPC_LIST_LAST_CLICK_OFFSET)
                                .unwrap_or(u16::MAX)
                                as i16;
                            let previous_h = memory
                                .read_u16_be(list_ptr + PPC_LIST_LAST_CLICK_OFFSET + 2)
                                .unwrap_or(u16::MAX)
                                as i16;
                            let double_time = memory
                                .read_u32_be(crate::memory::globals::addr::DOUBLE_TIME)
                                .unwrap_or(PPC_DEFAULT_DOUBLE_TIME_TICKS);
                            double_click = previous_time != 0
                                && previous_v == row
                                && previous_h == column
                                && tick_count.wrapping_sub(previous_time) <= double_time;
                            if modifiers & 0x0300 == 0 {
                                record.selected.clear();
                                record.selected.insert((row, column));
                            } else if modifiers & 0x0100 != 0 {
                                if !record.selected.remove(&(row, column)) {
                                    record.selected.insert((row, column));
                                }
                            } else {
                                record.selected.insert((row, column));
                            }
                            record.last_click = (row, column);
                            record.last_click_tick = tick_count;
                            let _ =
                                memory.write_u16_be(list_ptr + PPC_LIST_CLICK_LOC_OFFSET, v as u16);
                            let _ = memory
                                .write_u16_be(list_ptr + PPC_LIST_CLICK_LOC_OFFSET + 2, h as u16);
                            let _ =
                                memory.write_u16_be(list_ptr + PPC_LIST_MOUSE_LOC_OFFSET, v as u16);
                            let _ = memory
                                .write_u16_be(list_ptr + PPC_LIST_MOUSE_LOC_OFFSET + 2, h as u16);
                            let _ = memory
                                .write_u16_be(list_ptr + PPC_LIST_LAST_CLICK_OFFSET, row as u16);
                            let _ = memory.write_u16_be(
                                list_ptr + PPC_LIST_LAST_CLICK_OFFSET + 2,
                                column as u16,
                            );
                            let _ = memory
                                .write_u32_be(list_ptr + PPC_LIST_CLICK_TIME_OFFSET, tick_count);
                            let mut allocator = PpcProcessAllocatorView {
                                memory_manager: process_memory_manager,
                            };
                            let result = ppc_list_sync_guest_storage(
                                Some(&mut allocator),
                                memory,
                                heap_cursor,
                                heap_limit,
                                last_mem_error,
                                handles,
                                record,
                            );
                            *last_mem_error = result;
                            if record.draw_enabled {
                                ppc_list_redraw(
                                    memory,
                                    handles,
                                    controls,
                                    gworlds,
                                    vfs_resources,
                                    current_resource_refnum,
                                    record,
                                );
                            }
                        }
                    }
                }
            });
            Some(PpcImportAction::Return(u32::from(double_click)))
        }
        PpcImportDispatcherTarget::LActivate => {
            let active = cpu.gpr[3] != 0;
            list_manager.with_record_mut(cpu.gpr[4], |record| {
                record.active = active;
                if let Some(list_ptr) = memory.read_u32_be(record.handle).filter(|ptr| *ptr != 0) {
                    let _ = memory.write_u8(list_ptr + PPC_LIST_ACTIVE_OFFSET, u8::from(active));
                    for offset in [PPC_LIST_VSCROLL_OFFSET, PPC_LIST_HSCROLL_OFFSET] {
                        let control_handle = memory.read_u32_be(list_ptr + offset).unwrap_or(0);
                        if let Some(control) = ppc_control_ptr(memory, control_handle) {
                            // LActivate: IM IV-276 describes hiding the bars.
                            // Mac OS 8.1 keeps contrlVis and sets contrlHilite=255
                            // (BasiliskII/SheepShaver probe).
                            let _ = memory.write_u8(
                                control + PPC_CONTROL_HILITE_OFFSET,
                                if active { 0 } else { 0xff },
                            );
                        }
                    }
                    if record.draw_enabled {
                        ppc_list_redraw(
                            memory,
                            handles,
                            controls,
                            gworlds,
                            vfs_resources,
                            current_resource_refnum,
                            record,
                        );
                    }
                }
            });
            Some(PpcImportAction::ReturnPreserve)
        }
        PpcImportDispatcherTarget::LSetDrawingMode => {
            let draw_enabled = cpu.gpr[3] != 0;
            list_manager.with_record_mut(cpu.gpr[4], |record| {
                record.draw_enabled = draw_enabled;
                if let Some(list_ptr) = memory.read_u32_be(record.handle).filter(|ptr| *ptr != 0) {
                    for offset in [PPC_LIST_VSCROLL_OFFSET, PPC_LIST_HSCROLL_OFFSET] {
                        let control_handle = memory.read_u32_be(list_ptr + offset).unwrap_or(0);
                        if let Some(control) = ppc_control_ptr(memory, control_handle) {
                            // LDoDraw(FALSE) disables cell drawing, not control
                            // visibility (IM IV-275; Mac OS 8.1 oracle probe).
                            if draw_enabled {
                                let _ = memory.write_u8(control + PPC_CONTROL_VISIBLE_OFFSET, 0xff);
                            }
                        }
                    }
                }
                // More Macintosh Toolbox (1993), p. 4-84: after enabling the
                // drawing mode, the application redraws the list itself. An
                // application LDEF is not called here: Cythera turns drawing
                // on while its list object is still being built.
                if draw_enabled && !dispatch_defproc::ppc_list_has_app_ldef(record.handle) {
                    ppc_list_redraw(
                        memory,
                        handles,
                        controls,
                        gworlds,
                        vfs_resources,
                        current_resource_refnum,
                        record,
                    );
                }
            });
            Some(PpcImportAction::ReturnPreserve)
        }
        PpcImportDispatcherTarget::LScroll => {
            // More Macintosh Toolbox (1993), pp. 4-89--4-90: scrolling is
            // pinned to dataBounds and redraws when automatic drawing is on.
            let d_cols = cpu.gpr[3] as u16 as i16;
            let d_rows = cpu.gpr[4] as u16 as i16;
            list_manager.with_record_mut(cpu.gpr[5], |record| {
                ppc_list_set_visible_origin(
                    record,
                    record.visible.0.saturating_add(d_rows),
                    record.visible.1.saturating_add(d_cols),
                );
                let result = ppc_list_sync_guest_visible(memory, record);
                *last_mem_error = result;
                if record.draw_enabled {
                    ppc_list_redraw(
                        memory,
                        handles,
                        controls,
                        gworlds,
                        vfs_resources,
                        current_resource_refnum,
                        record,
                    );
                }
            });
            Some(PpcImportAction::ReturnPreserve)
        }
        PpcImportDispatcherTarget::LSize => {
            // More Macintosh Toolbox (1993), pp. 4-91--4-92: resize the
            // visible rectangle and redraw its contents as necessary.
            let width = cpu.gpr[3] as u16 as i16;
            let height = cpu.gpr[4] as u16 as i16;
            list_manager.with_record_mut(cpu.gpr[5], |record| {
                record.view_rect.2 = record.view_rect.0.saturating_add(height.max(0));
                record.view_rect.3 = record.view_rect.1.saturating_add(width.max(0));
                ppc_list_recompute_visible(record);
                let result = ppc_list_sync_guest_size(memory, record);
                *last_mem_error = result;
                if record.draw_enabled {
                    ppc_list_redraw(
                        memory,
                        handles,
                        controls,
                        gworlds,
                        vfs_resources,
                        current_resource_refnum,
                        record,
                    );
                }
            });
            Some(PpcImportAction::ReturnPreserve)
        }
        PpcImportDispatcherTarget::LUpdate => {
            // More Macintosh Toolbox (1993), p. 4-86: LUpdate redraws the
            // cells, and the scroll bars, that intersect the update region.
            // For a list drawn by its own LDEF that decides which cells the
            // application is asked to draw; an empty region, as in a hidden
            // window, asks for none.
            let update_bounds = memory
                .read_u32_be(cpu.gpr[3])
                .filter(|_| cpu.gpr[3] != 0)
                .and_then(|_| ppc_region_storage(memory, cpu.gpr[3]))
                .and_then(|storage| ppc_region_storage_bbox(&storage))
                .unwrap_or((0, 0, 0, 0));
            let app_ldef = dispatch_defproc::ppc_list_has_app_ldef(cpu.gpr[4]);
            list_manager.with_record_ref(cpu.gpr[4], |record| {
                if record.draw_enabled && app_ldef {
                    ppc_list_draw_within(memory, gworlds, record, Some(update_bounds));
                    // Cythera's To Do and Journal drawers are updated with
                    // LUpdate alone, and showed no scroll bar.
                    ppc_list_draw_scroll_bars_within(
                        memory,
                        handles,
                        controls,
                        gworlds,
                        vfs_resources,
                        current_resource_refnum,
                        record,
                        update_bounds,
                    );
                } else if record.draw_enabled {
                    ppc_list_redraw(
                        memory,
                        handles,
                        controls,
                        gworlds,
                        vfs_resources,
                        current_resource_refnum,
                        record,
                    );
                }
            });
            Some(PpcImportAction::ReturnPreserve)
        }
        PpcImportDispatcherTarget::LAutoScroll => {
            list_manager.with_record_mut(cpu.gpr[3], |record| {
                if let (Some(&(row, column)), Some(list_ptr)) = (
                    record.selected.iter().next(),
                    memory.read_u32_be(record.handle).filter(|ptr| *ptr != 0),
                ) {
                    let visible = ppc_read_rect(memory, list_ptr + PPC_LIST_VISIBLE_OFFSET)
                        .unwrap_or(record.data_bounds);
                    let height = visible.2.saturating_sub(visible.0);
                    let width = visible.3.saturating_sub(visible.1);
                    record.visible = (
                        row,
                        column,
                        row.saturating_add(height).min(record.data_bounds.2),
                        column.saturating_add(width).min(record.data_bounds.3),
                    );
                    let _ = ppc_write_rect(
                        memory,
                        list_ptr + PPC_LIST_VISIBLE_OFFSET,
                        record.visible.0,
                        record.visible.1,
                        record.visible.2,
                        record.visible.3,
                    );
                    if record.draw_enabled {
                        ppc_list_redraw(
                            memory,
                            handles,
                            controls,
                            gworlds,
                            vfs_resources,
                            current_resource_refnum,
                            record,
                        );
                    }
                }
            });
            Some(PpcImportAction::ReturnPreserve)
        }
        PpcImportDispatcherTarget::LSearch => {
            let requested = ppc_memory_read_bytes(memory, cpu.gpr[3], u32::from(cpu.gpr[4] as u16))
                .unwrap_or_default();
            let cell_ptr = cpu.gpr[6];
            let result = list_manager
                .with_record_ref(cpu.gpr[7], |record| {
                    let start_v = memory.read_u16_be(cell_ptr)? as i16;
                    let start_h = memory.read_u16_be(cell_ptr + 2)? as i16;
                    let start = ppc_list_cell_index(record, start_v, start_h)?;
                    let (columns, rows) = ppc_list_dimensions(record.data_bounds);
                    (start..columns.saturating_mul(rows)).find_map(|index| {
                        let cell = ppc_list_cell_for_index(record, index)?;
                        let bytes = record.cells.get(&cell).map(Vec::as_slice).unwrap_or(&[]);
                        (bytes.len() == requested.len()
                            && bytes
                                .iter()
                                .zip(&requested)
                                .all(|(left, right)| left.eq_ignore_ascii_case(right)))
                        .then_some(cell)
                    })
                })
                .flatten();
            if let Some((v, h)) = result {
                let _ = memory.write_u16_be(cell_ptr, v as u16);
                let _ = memory.write_u16_be(cell_ptr + 2, h as u16);
                Some(PpcImportAction::Return(1))
            } else {
                Some(PpcImportAction::Return(0))
            }
        }
        _ => None,
    }
}

fn ppc_list_dimensions(bounds: (i16, i16, i16, i16)) -> (usize, usize) {
    (
        usize::try_from(i32::from(bounds.3) - i32::from(bounds.1)).unwrap_or(0),
        usize::try_from(i32::from(bounds.2) - i32::from(bounds.0)).unwrap_or(0),
    )
}

fn ppc_list_visible_rect(
    view_rect: (i16, i16, i16, i16),
    data_bounds: (i16, i16, i16, i16),
    cell_size: (i16, i16),
) -> (i16, i16, i16, i16) {
    // More Macintosh Toolbox (1993), pp. 4-70--4-72 and 4-91--4-92:
    // visible includes any cell that intersects the view, so partial cells
    // round up to the next row or column.
    let (columns, rows) = ppc_list_dimensions(data_bounds);
    let cell_v = cell_size.0.max(1);
    let cell_h = cell_size.1.max(1);
    let view_height = (i32::from(view_rect.2) - i32::from(view_rect.0)).max(0);
    let view_width = (i32::from(view_rect.3) - i32::from(view_rect.1)).max(0);
    let visible_rows = usize::try_from((view_height + i32::from(cell_v) - 1) / i32::from(cell_v))
        .unwrap_or(0)
        .max(1)
        .min(rows);
    let visible_columns = usize::try_from((view_width + i32::from(cell_h) - 1) / i32::from(cell_h))
        .unwrap_or(0)
        .max(1)
        .min(columns);
    (
        data_bounds.0,
        data_bounds.1,
        data_bounds.0.saturating_add(visible_rows as i16),
        data_bounds.1.saturating_add(visible_columns as i16),
    )
}

fn ppc_list_set_visible_origin(record: &mut PpcListRecord, row: i16, column: i16) {
    record.set_visible_origin(row, column);
}

fn ppc_list_recompute_visible(record: &mut PpcListRecord) {
    let old_origin = (record.visible.0, record.visible.1);
    record.visible = ppc_list_visible_rect(record.view_rect, record.data_bounds, record.cell_size);
    ppc_list_set_visible_origin(record, old_origin.0, old_origin.1);
}

fn ppc_list_cell_index(record: &PpcListRecord, v: i16, h: i16) -> Option<usize> {
    let (columns, rows) = ppc_list_dimensions(record.data_bounds);
    let column = usize::try_from(i32::from(h) - i32::from(record.data_bounds.1)).ok()?;
    let row = usize::try_from(i32::from(v) - i32::from(record.data_bounds.0)).ok()?;
    (column < columns && row < rows).then(|| row * columns + column)
}

fn ppc_list_cell_for_index(record: &PpcListRecord, index: usize) -> Option<(i16, i16)> {
    let (columns, rows) = ppc_list_dimensions(record.data_bounds);
    if columns == 0 || index >= columns.saturating_mul(rows) {
        return None;
    }
    Some((
        record
            .data_bounds
            .0
            .saturating_add((index / columns) as i16),
        record
            .data_bounds
            .1
            .saturating_add((index % columns) as i16),
    ))
}

/// Where a click lands in one of a list's scroll bars, as the (rows,
/// columns) to scroll by; None when the point is in neither bar. The parts
/// are the standard bar's: 16-pixel arrows at the ends and a 16-pixel thumb
/// placed by the control's value, the grey area either side of it.
fn ppc_list_scroll_bar_click(
    memory: &mut PpcSectionMem,
    list_ptr: u32,
    record: &PpcListRecord,
    v: i16,
    h: i16,
) -> Option<(i16, i16)> {
    for (offset, vertical) in [
        (PPC_LIST_VSCROLL_OFFSET, true),
        (PPC_LIST_HSCROLL_OFFSET, false),
    ] {
        let handle = memory.read_u32_be(list_ptr + offset).unwrap_or(0);
        let Some(control) = ppc_control_ptr(memory, handle) else {
            continue;
        };
        if memory.read_u8(control + PPC_CONTROL_VISIBLE_OFFSET).unwrap_or(0) == 0
            || memory.read_u8(control + PPC_CONTROL_HILITE_OFFSET).unwrap_or(0) >= 254
        {
            continue;
        }
        let Some((top, left, bottom, right)) =
            ppc_read_rect(memory, control + PPC_CONTROL_RECT_OFFSET)
        else {
            continue;
        };
        if v < top || v >= bottom || h < left || h >= right {
            continue;
        }
        let (start, end, at) = if vertical { (top, bottom, v) } else { (left, right, h) };
        let arrow = ((end - start) / 2).clamp(0, 16);
        let page = if vertical {
            record.visible.2 - record.visible.0 - 1
        } else {
            record.visible.3 - record.visible.1 - 1
        }
        .max(1);
        let step = if at < start + arrow {
            -1
        } else if at >= end - arrow {
            1
        } else {
            let value = memory.read_u16_be(control + PPC_CONTROL_VALUE_OFFSET).unwrap_or(0) as i16;
            let min = memory.read_u16_be(control + PPC_CONTROL_MIN_OFFSET).unwrap_or(0) as i16;
            let max = memory.read_u16_be(control + PPC_CONTROL_MAX_OFFSET).unwrap_or(0) as i16;
            let track = i32::from(end - start - 2 * arrow - 16).max(0);
            let span = (i32::from(max) - i32::from(min)).max(1);
            let thumb = i32::from(start + arrow)
                + (i32::from(value) - i32::from(min)).clamp(0, span) * track / span;
            if i32::from(at) < thumb {
                -page
            } else if i32::from(at) >= thumb + 16 {
                page
            } else {
                0
            }
        };
        return Some(if vertical { (step, 0) } else { (0, step) });
    }
    None
}

fn ppc_list_scrollbar_bounds(record: &PpcListRecord, vertical: bool) -> (i16, i16, i16, i16) {
    // More Macintosh Toolbox (1993), pp. 4-75--4-76: standard list scroll
    // bars occupy the one-pixel border outside rView and a 16-pixel strip.
    if vertical {
        (
            record.view_rect.0.saturating_sub(1),
            record.view_rect.3,
            record.view_rect.2.saturating_add(1),
            record.view_rect.3.saturating_add(16),
        )
    } else {
        (
            record.view_rect.2,
            record.view_rect.1.saturating_sub(1),
            record.view_rect.2.saturating_add(16),
            record.view_rect.3.saturating_add(1),
        )
    }
}

fn ppc_list_scrollbar_limits(record: &PpcListRecord, vertical: bool) -> (i16, i16, i16) {
    record.scrollbar_limits(vertical)
}

fn ppc_list_sync_guest_scrollbars(memory: &mut PpcSectionMem, record: &PpcListRecord) -> i16 {
    let Some(list_ptr) = memory.read_u32_be(record.handle).filter(|ptr| *ptr != 0) else {
        return PPC_PARAM_ERR;
    };
    for (offset, vertical) in [
        (PPC_LIST_VSCROLL_OFFSET, true),
        (PPC_LIST_HSCROLL_OFFSET, false),
    ] {
        let control_handle = memory.read_u32_be(list_ptr + offset).unwrap_or(0);
        let Some(control) = ppc_control_ptr(memory, control_handle) else {
            continue;
        };
        let (top, left, bottom, right) = ppc_list_scrollbar_bounds(record, vertical);
        let (value, min, max) = ppc_list_scrollbar_limits(record, vertical);
        if ppc_write_rect(
            memory,
            control + PPC_CONTROL_RECT_OFFSET,
            top,
            left,
            bottom,
            right,
        )
        .is_none()
            || memory
                .write_u16_be(control + PPC_CONTROL_VALUE_OFFSET, value as u16)
                .is_none()
            || memory
                .write_u16_be(control + PPC_CONTROL_MIN_OFFSET, min as u16)
                .is_none()
            || memory
                .write_u16_be(control + PPC_CONTROL_MAX_OFFSET, max as u16)
                .is_none()
        {
            return PPC_PARAM_ERR;
        }
    }
    PPC_NO_ERR
}

fn ppc_list_sync_guest_geometry(memory: &mut PpcSectionMem, record: &PpcListRecord) -> i16 {
    let Some(list_ptr) = memory.read_u32_be(record.handle).filter(|ptr| *ptr != 0) else {
        return PPC_PARAM_ERR;
    };
    if ppc_write_rect(
        memory,
        list_ptr + PPC_LIST_VIEW_OFFSET,
        record.view_rect.0,
        record.view_rect.1,
        record.view_rect.2,
        record.view_rect.3,
    )
    .is_none()
        || ppc_write_rect(
            memory,
            list_ptr + PPC_LIST_VISIBLE_OFFSET,
            record.visible.0,
            record.visible.1,
            record.visible.2,
            record.visible.3,
        )
        .is_none()
    {
        return PPC_PARAM_ERR;
    }
    ppc_list_sync_guest_scrollbars(memory, record)
}

fn ppc_list_sync_guest_visible(memory: &mut PpcSectionMem, record: &PpcListRecord) -> i16 {
    let Some(list_ptr) = memory.read_u32_be(record.handle).filter(|ptr| *ptr != 0) else {
        return PPC_PARAM_ERR;
    };
    if ppc_write_rect(
        memory,
        list_ptr + PPC_LIST_VISIBLE_OFFSET,
        record.visible.0,
        record.visible.1,
        record.visible.2,
        record.visible.3,
    )
    .is_none()
    {
        return PPC_PARAM_ERR;
    }
    ppc_list_sync_guest_scrollbars(memory, record)
}

fn ppc_list_sync_guest_size(memory: &mut PpcSectionMem, record: &PpcListRecord) -> i16 {
    let Some(list_ptr) = memory.read_u32_be(record.handle).filter(|ptr| *ptr != 0) else {
        return PPC_PARAM_ERR;
    };
    if memory
        .write_u16_be(
            list_ptr + PPC_LIST_VIEW_OFFSET + 4,
            record.view_rect.2 as u16,
        )
        .is_none()
        || memory
            .write_u16_be(
                list_ptr + PPC_LIST_VIEW_OFFSET + 6,
                record.view_rect.3 as u16,
            )
            .is_none()
    {
        return PPC_PARAM_ERR;
    }
    ppc_list_sync_guest_visible(memory, record)
}

fn ppc_list_sync_guest_storage(
    mut allocator: Option<&mut PpcProcessAllocatorView<'_>>,
    memory: &mut PpcSectionMem,
    heap_cursor: &mut u32,
    heap_limit: u32,
    last_mem_error: &mut i16,
    handles: &mut Vec<PpcHandleRecord>,
    record: &PpcListRecord,
) -> i16 {
    let (columns, rows) = ppc_list_dimensions(record.data_bounds);
    let cell_count = columns.saturating_mul(rows);
    let offsets_size = (cell_count as u32).saturating_add(1).saturating_mul(2);
    let list_size = PPC_LIST_REC_MIN_SIZE.max(PPC_LIST_CELL_ARRAY_OFFSET + offsets_size);
    let result = ppc_allocator_view_resize_handle(
        allocator.as_deref_mut(),
        memory,
        heap_cursor,
        heap_limit,
        last_mem_error,
        handles,
        record.handle,
        list_size,
    );
    if result != PPC_NO_ERR {
        return result;
    }
    let data_size = (0..cell_count).fold(0u32, |size, index| {
        let bytes = ppc_list_cell_for_index(record, index)
            .and_then(|cell| record.cells.get(&cell))
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        size.saturating_add(bytes.len().min(0x7fff) as u32)
    });
    if data_size > 32_000 {
        return PPC_MEM_FULL_ERR;
    }
    let result = ppc_allocator_view_resize_handle(
        allocator.as_deref_mut(),
        memory,
        heap_cursor,
        heap_limit,
        last_mem_error,
        handles,
        record.cells_handle,
        data_size,
    );
    if result != PPC_NO_ERR {
        return result;
    }
    let result = ppc_list_sync_guest_geometry(memory, record);
    if result != PPC_NO_ERR {
        return result;
    }
    let Some(list_ptr) = memory.read_u32_be(record.handle).filter(|ptr| *ptr != 0) else {
        return PPC_PARAM_ERR;
    };
    if ppc_write_rect(
        memory,
        list_ptr + PPC_LIST_DATA_BOUNDS_OFFSET,
        record.data_bounds.0,
        record.data_bounds.1,
        record.data_bounds.2,
        record.data_bounds.3,
    )
    .is_none()
        || memory
            .write_u32_be(list_ptr + PPC_LIST_CELLS_OFFSET, record.cells_handle)
            .is_none()
        || memory
            .write_u16_be(
                list_ptr + PPC_LIST_MAX_INDEX_OFFSET,
                cell_count.saturating_mul(2).min(u16::MAX as usize) as u16,
            )
            .is_none()
    {
        return PPC_PARAM_ERR;
    }
    let data_ptr = memory.read_u32_be(record.cells_handle).unwrap_or(0);
    let mut offset = 0u16;
    for index in 0..cell_count {
        let cell = ppc_list_cell_for_index(record, index)
            .expect("List Manager index is inside dataBounds");
        let bytes = record.cells.get(&cell).map(Vec::as_slice).unwrap_or(&[]);
        let selection = if record.selected.contains(&cell) {
            0x8000
        } else {
            0
        };
        let _ = memory.write_u16_be(
            list_ptr + PPC_LIST_CELL_ARRAY_OFFSET + index as u32 * 2,
            selection | offset,
        );
        if !bytes.is_empty()
            && memory
                .write_bytes(data_ptr + u32::from(offset), bytes)
                .is_none()
        {
            return PPC_PARAM_ERR;
        }
        offset = offset.saturating_add(bytes.len().min(0x7fff) as u16);
    }
    let _ = memory.write_u16_be(
        list_ptr + PPC_LIST_CELL_ARRAY_OFFSET + cell_count as u32 * 2,
        offset,
    );
    PPC_NO_ERR
}

#[allow(clippy::too_many_arguments)]
fn ppc_list_new(
    cpu: &PpcCpu,
    mut allocator: Option<&mut PpcProcessAllocatorView<'_>>,
    memory: &mut PpcSectionMem,
    heap_cursor: &mut u32,
    heap_limit: u32,
    last_mem_error: &mut i16,
    handles: &mut Vec<PpcHandleRecord>,
    controls: &mut Vec<PpcControlRecord>,
    list_manager: &mut ProcessListManagerState,
) -> u32 {
    let (Some(view), Some(data_bounds)) = (
        ppc_read_rect(memory, cpu.gpr[3]),
        ppc_read_rect(memory, cpu.gpr[4]),
    ) else {
        return 0;
    };
    let (columns, rows) = ppc_list_dimensions(data_bounds);
    let Some(cell_count) = columns
        .checked_mul(rows)
        .filter(|count| *count <= i16::MAX as usize)
    else {
        return 0;
    };
    let list_handle = ppc_allocator_view_allocate_handle(
        allocator.as_deref_mut(),
        memory,
        heap_cursor,
        heap_limit,
        last_mem_error,
        handles,
        PPC_LIST_REC_MIN_SIZE + (cell_count as u32 + 1) * 2,
        true,
    );
    let cells_handle = ppc_allocator_view_allocate_handle(
        allocator.as_deref_mut(),
        memory,
        heap_cursor,
        heap_limit,
        last_mem_error,
        handles,
        0,
        true,
    );
    let Some(list_ptr) = memory.read_u32_be(list_handle).filter(|ptr| *ptr != 0) else {
        return 0;
    };
    let mut cell_v = (cpu.gpr[5] >> 16) as u16 as i16;
    let mut cell_h = cpu.gpr[5] as u16 as i16;
    if cell_v <= 0 {
        let font = ppc_current_text_font(memory, cpu.gpr[7]);
        let size = memory
            .read_u16_be(cpu.gpr[7].wrapping_add(PPC_CGRAF_PORT_TX_SIZE_OFFSET))
            .unwrap_or(PPC_QD_TEXT_SIZE_SYSTEM as u16) as i16;
        let (face, scale) = get_font_face_scaled(font, size);
        cell_v = face
            .metrics
            .ascent
            .saturating_add(face.metrics.descent)
            .saturating_add(face.metrics.leading)
            .saturating_mul(scale)
            .max(1);
    }
    if cell_h <= 0 {
        cell_h = if columns == 0 {
            1
        } else {
            (view.3.saturating_sub(view.1) / columns as i16).max(1)
        };
    }
    let visible = ppc_list_visible_rect(view, data_bounds, (cell_v, cell_h));
    let scroll_horiz = cpu.gpr[10] != 0;
    let scroll_vert = ppc_parameter_area_slot_addr(cpu.gpr[1], PPC_NATIVE_PARAMETER_GPR_COUNT)
        .and_then(|slot| memory.read_u32_be(slot))
        .unwrap_or(0)
        != 0;
    let draw_enabled = cpu.gpr[8] != 0;
    let record = PpcListRecord {
        handle: list_handle,
        cells_handle,
        view_rect: view,
        data_bounds,
        cell_size: (cell_v, cell_h),
        visible,
        port: cpu.gpr[7],
        draw_enabled,
        active: true,
        cells: std::collections::HashMap::new(),
        selected: std::collections::BTreeSet::new(),
        last_click: (-1, -1),
        last_click_tick: 0,
    };
    let v_scroll = if scroll_vert {
        ppc_new_control_record_values(
            allocator.as_deref_mut(),
            memory,
            heap_cursor,
            heap_limit,
            last_mem_error,
            handles,
            controls,
            record.port,
            ppc_list_scrollbar_bounds(&record, true),
            &[],
            draw_enabled,
            ppc_list_scrollbar_limits(&record, true).0,
            ppc_list_scrollbar_limits(&record, true).1,
            ppc_list_scrollbar_limits(&record, true).2,
            16,
            0,
        )
    } else {
        0
    };
    let h_scroll = if scroll_horiz {
        ppc_new_control_record_values(
            allocator.as_deref_mut(),
            memory,
            heap_cursor,
            heap_limit,
            last_mem_error,
            handles,
            controls,
            record.port,
            ppc_list_scrollbar_bounds(&record, false),
            &[],
            draw_enabled,
            ppc_list_scrollbar_limits(&record, false).0,
            ppc_list_scrollbar_limits(&record, false).1,
            ppc_list_scrollbar_limits(&record, false).2,
            16,
            0,
        )
    } else {
        0
    };
    if ppc_write_rect(
        memory,
        list_ptr + PPC_LIST_VIEW_OFFSET,
        view.0,
        view.1,
        view.2,
        view.3,
    )
    .is_none()
        || memory
            .write_u32_be(list_ptr + PPC_LIST_PORT_OFFSET, cpu.gpr[7])
            .is_none()
        || memory
            .write_u32_be(list_ptr + PPC_LIST_VSCROLL_OFFSET, v_scroll)
            .is_none()
        || memory
            .write_u32_be(list_ptr + PPC_LIST_HSCROLL_OFFSET, h_scroll)
            .is_none()
        || memory
            .write_u8(list_ptr + PPC_LIST_SEL_FLAGS_OFFSET, 0)
            .is_none()
        || memory
            .write_u8(list_ptr + PPC_LIST_ACTIVE_OFFSET, 1)
            .is_none()
        || memory
            .write_u16_be(
                list_ptr + PPC_LIST_INDENT_OFFSET,
                cell_v.saturating_sub(3) as u16,
            )
            .is_none()
        || memory
            .write_u16_be(list_ptr + PPC_LIST_INDENT_OFFSET + 2, 1)
            .is_none()
        || memory
            .write_u16_be(list_ptr + PPC_LIST_CELL_SIZE_OFFSET, cell_v as u16)
            .is_none()
        || memory
            .write_u16_be(list_ptr + PPC_LIST_CELL_SIZE_OFFSET + 2, cell_h as u16)
            .is_none()
        || ppc_write_rect(
            memory,
            list_ptr + PPC_LIST_VISIBLE_OFFSET,
            visible.0,
            visible.1,
            visible.2,
            visible.3,
        )
        .is_none()
        || memory
            .write_u8(list_ptr + PPC_LIST_FLAGS_OFFSET, 0)
            .is_none()
    {
        return 0;
    }
    if ppc_list_sync_guest_storage(
        allocator.as_deref_mut(),
        memory,
        heap_cursor,
        heap_limit,
        last_mem_error,
        handles,
        &record,
    ) != PPC_NO_ERR
    {
        return 0;
    }
    list_manager.insert_record(list_handle, record);
    list_handle
}

pub(super) fn ppc_list_draw(
    memory: &mut PpcSectionMem,
    gworlds: &[PpcGWorldRecord],
    record: &PpcListRecord,
) {
    ppc_list_draw_within(memory, gworlds, record, None);
}

/// Draw the list's visible cells; with `limit`, only those whose rectangle
/// meets it (port-local).
fn ppc_list_draw_within(
    memory: &mut PpcSectionMem,
    gworlds: &[PpcGWorldRecord],
    record: &PpcListRecord,
    limit: Option<(i16, i16, i16, i16)>,
) {
    let Some(list_ptr) = memory.read_u32_be(record.handle).filter(|ptr| *ptr != 0) else {
        return;
    };
    let Some((view_top, view_left, view_bottom, view_right)) =
        ppc_read_rect(memory, list_ptr + PPC_LIST_VIEW_OFFSET)
    else {
        return;
    };
    let Some(visible) = ppc_read_rect(memory, list_ptr + PPC_LIST_VISIBLE_OFFSET) else {
        return;
    };
    let cell_v = memory
        .read_u16_be(list_ptr + PPC_LIST_CELL_SIZE_OFFSET)
        .unwrap_or(1) as i16;
    let cell_h = memory
        .read_u16_be(list_ptr + PPC_LIST_CELL_SIZE_OFFSET + 2)
        .unwrap_or(1) as i16;
    let active = memory
        .read_u8(list_ptr + PPC_LIST_ACTIVE_OFFSET)
        .unwrap_or(1)
        != 0;
    let port = memory
        .read_u32_be(list_ptr + PPC_LIST_PORT_OFFSET)
        .unwrap_or(PPC_MAIN_GWORLD);
    if ppc_live_quickdraw_surface(memory, gworlds, port).is_none() {
        return;
    }
    if dispatch_defproc::ppc_list_has_app_ldef(record.handle) {
        ppc_list_queue_app_ldef_draws(
            record,
            visible,
            (view_top, view_left, view_bottom, view_right),
            (cell_v, cell_h),
            active,
            port,
            limit,
        );
        return;
    }
    let font = ppc_current_text_font(memory, port);
    let size = memory
        .read_u16_be(port.wrapping_add(PPC_CGRAF_PORT_TX_SIZE_OFFSET))
        .unwrap_or(PPC_QD_TEXT_SIZE_SYSTEM as u16) as i16;
    let (face, scale) = get_font_face_scaled(font, size);
    let ascent = face.metrics.ascent.saturating_mul(scale);
    for row in visible.0..visible.2 {
        for column in visible.1..visible.3 {
            if ppc_list_cell_index(record, row, column).is_none() {
                continue;
            }
            let top = view_top.saturating_add(row.saturating_sub(visible.0).saturating_mul(cell_v));
            let left =
                view_left.saturating_add(column.saturating_sub(visible.1).saturating_mul(cell_h));
            // List view coordinates are local to the list's port. Imaging
            // With QuickDraw (1994), pp. 2-9--2-10: map them through the
            // port's PixMap boundary before writing the backing pixels.
            let port_rect = (
                top,
                left,
                top.saturating_add(cell_v).min(view_bottom),
                left.saturating_add(cell_h).min(view_right),
            );
            let selected = active && record.selected.contains(&(row, column));
            let background = if selected {
                PPC_RGB_BLACK
            } else {
                PPC_RGB_WHITE
            };
            let foreground = if selected {
                PPC_RGB_WHITE
            } else {
                PPC_RGB_BLACK
            };
            // Imaging With QuickDraw (1994), pp. 2-20--2-21: the cell is
            // painted within the port's visRgn and clipRgn, so a cell wider
            // than its window stops at the window's edge.
            let _ = ppc_paint_rect_bounds(memory, gworlds, port, port_rect, background, None);
            let _ = ppc_draw_text_bytes(
                memory,
                gworlds,
                port,
                (left.saturating_add(1), top.saturating_add(ascent)),
                font,
                size,
                PPC_QD_TEXT_MODE_SRC_OR,
                foreground,
                None,
                record
                    .cells
                    .get(&(row, column))
                    .map(Vec::as_slice)
                    .unwrap_or(&[]),
            );
        }
    }
}

fn ppc_list_redraw(
    memory: &mut PpcSectionMem,
    handles: &[PpcHandleRecord],
    controls: &[PpcControlRecord],
    gworlds: &[PpcGWorldRecord],
    vfs_resources: &[PpcVfsResourceRecord],
    current_resource_refnum: i16,
    record: &PpcListRecord,
) {
    ppc_list_draw(memory, gworlds, record);
    let Some(list_ptr) = memory.read_u32_be(record.handle).filter(|ptr| *ptr != 0) else {
        return;
    };
    for offset in [PPC_LIST_VSCROLL_OFFSET, PPC_LIST_HSCROLL_OFFSET] {
        let Some(control_handle) = memory
            .read_u32_be(list_ptr + offset)
            .filter(|handle| *handle != 0)
        else {
            continue;
        };
        if !record.active {
            if let Some(control) = ppc_control_ptr(memory, control_handle) {
                // LUpdate's inactive-list scrollbar state on Mac OS 8.1.
                let _ = memory.write_u8(control + PPC_CONTROL_HILITE_OFFSET, 254);
            }
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
}

/// Draw the list's scroll bars that meet `bounds`, in the list port's
/// local coordinates.
#[allow(clippy::too_many_arguments)]
fn ppc_list_draw_scroll_bars_within(
    memory: &mut PpcSectionMem,
    handles: &[PpcHandleRecord],
    controls: &[PpcControlRecord],
    gworlds: &[PpcGWorldRecord],
    vfs_resources: &[PpcVfsResourceRecord],
    current_resource_refnum: i16,
    record: &PpcListRecord,
    bounds: (i16, i16, i16, i16),
) {
    let Some(list_ptr) = memory.read_u32_be(record.handle).filter(|ptr| *ptr != 0) else {
        return;
    };
    for offset in [PPC_LIST_VSCROLL_OFFSET, PPC_LIST_HSCROLL_OFFSET] {
        let control_handle = memory.read_u32_be(list_ptr + offset).unwrap_or(0);
        let Some(control) = ppc_control_ptr(memory, control_handle) else {
            continue;
        };
        let Some((top, left, bottom, right)) =
            ppc_read_rect(memory, control + PPC_CONTROL_RECT_OFFSET)
        else {
            continue;
        };
        if top < bounds.2 && bottom > bounds.0 && left < bounds.3 && right > bounds.1 {
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
    }
}

fn ppc_list_cell_rect(record: &PpcListRecord, v: i16, h: i16) -> Option<(i16, i16, i16, i16)> {
    ppc_list_cell_index(record, v, h)?;
    let visible = record.visible;
    if v < visible.0 || v >= visible.2 || h < visible.1 || h >= visible.3 {
        return None;
    }
    let (cell_v, cell_h) = (record.cell_size.0.max(1), record.cell_size.1.max(1));
    let top = record.view_rect.0 + (v - visible.0) * cell_v;
    let left = record.view_rect.1 + (h - visible.1) * cell_h;
    let bottom = (top + cell_v).min(record.view_rect.2);
    let right = (left + cell_h).min(record.view_rect.3);
    (bottom > top && right > left).then_some((top, left, bottom, right))
}

/// Insert `count` empty columns before `at`, moving cells and selection.
fn ppc_list_insert_columns(record: &mut PpcListRecord, at: i16, count: i16) -> bool {
    if count <= 0 {
        return false;
    }
    let shift = |column: i16| if column >= at { column.saturating_add(count) } else { column };
    record.cells = record
        .cells
        .drain()
        .map(|((row, column), bytes)| ((row, shift(column)), bytes))
        .collect();
    record.selected = record.selected.iter().map(|&(row, column)| (row, shift(column))).collect();
    record.data_bounds.3 = record.data_bounds.3.saturating_add(count);
    true
}

/// Delete `count` columns from `column`; zero deletes them all.
fn ppc_list_delete_columns(record: &mut PpcListRecord, column: i16, count: i16) -> bool {
    let (columns, _) = ppc_list_dimensions(record.data_bounds);
    if count != 0 && !(column >= record.data_bounds.1 && column < record.data_bounds.3) {
        return false;
    }
    let (first, delete) = if count == 0 {
        (record.data_bounds.1, columns)
    } else {
        let offset = usize::try_from(column - record.data_bounds.1).unwrap_or(0);
        (column, usize::try_from(count.max(0)).unwrap_or(0).min(columns - offset))
    };
    let after = first.saturating_add(delete as i16);
    let keep = |column: i16| !(first..after).contains(&column);
    let shift = |column: i16| {
        if column >= after {
            column.saturating_sub(delete as i16)
        } else {
            column
        }
    };
    record.cells = record
        .cells
        .drain()
        .filter(|((_, column), _)| keep(*column))
        .map(|((row, column), bytes)| ((row, shift(column)), bytes))
        .collect();
    record.selected = record
        .selected
        .iter()
        .filter(|(_, column)| keep(*column))
        .map(|&(row, column)| (row, shift(column)))
        .collect();
    record.data_bounds.3 = record.data_bounds.3.saturating_sub(delete.min(i16::MAX as usize) as i16);
    true
}

/// Ask the list's own LDEF to draw each visible cell (lDrawMsg), with the
/// cell's data located as LGetCellDataLocation reports it. The rectangle is
/// clipped to the view, as LRect reports it.
fn ppc_list_queue_app_ldef_draws(
    record: &PpcListRecord,
    visible: (i16, i16, i16, i16),
    view: (i16, i16, i16, i16),
    (cell_v, cell_h): (i16, i16),
    active: bool,
    port: u32,
    limit: Option<(i16, i16, i16, i16)>,
) {
    let (columns, rows) = ppc_list_dimensions(record.data_bounds);
    let mut offsets = vec![0u16; columns * rows + 1];
    for index in 0..columns * rows {
        let length = ppc_list_cell_for_index(record, index)
            .and_then(|cell| record.cells.get(&cell))
            .map_or(0, |bytes| bytes.len() as u16);
        offsets[index + 1] = offsets[index].saturating_add(length);
    }
    for row in visible.0..visible.2 {
        for column in visible.1..visible.3 {
            let Some(index) = ppc_list_cell_index(record, row, column) else {
                continue;
            };
            let top = view.0.saturating_add(row.saturating_sub(visible.0).saturating_mul(cell_v.max(1)));
            let left = view.1.saturating_add(column.saturating_sub(visible.1).saturating_mul(cell_h.max(1)));
            let rect = (
                top,
                left,
                top.saturating_add(cell_v.max(1)).min(view.2),
                left.saturating_add(cell_h.max(1)).min(view.3),
            );
            if rect.0 >= rect.2 || rect.1 >= rect.3 {
                continue;
            }
            if let Some((top, left, bottom, right)) = limit {
                if rect.0 >= bottom || rect.2 <= top || rect.1 >= right || rect.3 <= left {
                    continue;
                }
            }
            dispatch_defproc::ppc_note_app_ldef_call(dispatch_defproc::PpcLdefCall {
                list: record.handle,
                message: dispatch_defproc::LDEF_DRAW,
                select: active && record.selected.contains(&(row, column)),
                rect,
                cell: (row, column),
                data_offset: offsets[index],
                data_len: offsets[index + 1] - offsets[index],
                port,
            });
        }
    }
}
