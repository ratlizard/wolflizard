//! Calls into an application's own window definition function.
//!
//! The native Window Manager here draws window frames itself. An application
//! can instead supply its own WDEF: a 'WDEF' resource whose code the Window
//! Manager calls with a message (Macintosh Toolbox Essentials (1992),
//! pp. 4-121 to 4-130). A PowerPC application commonly ships a six-byte stub
//! resource and patches it at run time into a 68K `JMP` (`$4EF9`) to a
//! routine descriptor for its native definition function; Cythera does this
//! for 'WDEF' 1000 to 1003 in `TWDEFRegister`.
//!
//! Frame drawing happens deep inside synchronous helpers, which cannot call
//! guest code. So a window whose procID names an application WDEF is only
//! noted when its frame would be drawn; once the import that drew it has
//! finished, the noted windows' definition functions are called with
//! `wDraw` as nested guest calls, and the import's own result is returned
//! after the last one.
use super::*;
use std::cell::RefCell;

/// WDEF messages (Macintosh Toolbox Essentials (1992), pp. 4-122 to 4-130).
pub(super) const WDEF_DRAW: u32 = 0;
pub(super) const WDEF_CALC_REGIONS: u32 = 2;
pub(super) const WDEF_NEW: u32 = 3;

#[derive(Clone, Copy)]
struct DefProcCall {
    target: PpcCallbackTarget,
    args: [u32; 4],
    /// The port to make current for the call (a CDEF draws in its window).
    port: Option<u32>,
}

/// CDEF messages (Macintosh Toolbox Essentials (1992), pp. 5-104 to 5-116).
pub(super) const CDEF_DRAW: u32 = 0;
pub(super) const CDEF_INIT: u32 = 3;

/// Record a new control; one naming an application CDEF is told of it.
pub(super) fn ppc_register_control_proc(handle: u32, proc_id: i16) {
    let app = (proc_id as u16 >> 4) >= 128;
    APP_CDEF_CONTROLS.with(|controls| {
        let mut controls = controls.borrow_mut();
        if app {
            controls.insert(handle, proc_id);
        } else {
            controls.remove(&handle);
        }
    });
    if app {
        ppc_note_app_cdef_message(handle, CDEF_INIT, 0);
    }
}

pub(super) fn ppc_control_has_app_cdef(handle: u32) -> bool {
    APP_CDEF_CONTROLS.with(|controls| controls.borrow().contains_key(&handle))
}

pub(super) fn ppc_note_app_cdef_message(handle: u32, message: u32, param: u32) {
    PENDING_CDEF_CALLS.with(|pending| {
        let mut pending = pending.borrow_mut();
        if !pending.contains(&(handle, message, param)) {
            pending.push((handle, message, param));
        }
    });
}

struct DefProcCallState {
    /// The current port when the calls began; the Window Manager restores it
    /// after calling a definition function, which may leave its own port set.
    saved_port: u32,
    /// A word pair to put back once the call in flight returns.
    restore: Option<(u32, u32)>,
    import_pc: u32,
    final_lr: u32,
    restore_rtoc: u32,
    /// The import's argument registers, which the calls clobber; a yielding
    /// import is dispatched again with them.
    saved_args: [u32; 8],
    calls: Vec<DefProcCall>,
    next: usize,
    completion: PpcImportAction,
}

thread_local! {
    /// Windows whose windowDefProc now holds their application WDEF's
    /// resource handle, with the procID the native side kept there before.
    static APP_WDEF_WINDOWS: RefCell<std::collections::HashMap<u32, i16>> =
        RefCell::new(std::collections::HashMap::new());
    static PENDING_WDEF_DRAWS: RefCell<Vec<(u32, u32)>> = const { RefCell::new(Vec::new()) };
    /// Controls with an application CDEF: handle -> procID.
    static APP_CDEF_CONTROLS: RefCell<std::collections::HashMap<u32, i16>> =
        RefCell::new(std::collections::HashMap::new());
    static PENDING_CDEF_CALLS: RefCell<Vec<(u32, u32, u32)>> = const { RefCell::new(Vec::new()) };
    static DEF_PROC_STACK: RefCell<Vec<DefProcCallState>> = const { RefCell::new(Vec::new()) };
    static PORT_TO_RESTORE: RefCell<Option<u32>> = const { RefCell::new(None) };
    /// ClipAbove for the next WDEF draw (Some(window)), or Some(0) to open
    /// the Window Manager port's clip again.
    static CLIP_ABOVE: RefCell<Option<u32>> = const { RefCell::new(None) };
}

/// Resource IDs from 128 up are the application's; the system's WDEFs are
/// 0 and 1 (and the Appearance ones below 128).
pub(super) fn ppc_proc_id_names_app_wdef(proc_id: i16) -> bool {
    (proc_id as u16 >> 4) >= 128
}

pub(super) fn ppc_app_wdef_window_proc_id(window: u32) -> Option<i16> {
    APP_WDEF_WINDOWS.with(|windows| windows.borrow().get(&window).copied())
}

/// Forget a disposed window; true when it had an application WDEF.
pub(super) fn ppc_forget_app_wdef_window(window: u32) -> bool {
    PENDING_WDEF_DRAWS.with(|pending| pending.borrow_mut().retain(|(w, _)| *w != window));
    APP_WDEF_WINDOWS.with(|windows| windows.borrow_mut().remove(&window).is_some())
}

pub(super) fn ppc_note_app_wdef_draw(window: u32) {
    ppc_note_app_wdef_message(window, WDEF_DRAW);
}

/// Queue a message for a window's application WDEF, once per import.
pub(super) fn ppc_note_app_wdef_message(window: u32, message: u32) {
    PENDING_WDEF_DRAWS.with(|pending| {
        let mut pending = pending.borrow_mut();
        if pending.contains(&(window, message)) {
            return;
        }
        // Regions are computed before the window is drawn with them.
        let before_draw = (message == WDEF_CALC_REGIONS)
            .then(|| pending.iter().position(|&entry| entry == (window, WDEF_DRAW)))
            .flatten();
        match before_draw {
            Some(index) => pending.insert(index, (window, message)),
            None => pending.push((window, message)),
        }
    });
}

/// The native definition function a patched 'WDEF' stub jumps to.
fn ppc_app_def_proc_handle(
    vfs_resources: &[PpcVfsResourceRecord],
    res_type: u32,
    res_id: i16,
) -> Option<u32> {
    vfs_resources
        .iter()
        .find(|record| record.res_type == res_type && record.res_id == res_id && record.handle != 0)
        .map(|record| record.handle)
}

fn ppc_app_def_proc_target(
    memory: &mut PpcSectionMem,
    vfs_resources: &[PpcVfsResourceRecord],
    res_type: u32,
    res_id: i16,
    default_rtoc: u32,
) -> Option<PpcCallbackTarget> {
    let handle = ppc_app_def_proc_handle(vfs_resources, res_type, res_id)?;
    if ppc_trace_defproc_enabled() {
        let records: Vec<String> = vfs_resources
            .iter()
            .filter(|record| record.res_type == res_type && record.res_id == res_id)
            .map(|record| format!("ref={} handle=${:08X}", record.ref_num, record.handle))
            .collect();
        let code = memory.read_u32_be(handle).unwrap_or(0);
        eprintln!(
            "[PPC-DEFPROC] resolve {res_id}: {records:?} code=${code:08X} word=${:04X} rd=${:08X}",
            memory.read_u16_be(code).unwrap_or(0),
            memory.read_u32_be(code.wrapping_add(2)).unwrap_or(0)
        );
    }
    let code = memory.read_u32_be(handle).filter(|ptr| *ptr != 0)?;
    if memory.read_u16_be(code)? != 0x4EF9 {
        return None;
    }
    let descriptor = memory.read_u32_be(code.wrapping_add(2)).filter(|ptr| *ptr != 0)?;
    let target = ppc_resolve_callback_target(memory, descriptor, default_rtoc, None)?;
    memory.read_u32_be(target.entry)?;
    Some(target)
}

fn ppc_next_def_proc_call(cpu: &mut PpcCpu, memory: &mut PpcSectionMem) -> Option<PpcImportAction> {
    DEF_PROC_STACK.with(|stack| {
        let mut stack = stack.borrow_mut();
        loop {
            let state = stack.last_mut()?;
            if let Some((address, value)) = state.restore.take() {
                let _ = memory.write_u32_be(address, value);
            }
            if state.next >= state.calls.len() {
                let state = stack.pop().expect("state present");
                PORT_TO_RESTORE.with(|slot| *slot.borrow_mut() = Some(state.saved_port));
                CLIP_ABOVE.with(|slot| *slot.borrow_mut() = Some(0));
                cpu.lr = state.final_lr;
                cpu.gpr[2] = state.restore_rtoc;
                cpu.gpr[3..11].copy_from_slice(&state.saved_args);
                return Some(state.completion);
            }
            let call = state.calls[state.next];
            state.next += 1;
            if let Some(port) = call.port {
                PORT_TO_RESTORE.with(|slot| *slot.borrow_mut() = Some(port));
            }
            // Macintosh Toolbox Essentials (1992), 4-118: before a WDEF
            // draws, the Window Manager port is clipped to the window's
            // structure less the windows in front (ClipAbove).
            if call.port.is_none() && call.args[2] == WDEF_DRAW {
                CLIP_ABOVE.with(|slot| *slot.borrow_mut() = Some(call.args[1]));
            }
            // A WDEF computing regions takes the window's global origin from
            // portBits.bounds (GrafPort offsets 8 and 10), which in a
            // CGrafPort is where grafVars lives. For the length of the call
            // put the colour port's PixMap bounds there, as a basic port
            // would hold them, and restore the handle afterwards.
            if call.args[2] == WDEF_CALC_REGIONS {
                let window = call.args[1];
                let bounds = memory
                    .read_u32_be(window.wrapping_add(2))
                    .and_then(|pix_map| memory.read_u32_be(pix_map))
                    .and_then(|pix_map| memory.read_u32_be(pix_map.wrapping_add(6)));
                if let (Some(bounds), Some(saved)) =
                    (bounds, memory.read_u32_be(window.wrapping_add(8)))
                {
                    state.restore = Some((window.wrapping_add(8), saved));
                    let _ = memory.write_u32_be(window.wrapping_add(8), bounds);
                }
            }
            if ppc_trace_defproc_enabled() {
                let window = call.args[1];
                let bbox = |memory: &mut PpcSectionMem, rgn_handle: u32| -> String {
                    memory
                        .read_u32_be(rgn_handle)
                        .and_then(|ptr| ppc_read_rect(memory, ptr.wrapping_add(2)))
                        .map_or("-".to_string(), |r| format!("{:?}", r))
                };
                let port = PPC_MAIN_GWORLD;
                let clip = memory.read_u32_be(port.wrapping_add(PPC_CGRAF_PORT_CLIP_RGN_OFFSET)).unwrap_or(0);
                let vis = memory.read_u32_be(port.wrapping_add(PPC_CGRAF_PORT_VIS_RGN_OFFSET)).unwrap_or(0);
                let pix_bounds = memory
                    .read_u32_be(window.wrapping_add(2))
                    .and_then(|pix_map| memory.read_u32_be(pix_map))
                    .and_then(|pix_map| ppc_read_rect(memory, pix_map.wrapping_add(6)));
                let port_rect = ppc_read_rect(memory, window.wrapping_add(16));
                eprintln!("[PPC-DEFPROC]   pixmap bounds={pix_bounds:?} portRect={port_rect:?}");
                let struc = memory.read_u32_be(window.wrapping_add(114)).unwrap_or(0);
                let cont = memory.read_u32_be(window.wrapping_add(118)).unwrap_or(0);
                eprintln!(
                    "[PPC-DEFPROC] call message={} window=${window:08X} dataHandle=${:08X} visible={} wmgr clip={} vis={} struc={} cont={}",
                    call.args[2],
                    memory.read_u32_be(window.wrapping_add(130)).unwrap_or(0),
                    memory.read_u8(window.wrapping_add(110)).unwrap_or(0),
                    bbox(memory, clip),
                    bbox(memory, vis),
                    bbox(memory, struc),
                    bbox(memory, cont),
                );
            }
            let restore_rtoc = state.restore_rtoc;
            if install_powerpc_call_arguments(cpu, memory, &call.args).is_none() {
                continue;
            }
            return GuestCallEffect::call_guest(
                GuestCallRequest::new(GuestCallTarget {
                    isa: GuestIsa::PowerPc,
                    entry: call.target.entry,
                    rtoc: call.target.rtoc,
                }),
                GuestCallContinuation::to_powerpc(
                    PPC_GUEST_CALL_RETURN_PC,
                    cpu.pc,
                    restore_rtoc,
                    PpcNativeReturnGpr3::Preserve,
                ),
            )
            .into_ppc_import_action();
        }
    })
}

/// Called before an import is dispatched: when a definition call begun by
/// this same import has returned, run the next one or finish the import.
pub(super) fn ppc_resume_def_proc_calls(
    cpu: &mut PpcCpu,
    memory: &mut PpcSectionMem,
) -> Option<PpcImportAction> {
    let resuming = cpu.lr == cpu.pc
        && DEF_PROC_STACK.with(|stack| {
            stack
                .borrow()
                .last()
                .is_some_and(|state| state.import_pc == cpu.pc)
        });
    if !resuming {
        return None;
    }
    ppc_next_def_proc_call(cpu, memory)
}

/// Called after an import is dispatched: when it would have drawn the frame
/// of a window with an application WDEF, call that WDEF now, then return
/// the import's own result.
/// The port to make current again once a batch of definition calls is done.
pub(super) fn ppc_take_clip_above() -> Option<u32> {
    CLIP_ABOVE.with(|slot| slot.borrow_mut().take())
}

pub(super) fn ppc_take_port_to_restore() -> Option<u32> {
    PORT_TO_RESTORE.with(|slot| slot.borrow_mut().take())
}

pub(super) fn ppc_begin_pending_def_procs(
    cpu: &mut PpcCpu,
    memory: &mut PpcSectionMem,
    vfs_resources: &[PpcVfsResourceRecord],
    action: Option<PpcImportAction>,
    current_port: u32,
) -> Option<PpcImportAction> {
    let pending = PENDING_WDEF_DRAWS.with(|pending| std::mem::take(&mut *pending.borrow_mut()));
    let pending_controls =
        PENDING_CDEF_CALLS.with(|pending| std::mem::take(&mut *pending.borrow_mut()));
    if pending.is_empty() && pending_controls.is_empty() {
        return action;
    }
    let completion = match action {
        // A yield leaves the CPU at the import slot, so the calls can run
        // first: ModalDialog draws its controls on an update event and then
        // waits, and would otherwise never return to let them be drawn.
        Some(
            action @ (PpcImportAction::Return(_)
            | PpcImportAction::ReturnPreserve
            | PpcImportAction::Yield(_)),
        ) => action,
        // Anything else (a guest call of the import's own, a halt) keeps its
        // meaning; the frames are drawn at a later import.
        other => {
            PENDING_WDEF_DRAWS.with(|slot| slot.borrow_mut().extend(pending));
            PENDING_CDEF_CALLS.with(|slot| slot.borrow_mut().extend(pending_controls));
            return other;
        }
    };
    let wdef = u32::from_be_bytes(*b"WDEF");
    let calls: Vec<DefProcCall> = pending
        .into_iter()
        .filter_map(|(window, message)| {
            let proc_id = ppc_window_proc_id(memory, window);
            let res_id = (proc_id as u16 >> 4) as i16;
            let var_code = u32::from(proc_id as u16 & 0x000F);
            let target = ppc_app_def_proc_target(memory, vfs_resources, wdef, res_id, cpu.gpr[2]);
            // Macintosh Toolbox Essentials (1992), p. 4-70: windowDefProc
            // is the handle to the WDEF. An application WDEF may identify
            // itself by that handle (Cythera's TWDEFRegister does), so it
            // must be the resource's own handle before wNew; the procID the
            // native side stored there is kept aside.
            if message == WDEF_NEW && target.is_some() {
                if let Some(handle) = ppc_app_def_proc_handle(vfs_resources, wdef, res_id) {
                    APP_WDEF_WINDOWS.with(|windows| windows.borrow_mut().insert(window, proc_id));
                    let _ = memory.write_u32_be(window.wrapping_add(126), handle);
                }
            }
            if ppc_trace_defproc_enabled() {
                eprintln!(
                    "[PPC-DEFPROC] WDEF {res_id} var={var_code} window=${window:08X} message={message} target={}",
                    target.map_or("none".to_string(), |t| format!("${:08X}", t.entry))
                );
            }
            let target = target?;
            Some(DefProcCall {
                target,
                args: [var_code, window, message, 0],
                port: None,
            })
        })
        .collect();
    let cdef = u32::from_be_bytes(*b"CDEF");
    let mut calls = calls;
    calls.extend(pending_controls.into_iter().filter_map(|(handle, message, param)| {
        let proc_id = APP_CDEF_CONTROLS.with(|controls| controls.borrow().get(&handle).copied())?;
        let res_id = (proc_id as u16 >> 4) as i16;
        let var_code = u32::from(proc_id as u16 & 0x000F);
        let target = ppc_app_def_proc_target(memory, vfs_resources, cdef, res_id, cpu.gpr[2])?;
        let control = memory.read_u32_be(handle).filter(|ptr| *ptr != 0)?;
        // Macintosh Toolbox Essentials (1992), p. 5-114: contrlDefProc is
        // the handle to the CDEF, which an application CDEF may identify
        // itself by (Cythera's TCDEFRegister, like its WDEF).
        if message == CDEF_INIT {
            if let Some(defproc) = ppc_app_def_proc_handle(vfs_resources, cdef, res_id) {
                let _ = memory.write_u32_be(control.wrapping_add(24), defproc);
            }
        }
        let owner = memory.read_u32_be(control.wrapping_add(4)).filter(|ptr| *ptr != 0);
        Some(DefProcCall {
            target,
            args: [var_code, handle, message, param],
            port: owner,
        })
    }));
    if calls.is_empty() {
        return Some(completion);
    }
    DEF_PROC_STACK.with(|stack| {
        stack.borrow_mut().push(DefProcCallState {
            import_pc: cpu.pc,
            final_lr: cpu.lr,
            restore_rtoc: cpu.gpr[2],
            saved_args: cpu.gpr[3..11].try_into().expect("eight argument registers"),
            calls,
            next: 0,
            completion,
            restore: None,
            saved_port: current_port,
        })
    });
    ppc_next_def_proc_call(cpu, memory).or(Some(completion))
}
