//! QuickTime music on the PowerPC path: `'tune'` components.
//!
//! An application opens a tune player with `OpenDefaultComponent('tune', 0)`,
//! gives it a time scale, a header and a volume, and keeps its queue topped up
//! by reading `TuneGetStatus` (`QuickTimeMusic.h`, Universal Interfaces 3.3.1).
//! Cythera's `GMSInit` and `GMSTune` do exactly this. The player is the 68K
//! Component Manager path's: the same decoding and synthesis, and the same
//! substitute recordings or MIDI files from `SYSTEMLESS_TUNE_LIBRARY`.
//!
//! The Note Allocator (`NANewNoteChannel` and the rest) is bound so that its
//! weak imports resolve, but `OpenDefaultComponent('nota')` still finds no
//! component, as on the 68K path.
use super::*;

/// The QuickTimeLib music routines bound here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PpcTuneOp {
    SetHeader,
    SetTimeScale,
    SetVolume,
    Stop,
    Queue,
    GetStatus,
    Preroll,
    Unroll,
    NoteAllocator,
}

const TUNE_COMPONENT_TYPE: u32 = u32::from_be_bytes(*b"tune");
/// Component instances handed out for tune players. Distinct from any guest
/// address the application could hold as a `SndChannel`, since the sound
/// manager keys the player's channel by it.
const FIRST_TUNE_INSTANCE: u32 = 0x00C2_0001;
/// `badComponentInstance` from Components.h.
const BAD_COMPONENT_INSTANCE: i32 = 0x8000_8001_u32 as i32;

pub(super) fn ppc_tune_symbol_op(symbol: &str) -> Option<PpcTuneOp> {
    Some(match symbol {
        "TuneSetHeader" => PpcTuneOp::SetHeader,
        "TuneSetTimeScale" => PpcTuneOp::SetTimeScale,
        "TuneSetVolume" => PpcTuneOp::SetVolume,
        "TuneStop" => PpcTuneOp::Stop,
        "TuneQueue" => PpcTuneOp::Queue,
        "TuneGetStatus" => PpcTuneOp::GetStatus,
        "TunePreroll" => PpcTuneOp::Preroll,
        "TuneUnroll" => PpcTuneOp::Unroll,
        "NANewNoteChannel" | "NAPlayNote" | "NADisposeNoteChannel" | "NAStuffToneDescription" => {
            PpcTuneOp::NoteAllocator
        }
        _ => return None,
    })
}

/// Serve a tune call, or an `OpenDefaultComponent`/`CloseComponent` that
/// concerns a tune player. Anything else is left to the other dispatchers.
pub(super) fn dispatch_tune_import(
    binding: &PpcImportBinding,
    cpu: &mut PpcCpu,
    memory: &mut PpcSectionMem,
    sound: &mut PpcSoundState,
    tick: u32,
) -> Option<PpcImportAction> {
    match binding.dispatcher_target {
        PpcImportDispatcherTarget::SystemCompatibility(
            PpcSystemCompatibilityOperation::OpenDefaultComponent,
        ) if cpu.gpr[3] == TUNE_COMPONENT_TYPE => {
            let tunes = &mut sound.tunes;
            let instance = FIRST_TUNE_INSTANCE.wrapping_add(tunes.next_instance);
            tunes.next_instance = tunes.next_instance.wrapping_add(1);
            tunes.players.insert(instance, Default::default());
            Some(PpcImportAction::Return(instance))
        }
        PpcImportDispatcherTarget::CloseComponent
            if sound.tunes.players.contains_key(&cpu.gpr[3]) =>
        {
            let instance = cpu.gpr[3];
            sound.tunes.players.remove(&instance);
            sound.manager.with_mut(|manager| manager.stop_tune_channel(instance));
            Some(PpcImportAction::Return(0))
        }
        PpcImportDispatcherTarget::QuickTimeMusic(op) => {
            Some(PpcImportAction::Return(ppc_tune_call(op, cpu, memory, sound, tick) as u32))
        }
        _ => None,
    }
}

fn ppc_tune_call(
    op: PpcTuneOp,
    cpu: &mut PpcCpu,
    memory: &mut PpcSectionMem,
    sound: &mut PpcSoundState,
    tick: u32,
) -> i32 {
    match op {
        // No note allocator component is ever opened, so there is no
        // channel for these to act on.
        PpcTuneOp::NoteAllocator => return BAD_COMPONENT_INSTANCE,
        _ => {}
    }
    let instance = cpu.gpr[3];
    let trace = std::env::var_os("SYSTEMLESS_TRACE_TUNE").is_some();
    let read_long = |memory: &mut PpcSectionMem, address: u32| memory.read_u32_be(address).unwrap_or(0);
    let Some(player) = sound.tunes.players.get_mut(&instance) else {
        return BAD_COMPONENT_INSTANCE;
    };
    match op {
        // TuneSetHeader(tp, unsigned long *header): a tune stream of note
        // requests naming each part's General MIDI instrument.
        PpcTuneOp::SetHeader => {
            let header = cpu.gpr[4];
            let words =
                crate::trap::read_tune_stream_with(header, |address| read_long(memory, address));
            player.header = header;
            player.header_programs = crate::tune_player::decode_tune(&words).programs;
            player.rendered = None;
            if trace {
                eprintln!(
                    "[TUNE] PPC header=${header:08X} longs={} programs={:?}",
                    words.len(),
                    player.header_programs
                );
            }
        }
        // TuneSetTimeScale(tp, TimeScale scale)
        PpcTuneOp::SetTimeScale => player.time_scale = cpu.gpr[4].max(1),
        // TuneSetVolume(tp, Fixed volume)
        PpcTuneOp::SetVolume => player.volume_fixed = cpu.gpr[4],
        // TuneStop(tp, long stopFlags)
        PpcTuneOp::Stop => {
            player.stop();
            sound.manager.with_mut(|manager| manager.stop_tune_channel(instance));
        }
        // TuneQueue(tp, tune, tuneRate, startPosition, stopPosition,
        //           queueFlags, callBackProc, refCon)
        PpcTuneOp::Queue => {
            let tune_ptr = cpu.gpr[4];
            let words =
                crate::trap::read_tune_stream_with(tune_ptr, |address| read_long(memory, address));
            let installed = &sound.tunes.installed;
            if crate::trap::queue_tune_stream(player, installed, tune_ptr, &words, tick, trace) {
                ppc_start_due_tune_segment(sound, instance, tick);
            }
        }
        // TuneGetStatus(tp, TuneStatus *status): tune, tunePtr, time,
        // queueCount (short), queueSpots (short), queueTime, reserved[3].
        PpcTuneOp::GetStatus => {
            ppc_start_due_tune_segment(sound, instance, tick);
            let Some(player) = sound.tunes.players.get(&instance) else {
                return BAD_COMPONENT_INSTANCE;
            };
            let status = cpu.gpr[4];
            if status != 0 && ppc_memory_can_write_bytes(memory, status, 32) {
                let tune = player.current_tune();
                let _ = memory.write_u32_be(status, tune);
                let _ = memory.write_u32_be(status + 4, tune);
                let _ = memory.write_u32_be(status + 8, 0);
                let _ = memory.write_u16_be(status + 12, player.queue_count());
                let _ = memory.write_u16_be(status + 14, player.queue_spots());
                for offset in [16, 20, 24, 28] {
                    let _ = memory.write_u32_be(status + offset, 0);
                }
            }
        }
        // Accepted with noErr and no state change, as on the 68K path.
        PpcTuneOp::Preroll | PpcTuneOp::Unroll => {}
        PpcTuneOp::NoteAllocator => unreachable!("answered above"),
    }
    0
}

/// Retire a finished segment and hand the next to the mixer.
fn ppc_start_due_tune_segment(sound: &mut PpcSoundState, instance: u32, tick: u32) {
    let started = sound
        .tunes
        .players
        .get_mut(&instance)
        .and_then(|player| player.advance(tick));
    if let Some((samples, rate)) = started {
        sound
            .manager
            .with_mut(|manager| manager.play_tune_samples(instance, samples, rate));
    }
}
