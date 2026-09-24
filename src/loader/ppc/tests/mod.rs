use super::*;
use crate::loader::pef::PefResolvedImport;

// Low-level allocator tests borrow adapter fields independently. Keep the
// cursor canonical by publishing the temporary value when each call ends.
macro_rules! test_heap_cursor {
    ($app:ident) => {
        &mut *$app.process_memory_manager.heap_cursor_mut()
    };
}

macro_rules! test_handles {
    ($app:ident) => {
        &mut *$app.process_memory_manager.handles_mut()
    };
}

macro_rules! test_handle_records {
    ($app:ident) => {
        $app.process_memory_manager.handles()
    };
}

// Read the canonical heap limit through the disjoint manager field.
macro_rules! test_heap_limit {
    ($app:ident) => {
        $app.process_memory_manager.heap_limit($app.stack_base)
    };
}

macro_rules! with_test_screen_clut {
    ($app:ident, |$screen:ident| $body:expr) => {{
        let screen_clut = $app.screen_clut.shared_handle();
        screen_clut.with_mut(|$screen| $body)
    }};
}

macro_rules! with_test_color_manager_clut {
    ($app:ident, |$logical:ident| $body:expr) => {{
        let color_manager_clut = $app.color_manager_clut.shared_handle();
        color_manager_clut.with_mut(|$logical| $body)
    }};
}

macro_rules! with_test_display_cluts {
    ($app:ident, |$screen:ident, $logical:ident| $body:expr) => {{
        let screen_clut = $app.screen_clut.shared_handle();
        let color_manager_clut = $app.color_manager_clut.shared_handle();
        screen_clut.with_mut(|$screen| {
            color_manager_clut.with_mut(|$logical| $body)
        })
    }};
}

macro_rules! with_test_controls {
    ($app:ident, |$controls:ident| $body:expr) => {{
        let controls = $app.controls.shared_handle();
        controls.with_mut(|$controls| $body)
    }};
}
use crate::cpu::{CpuOps, Register};
use crate::managers::resource::serialize_resource_fork;
use crate::memory::MemoryBus;
use crate::trap::test_helpers::{setup_with_port, TEST_SP};
use ppc::PpcMemory;

fn test_q3_object(object: u32, object_type: u32) -> PpcQ3ObjectRecord {
    PpcQ3ObjectRecord {
        object,
        kind: PpcQ3ObjectKind::Generic,
        object_type,
        source: PpcQ3ObjectSource::default(),
        data_ptr: 0,
        data_size: 0,
    }
}

fn run_test_import(loaded: &mut PpcLoadedApp, target: PpcImportDispatcherTarget) {
    loaded.cpu.pc = loaded.entry_pc;
    loaded.cpu.lr = PPC_HALT_PC;
    loaded.imports[0].dispatcher_target = target;
    let probe = loaded.run_with_hle_imports(64);
    assert_eq!(probe.handled_import_count, 1);
    assert_eq!(probe.unsupported_import_index, None);
    drain_test_m68k_guest_calls(loaded);
}

fn initialize_test_cgraf_port(bus: &mut MacMemoryBus, port: u32) {
    bus.write_word(port + 6, 0xc000);
    for offset in [36, 38, 40] {
        bus.write_word(port + offset, 0);
    }
    for offset in [42, 44, 46] {
        bus.write_word(port + offset, u16::MAX);
    }
    bus.write_word(port + PPC_CGRAF_PORT_PN_SIZE_OFFSET, 1);
    bus.write_word(port + PPC_CGRAF_PORT_PN_SIZE_OFFSET + 2, 1);
    bus.write_word(port + PPC_CGRAF_PORT_PN_MODE_OFFSET, PPC_QD_PEN_MODE_PAT_COPY as u16);
    bus.write_word(
        port + PPC_CGRAF_PORT_TX_MODE_OFFSET,
        PPC_QD_TEXT_MODE_SRC_OR as u16,
    );
    bus.write_word(
        port + PPC_CGRAF_PORT_TX_SIZE_OFFSET,
        PPC_QD_TEXT_SIZE_SYSTEM as u16,
    );
}

fn run_test_import_with_process_memory_manager(
    loaded: &mut PpcLoadedApp,
    target: PpcImportDispatcherTarget,
    memory_manager: &SharedProcessMemoryManager,
) {
    loaded.cpu.pc = loaded.entry_pc;
    loaded.cpu.lr = PPC_HALT_PC;
    loaded.imports[0].dispatcher_target = target;
    let probe = loaded.run_with_process_memory_manager(
        64,
        false,
        false,
        &mut memory_manager.borrow_mut(),
    );
    assert_eq!(probe.handled_import_count, 1);
    assert_eq!(probe.unsupported_import_index, None);
    drain_test_m68k_guest_calls(loaded);
}

fn attach_test_classic_heap(
    native: &mut PpcLoadedApp,
    context: &mut ProcessContext,
    ram_size: usize,
    heap_len: usize,
) -> MacMemoryBus {
    let mut classic_bus = MacMemoryBus::new(ram_size);
    context.attach_classic_memory_bus(&mut classic_bus);
    let classic_heap = classic_bus
        .shared_ram_region(0x0020_0000, heap_len as u32)
        .expect("classic adapter owns its heap range");
    context.attach_memory(0x0020_0000, classic_heap, &mut native.memory);
    classic_bus
}

fn drain_test_m68k_guest_calls(loaded: &mut PpcLoadedApp) {
    while let Some(pending) = loaded.guest_calls()
        .active_m68k()
        .or_else(|| loaded.guest_calls().activate_m68k())
    {
        let mut cpu = m68k::CpuCore::new();
        cpu.set_cpu_type(REFERENCE_MACHINE_PROFILE.cpu_type());
        cpu.pc = pending.entry;
        cpu.set_a(7, pending.initial_sp);
        for (index, value) in pending.registers.data.into_iter().enumerate() {
            cpu.set_d(index, value);
        }
        for (index, value) in pending.registers.address.into_iter().enumerate() {
            cpu.set_a(index, value);
        }
        let result = cpu.run_batch(&mut loaded.memory, 4096, &[pending.return_pc]);
        assert_eq!(
            result.exit,
            m68k::BatchExit::WatchedPc {
                pc: pending.return_pc
            },
            "entry=${:08X} pc=${:08X} sp=${:08X} return=${:08X}",
            pending.entry,
            cpu.pc,
            cpu.a(7),
            pending.return_pc,
        );
        let result = match pending.result {
            None => None,
            Some(crate::guest_call::M68kResultSource::Data(index)) => {
                Some(cpu.d(usize::from(index)))
            }
            Some(crate::guest_call::M68kResultSource::Address(index)) => {
                Some(cpu.a(usize::from(index)))
            }
            Some(crate::guest_call::M68kResultSource::Memory { address, size }) => {
                Some(match size {
                    1 => u32::from(loaded.memory.read_u8(address).unwrap()),
                    2 => u32::from(loaded.memory.read_u16_be(address).unwrap()),
                    4 => loaded.memory.read_u32_be(address).unwrap(),
                    _ => panic!("unsupported 68k result size {size}"),
                })
            }
            Some(crate::guest_call::M68kResultSource::SpecialCase { selector, .. }) => {
                panic!("special-case result selector {selector} requires the shared runner")
            }
        };
        assert!(loaded
            .toolbox_startup
            .execution
            .calls()
            .complete_m68k_operation_for_powerpc(
            cpu.pc,
            cpu.a(7),
            result,
            &mut loaded.cpu,
            &mut loaded.memory,
            loaded.process_memory_manager.0.borrow_mut().native_mut(),
        ));

        if loaded.cpu.pc == PPC_GUEST_CALL_RETURN_PC
            || (loaded.cpu.pc >= loaded.import_trap_base
                && loaded.cpu.pc
                    < loaded
                        .import_trap_base
                        .saturating_add(loaded.import_count.saturating_mul(4)))
        {
            let probe = loaded.run_with_hle_imports(64);
            assert_eq!(probe.unsupported_import_index, None);
        }
    }
}

mod palette_manager;
mod gworld;
mod blit;

mod file_manager;
mod event_manager;

mod text_edit;
mod scrap_manager;
mod list_manager;

mod standard_c_library;
mod toolbox_utilities;

mod thread_manager;

mod process_manager;


mod font_manager;

mod gestalt;

mod display_depth;

mod desk_manager;

mod device_manager;

#[path = "native_exceptions.rs"]
mod native_exceptions;

mod collection_manager;

mod draw_sprocket;

mod input_sprocket;
mod qd3d;
mod menu_manager;
pub(crate) use menu_manager::*;
mod window_manager;
use window_manager::create_test_cwindow;
mod mixed_mode;
pub(crate) use mixed_mode::*;
mod quicktime;
mod memory_manager;
mod resource_manager;
mod process_services;
mod trap_manager;
mod code_fragment_manager;
pub(crate) use code_fragment_manager::synthetic_pef_with_enumerable_exports;
mod quickdraw;



mod sound_manager;
fn establish_loaded_reservation(loaded: &mut PpcLoadedApp, address: u32) {
    const LWARX_R12_R4_R5: u32 =
        (31 << 26) | (12 << 21) | (4 << 16) | (5 << 11) | (20 << 1);
    let preserved = (
        loaded.cpu.pc,
        loaded.cpu.gpr[4],
        loaded.cpu.gpr[5],
        loaded.cpu.gpr[12],
    );
    loaded.cpu.gpr[4] = address;
    loaded.cpu.gpr[5] = 0;
    assert_eq!(
        loaded.cpu.step(&mut loaded.memory, LWARX_R12_R4_R5),
        ppc::PpcStepResult::Stepped
    );
    assert_eq!(loaded.cpu.reservation_address(), Some(address));
    (
        loaded.cpu.pc,
        loaded.cpu.gpr[4],
        loaded.cpu.gpr[5],
        loaded.cpu.gpr[12],
    ) = preserved;
}

mod interrupt_callbacks;

mod dialog_parameters;
mod dialog_manager;
mod control_manager;

mod loader_configuration;

pub(crate) fn synthetic_pef() -> Vec<u8> {
    synthetic_pef_with_import(b"TestImport")
}

fn synthetic_pef_with_initializer() -> Vec<u8> {
    let mut pef = synthetic_pef();
    // The synthetic loader section begins at $80. Reuse its main TVector
    // as a valid initializer TVector so loading exercises CFM's startup
    // fragment and InitBlock allocations without executing the code.
    write_i32(&mut pef, 0x80 + 8, 1);
    write_u32(&mut pef, 0x80 + 12, 0);
    pef
}

fn assert_ppc_bytes_equal(memory: &mut PpcSectionMem, start: u32, len: u32, expected: u8) {
    for offset in 0..len {
        assert_eq!(memory.read_u8(start + offset), Some(expected));
    }
}

mod standard_c_library_integration;

pub(crate) fn synthetic_pef_with_import(symbol_name: &[u8]) -> Vec<u8> {
    synthetic_pef_with_library_import(b"InterfaceLib", symbol_name)
}

fn synthetic_pef_with_import_class(symbol_name: &[u8], symbol_class: u8) -> Vec<u8> {
    synthetic_pef_with_loader(synthetic_loader_with_symbol_class(
        b"InterfaceLib",
        symbol_name,
        symbol_class,
        &[sm_index_reloc(0x30, 0)],
    ))
}

fn write_test_dsp_context_attributes(
    memory: &mut PpcSectionMem,
    attributes: u32,
    context_attributes: PpcDspContextAttributes,
) {
    memory
        .write_u32_be(attributes, context_attributes.frequency)
        .unwrap();
    memory
        .write_u32_be(attributes + 4, context_attributes.width)
        .unwrap();
    memory
        .write_u32_be(attributes + 8, context_attributes.height)
        .unwrap();
    memory
        .write_u32_be(attributes + 28, context_attributes.context_options)
        .unwrap();
    memory
        .write_u32_be(
            attributes + 32,
            context_attributes.back_buffer_best_depth_mask,
        )
        .unwrap();
    memory
        .write_u32_be(attributes + 36, context_attributes.display_best_depth_mask)
        .unwrap();
    memory
        .write_u32_be(attributes + 40, context_attributes.back_buffer_depth)
        .unwrap();
    memory
        .write_u32_be(attributes + 44, context_attributes.display_depth)
        .unwrap();
    memory
        .write_u32_be(attributes + 48, context_attributes.page_count)
        .unwrap();
}

fn synthetic_pef_with_library_import(library_name: &[u8], symbol_name: &[u8]) -> Vec<u8> {
    synthetic_pef_with_loader(synthetic_loader(library_name, symbol_name))
}

fn synthetic_pef_with_reloc_chunks(
    library_name: &[u8],
    symbol_name: &[u8],
    chunks: &[u16],
) -> Vec<u8> {
    synthetic_pef_with_loader(synthetic_loader_with_chunks(
        library_name,
        symbol_name,
        chunks,
    ))
}

fn synthetic_pef_with_loader(loader: Vec<u8>) -> Vec<u8> {
    synthetic_pef_with_loader_and_data(loader, &[0; 8])
}

fn synthetic_pef_with_loader_and_data(loader: Vec<u8>, data: &[u8]) -> Vec<u8> {
    let code = synthetic_code();
    let loader_offset = 0x80usize;
    let code_offset = align_test_offset((loader_offset + loader.len()).max(0x100), 0x10);
    let data_offset = code_offset + code.len();
    let total_len = data_offset + data.len();
    let mut bytes = vec![0u8; total_len.max(loader_offset + loader.len())];

    bytes[0..4].copy_from_slice(b"Joy!");
    bytes[4..8].copy_from_slice(b"peff");
    bytes[8..12].copy_from_slice(b"pwpc");
    write_u32(&mut bytes, 12, 1);
    write_u16(&mut bytes, 32, 3);
    write_u16(&mut bytes, 34, 2);

    write_section(
        &mut bytes,
        0,
        SectionSpec {
            total_size: code.len() as u32,
            unpacked_size: code.len() as u32,
            packed_size: code.len() as u32,
            container_offset: code_offset as u32,
            section_kind: SECTION_KIND_CODE,
        },
    );
    write_section(
        &mut bytes,
        1,
        SectionSpec {
            total_size: data.len() as u32,
            unpacked_size: data.len() as u32,
            packed_size: data.len() as u32,
            container_offset: data_offset as u32,
            section_kind: SECTION_KIND_UNPACKED_DATA,
        },
    );
    write_section(
        &mut bytes,
        2,
        SectionSpec {
            total_size: 0,
            unpacked_size: 0,
            packed_size: loader.len() as u32,
            container_offset: loader_offset as u32,
            section_kind: super::super::pef::SECTION_KIND_LOADER,
        },
    );

    bytes[loader_offset..loader_offset + loader.len()].copy_from_slice(&loader);
    bytes[code_offset..code_offset + code.len()].copy_from_slice(&code);
    bytes[data_offset..data_offset + data.len()].copy_from_slice(&data);
    bytes
}

fn align_test_offset(value: usize, align: usize) -> usize {
    (value + align - 1) & !(align - 1)
}

fn synthetic_code() -> Vec<u8> {
    let mut code = Vec::new();
    let hi = (PPC_IMPORT_TRAP_BASE >> 16) as u16;
    let lo = (PPC_IMPORT_TRAP_BASE & 0xffff) as u16;
    for word in [
        d_form_u(15, 12, 0, hi),       // lis r12, trap@ha
        d_form_u(24, 12, 12, lo),      // ori r12, r12, trap@l
        xfx_form(31, 12, 9, 467),      // mtctr r12
        xl_form(19, 20, 0, 528, true), // bctrl
        d_form_u(14, 0, 0, 0),         // li r0, 0
        xfx_form(31, 0, 8, 467),       // mtlr r0
        BLR,
    ] {
        code.extend_from_slice(&word.to_be_bytes());
    }
    code
}

fn synthetic_loader(library_name: &[u8], symbol_name: &[u8]) -> Vec<u8> {
    let chunks = [run_reloc(0x23, 1)];
    synthetic_loader_with_chunks(library_name, symbol_name, &chunks)
}

fn synthetic_loader_with_chunks(
    library_name: &[u8],
    symbol_name: &[u8],
    chunks: &[u16],
) -> Vec<u8> {
    synthetic_loader_with_symbol_class(library_name, symbol_name, 0x02, chunks)
}

fn synthetic_loader_with_symbol_class(
    library_name: &[u8],
    symbol_name: &[u8],
    symbol_class: u8,
    chunks: &[u16],
) -> Vec<u8> {
    let mut strings = Vec::new();
    let library_name = push_c_string(&mut strings, library_name);
    let symbol_name = push_c_string(&mut strings, symbol_name);
    let reloc_header_offset = 56 + 24 + 4;
    let reloc_instr_offset = reloc_header_offset + 12;
    let strings_offset = reloc_instr_offset + chunks.len() * 2;
    let mut bytes = vec![0u8; strings_offset + strings.len()];

    write_i32(&mut bytes, 0, 1);
    write_u32(&mut bytes, 4, 0);
    write_i32(&mut bytes, 8, -1);
    write_i32(&mut bytes, 16, -1);
    write_u32(&mut bytes, 24, 1);
    write_u32(&mut bytes, 28, 1);
    write_u32(&mut bytes, 32, 1);
    write_u32(&mut bytes, 36, reloc_instr_offset as u32);
    write_u32(&mut bytes, 40, strings_offset as u32);

    write_u32(&mut bytes, 56, library_name);
    write_u32(&mut bytes, 56 + 12, 1);
    write_symbol(&mut bytes, 56 + 24, symbol_class, symbol_name);

    write_u16(&mut bytes, reloc_header_offset, 1);
    write_u32(&mut bytes, reloc_header_offset + 4, chunks.len() as u32);
    write_u32(&mut bytes, reloc_header_offset + 8, 0);
    for (index, chunk) in chunks.iter().enumerate() {
        write_u16(&mut bytes, reloc_instr_offset + index * 2, *chunk);
    }

    bytes[strings_offset..].copy_from_slice(&strings);
    bytes
}

fn synthetic_loader_with_repeated_imports(import_count: u32) -> Vec<u8> {
    let mut strings = Vec::new();
    let library_name = push_c_string(&mut strings, b"InterfaceLib");
    let symbol_name = push_c_string(&mut strings, b"TickCount");
    let symbol_count = usize::try_from(import_count).unwrap();
    let strings_offset = 56 + 24 + symbol_count * 4;
    let mut bytes = vec![0u8; strings_offset + strings.len()];

    write_i32(&mut bytes, 0, 1);
    write_i32(&mut bytes, 8, -1);
    write_i32(&mut bytes, 16, -1);
    write_u32(&mut bytes, 24, 1);
    write_u32(&mut bytes, 28, import_count);
    write_u32(&mut bytes, 36, strings_offset as u32);
    write_u32(&mut bytes, 40, strings_offset as u32);
    write_u32(&mut bytes, 56, library_name);
    write_u32(&mut bytes, 56 + 12, import_count);
    for index in 0..symbol_count {
        write_symbol(&mut bytes, 56 + 24 + index * 4, 2, symbol_name);
    }
    bytes[strings_offset..].copy_from_slice(&strings);
    bytes
}

fn synthetic_loader_with_overlapping_library_ranges() -> Vec<u8> {
    let mut strings = Vec::new();
    let first_library = push_c_string(&mut strings, b"InterfaceLib");
    let second_library = push_c_string(&mut strings, b"StdCLib");
    let symbol = push_c_string(&mut strings, b"errno");
    let reloc_header_offset = 56 + 2 * 24 + 4;
    let reloc_instr_offset = reloc_header_offset + 12;
    let strings_offset = reloc_instr_offset + 2;
    let mut bytes = vec![0u8; strings_offset + strings.len()];

    write_i32(&mut bytes, 0, 1);
    write_i32(&mut bytes, 8, -1);
    write_i32(&mut bytes, 16, -1);
    write_u32(&mut bytes, 24, 2);
    write_u32(&mut bytes, 28, 1);
    write_u32(&mut bytes, 32, 1);
    write_u32(&mut bytes, 36, reloc_instr_offset as u32);
    write_u32(&mut bytes, 40, strings_offset as u32);

    write_u32(&mut bytes, 56, first_library);
    write_u32(&mut bytes, 56 + 12, 1);
    write_u32(&mut bytes, 56 + 24, second_library);
    write_u32(&mut bytes, 56 + 24 + 12, 1);
    write_symbol(&mut bytes, 56 + 2 * 24, 2, symbol);

    write_u16(&mut bytes, reloc_header_offset, 1);
    write_u32(&mut bytes, reloc_header_offset + 4, 1);
    write_u32(&mut bytes, reloc_header_offset + 8, 0);
    write_u16(&mut bytes, reloc_instr_offset, sm_index_reloc(0x30, 0));
    bytes[strings_offset..].copy_from_slice(&strings);
    bytes
}

fn synthetic_pef_with_exports() -> Vec<u8> {
    let names = [b"tvector".as_slice(), b"absolute", b"reexport"];
    let mut strings = Vec::new();
    let mut name_offsets = Vec::new();
    for name in names {
        name_offsets.push(strings.len() as u32);
        strings.extend_from_slice(name);
        strings.push(0);
    }

    let strings_offset = 56usize;
    let export_hash_offset = strings_offset + strings.len();
    let key_table_offset = export_hash_offset + 4;
    let symbol_table_offset = key_table_offset + names.len() * 4;
    let loader_len = symbol_table_offset + names.len() * 10;
    let mut loader = vec![0; loader_len];
    write_i32(&mut loader, 0, -1);
    write_i32(&mut loader, 8, -1);
    write_i32(&mut loader, 16, -1);
    write_u32(&mut loader, 40, strings_offset as u32);
    write_u32(&mut loader, 44, export_hash_offset as u32);
    write_u32(&mut loader, 48, 0);
    write_u32(&mut loader, 52, names.len() as u32);
    loader[strings_offset..export_hash_offset].copy_from_slice(&strings);

    // All three entries occupy one valid hash chain. The resolver uses
    // the flattened export table, but the parser still validates chains.
    write_u32(&mut loader, export_hash_offset, 3 << 18);
    for (index, name) in names.iter().enumerate() {
        write_u32(
            &mut loader,
            key_table_offset + index * 4,
            (name.len() as u32) << 16,
        );
    }
    let records = [
        (2u8, name_offsets[0], 0u32, 1i16),
        (1u8, name_offsets[1], 0x1234_5678, -2i16),
        (2u8, name_offsets[2], 0u32, -3i16),
    ];
    for (index, (class, name_offset, value, section_index)) in records.into_iter().enumerate() {
        let base = symbol_table_offset + index * 10;
        write_u32(&mut loader, base, (u32::from(class) << 24) | name_offset);
        write_u32(&mut loader, base + 4, value);
        write_u16(&mut loader, base + 8, section_index as u16);
    }
    synthetic_pef_with_loader_and_data(loader, &[0; 8])
}

fn write_ppc_pstring(memory: &mut PpcSectionMem, addr: u32, bytes: &[u8]) {
    assert!(bytes.len() <= 255);
    memory.write_u8(addr, bytes.len() as u8).unwrap();
    for (offset, byte) in bytes.iter().enumerate() {
        memory.write_u8(addr + 1 + offset as u32, *byte).unwrap();
    }
}

fn compressed_resource_bytes(decompressed_len: u32, body: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(b"\xA8\x9Fer");
    out.extend_from_slice(&0x12u16.to_be_bytes());
    out.extend_from_slice(&0x0801u16.to_be_bytes());
    out.extend_from_slice(&decompressed_len.to_be_bytes());
    out.extend_from_slice(&[0x80, 0x03]);
    out.extend_from_slice(&0i16.to_be_bytes());
    out.extend_from_slice(&0u16.to_be_bytes());
    out.extend_from_slice(body);
    out
}

fn noncanonical_single_resource_fork_bytes(
    res_type: [u8; 4],
    res_id: i16,
    data: &[u8],
    attrs: u8,
    map_attrs: u16,
) -> Vec<u8> {
    let data_offset = 16u32;
    let data_length = (4 + data.len()) as u32;
    let map_offset = data_offset + data_length;
    let type_list_offset = 30u16;
    let ref_list_offset = 10u16;
    let name_list_offset = 40u16;
    let map_length = 52u32;

    let mut bytes = vec![0u8; (map_offset + map_length) as usize];
    let mut header = [0u8; 16];
    header[0..4].copy_from_slice(&data_offset.to_be_bytes());
    header[4..8].copy_from_slice(&map_offset.to_be_bytes());
    header[8..12].copy_from_slice(&data_length.to_be_bytes());
    header[12..16].copy_from_slice(&map_length.to_be_bytes());
    bytes[0..16].copy_from_slice(&header);

    let data_start = data_offset as usize;
    bytes[data_start..data_start + 4].copy_from_slice(&(data.len() as u32).to_be_bytes());
    bytes[data_start + 4..data_start + 4 + data.len()].copy_from_slice(data);

    let map_start = map_offset as usize;
    bytes[map_start..map_start + 16].copy_from_slice(&header);
    bytes[map_start + 22..map_start + 24].copy_from_slice(&map_attrs.to_be_bytes());
    bytes[map_start + 24..map_start + 26].copy_from_slice(&type_list_offset.to_be_bytes());
    bytes[map_start + 26..map_start + 28].copy_from_slice(&name_list_offset.to_be_bytes());

    let type_list_start = map_start + type_list_offset as usize;
    bytes[type_list_start..type_list_start + 2].copy_from_slice(&0u16.to_be_bytes());
    bytes[type_list_start + 2..type_list_start + 6].copy_from_slice(&res_type);
    bytes[type_list_start + 6..type_list_start + 8].copy_from_slice(&0u16.to_be_bytes());
    bytes[type_list_start + 8..type_list_start + 10]
        .copy_from_slice(&ref_list_offset.to_be_bytes());

    let ref_list_start = type_list_start + ref_list_offset as usize;
    bytes[ref_list_start..ref_list_start + 2].copy_from_slice(&(res_id as u16).to_be_bytes());
    bytes[ref_list_start + 2..ref_list_start + 4].copy_from_slice(&0xffffu16.to_be_bytes());
    bytes[ref_list_start + 4] = attrs;
    bytes[ref_list_start + 5..ref_list_start + 8].copy_from_slice(&0u32.to_be_bytes()[1..4]);
    bytes
}

fn test_sound_file_playback(channel: u32) -> PpcSoundFilePlaybackRecord {
    PpcSoundFilePlaybackRecord {
        channel,
        ref_num: PPC_FIRST_FILE_REF_NUM,
        resource_id: 0,
        buffer_size: 0,
        buffer: 0,
        selection: 0,
        completion: 0,
        completion_command: None,
        async_play: false,
        aiff: None,
        decoded_aiff: None,
    }
}

fn write_ppc_fsspec(
    memory: &mut PpcSectionMem,
    addr: u32,
    vref: i16,
    dir_id: u32,
    name: &[u8],
) {
    memory.write_u16_be(addr, vref as u16).unwrap();
    memory.write_u32_be(addr + 2, dir_id).unwrap();
    write_ppc_pstring(memory, addr + 6, name);
}

pub(crate) fn test_v1_one_bit_packbits_pict() -> Vec<u8> {
    fn push_u16(bytes: &mut Vec<u8>, value: u16) {
        bytes.extend_from_slice(&value.to_be_bytes());
    }

    let mut bytes = Vec::new();
    push_u16(&mut bytes, 0); // patched picSize
    for value in [0i16, 0, 1, 8] {
        push_u16(&mut bytes, value as u16);
    }
    bytes.push(0x11); // versionOp
    bytes.push(0x01); // version 1
    bytes.push(0x98); // PackBitsRect
    push_u16(&mut bytes, 1); // rowBytes < 8: unpacked bitmap data
    for value in [0i16, 0, 1, 8] {
        push_u16(&mut bytes, value as u16);
    }
    for _ in 0..2 {
        for value in [0i16, 0, 1, 8] {
            push_u16(&mut bytes, value as u16);
        }
    }
    push_u16(&mut bytes, 0); // srcCopy
    bytes.push(0b1010_0000);
    bytes.push(0xff); // EndOfPicture
    let size = u16::try_from(bytes.len()).unwrap();
    bytes[0..2].copy_from_slice(&size.to_be_bytes());
    bytes
}

fn test_one_bit_cicon() -> Vec<u8> {
    let mut data = vec![0u8; 101];
    data[4..6].copy_from_slice(&0x8001u16.to_be_bytes());
    data[10..12].copy_from_slice(&1u16.to_be_bytes());
    data[12..14].copy_from_slice(&8u16.to_be_bytes());
    data[32..34].copy_from_slice(&1u16.to_be_bytes());
    data[34..36].copy_from_slice(&1u16.to_be_bytes());
    data[36..38].copy_from_slice(&1u16.to_be_bytes());

    data[54..56].copy_from_slice(&1u16.to_be_bytes());
    data[60..62].copy_from_slice(&1u16.to_be_bytes());
    data[62..64].copy_from_slice(&8u16.to_be_bytes());
    data[68..70].copy_from_slice(&1u16.to_be_bytes());
    data[74..76].copy_from_slice(&1u16.to_be_bytes());
    data[76..78].copy_from_slice(&8u16.to_be_bytes());

    data[82] = 0xf0; // first four pixels are opaque
    data[83] = 0x00; // monochrome fallback
    data[90..92].copy_from_slice(&0u16.to_be_bytes()); // one ColorSpec
    data[92..94].copy_from_slice(&0u16.to_be_bytes()); // pixel value 0
    data[94..96].copy_from_slice(&0xffffu16.to_be_bytes()); // red
    data[96..98].copy_from_slice(&0u16.to_be_bytes());
    data[98..100].copy_from_slice(&0u16.to_be_bytes());
    data[100] = 0x00; // eight one-bit red pixels
    data
}

fn compatibility_binding(
    library_name: &str,
    symbol_name: &str,
    dispatcher_target: PpcImportDispatcherTarget,
) -> PpcImportBinding {
    PpcImportBinding {
        library_index: 0,
        symbol_index: 0,
        library_name: library_name.to_string(),
        symbol_name: symbol_name.to_string(),
        class: 0,
        weak: false,
        address: 0,
        tvector_address: None,
        trap_pc: 0,
        dispatcher_target,
    }
}

mod apple_event_handlers;

mod apple_event_descriptors;

mod hardware_compatibility;

mod math_compatibility;

mod fixmath;

struct SectionSpec {
    total_size: u32,
    unpacked_size: u32,
    packed_size: u32,
    container_offset: u32,
    section_kind: u8,
}

fn write_section(bytes: &mut [u8], index: usize, spec: SectionSpec) {
    let off = 40 + index * 28;
    write_i32(bytes, off, -1);
    write_u32(bytes, off + 8, spec.total_size);
    write_u32(bytes, off + 12, spec.unpacked_size);
    write_u32(bytes, off + 16, spec.packed_size);
    write_u32(bytes, off + 20, spec.container_offset);
    bytes[off + 24] = spec.section_kind;
    bytes[off + 25] = 4;
    bytes[off + 26] = 4;
}

fn push_c_string(strings: &mut Vec<u8>, value: &[u8]) -> u32 {
    let offset = strings.len() as u32;
    strings.extend_from_slice(value);
    strings.push(0);
    offset
}

fn write_symbol(bytes: &mut [u8], offset: usize, class_byte: u8, name_offset: u32) {
    bytes[offset] = class_byte;
    bytes[offset + 1] = ((name_offset >> 16) & 0xff) as u8;
    bytes[offset + 2] = ((name_offset >> 8) & 0xff) as u8;
    bytes[offset + 3] = (name_offset & 0xff) as u8;
}

fn run_reloc(dispatch: u8, run_length: u16) -> u16 {
    (u16::from(dispatch) << 9) | ((run_length - 1) & 0x01ff)
}

fn sm_index_reloc(dispatch: u8, index: u16) -> u16 {
    (u16::from(dispatch) << 9) | (index & 0x01ff)
}

fn delt(offset: u16) -> u16 {
    (0x40u16 << 9) | ((offset - 1) & 0x0fff)
}

fn d_form_u(opcd: u8, rt: u8, ra: u8, value: u16) -> u32 {
    ((opcd as u32) << 26)
        | ((rt as u32 & 0x1f) << 21)
        | ((ra as u32 & 0x1f) << 16)
        | u32::from(value)
}

fn x_form(opcd: u8, rt: u8, ra: u8, rb: u8, xo: u16, rc: bool) -> u32 {
    ((opcd as u32 & 0x3f) << 26)
        | ((rt as u32 & 0x1f) << 21)
        | ((ra as u32 & 0x1f) << 16)
        | ((rb as u32 & 0x1f) << 11)
        | ((xo as u32 & 0x3ff) << 1)
        | u32::from(rc)
}

fn a_form_fp(opcd: u8, frt: u8, fra: u8, frb: u8, frc: u8, xo_5: u8, rc: bool) -> u32 {
    ((opcd as u32 & 0x3f) << 26)
        | ((frt as u32 & 0x1f) << 21)
        | ((fra as u32 & 0x1f) << 16)
        | ((frb as u32 & 0x1f) << 11)
        | ((frc as u32 & 0x1f) << 6)
        | ((xo_5 as u32 & 0x1f) << 1)
        | u32::from(rc)
}

fn xfx_form(opcd: u8, rt_or_rs: u8, spr_decimal: u16, xo: u16) -> u32 {
    let high_5 = (spr_decimal >> 5) & 0x1f;
    let low_5 = spr_decimal & 0x1f;
    ((opcd as u32 & 0x3f) << 26)
        | ((rt_or_rs as u32 & 0x1f) << 21)
        | ((low_5 as u32) << 16)
        | ((high_5 as u32) << 11)
        | ((xo as u32 & 0x3ff) << 1)
}

fn xl_form(opcd: u8, bo: u8, bi: u8, xo: u16, lk: bool) -> u32 {
    ((opcd as u32 & 0x3f) << 26)
        | ((bo as u32 & 0x1f) << 21)
        | ((bi as u32 & 0x1f) << 16)
        | ((xo as u32 & 0x3ff) << 1)
        | u32::from(lk)
}

fn write_i32(bytes: &mut [u8], offset: usize, value: i32) {
    bytes[offset..offset + 4].copy_from_slice(&value.to_be_bytes());
}

fn write_u32(bytes: &mut [u8], offset: usize, value: u32) {
    bytes[offset..offset + 4].copy_from_slice(&value.to_be_bytes());
}

fn write_u16(bytes: &mut [u8], offset: usize, value: u16) {
    bytes[offset..offset + 2].copy_from_slice(&value.to_be_bytes());
}

#[test]
fn get_resource_searches_files_opened_before_the_current_one_first() {
    let txst = u32::from_be_bytes(*b"TxSt");
    let record = |ref_num: i16, path: &str| PpcVfsResourceRecord {
        ref_num,
        path: path.to_string(),
        res_type: txst,
        res_id: 128,
        name: Vec::new(),
        data: Vec::new(),
        raw_data: None,
        raw_attrs: None,
        attrs: 0,
        handle: 0,
    };
    // The application's fork, then 'Cythera Data' (128), the preferences
    // (129) and a file opened later (130).
    let resources = [
        record(0, "Cythera"),
        record(130, "Later"),
        record(128, "Cythera Data"),
    ];
    let found = |current| ppc_vfs_resource_index(&resources, current, txst, 128, false);
    assert_eq!(found(129), Some(2));
    assert_eq!(found(128), Some(2));
    assert_eq!(found(0), Some(0));
    assert_eq!(ppc_vfs_resource_index(&resources, 129, txst, 128, true), None);
}

#[test]
fn drag_gray_rgn_pins_the_offset_to_limit_rect_and_gives_up_outside_slop_rect() {
    let limit = (0, 0, 600, 800);
    let slop = (-20, -20, 620, 820);
    let start = (463, 160);
    assert_eq!(ppc_drag_gray_rgn_offset(start, (403, 200), limit, slop, 0), Some((-60, 40)));
    // Past limitRect but inside slopRect: the offset point stops at the edge.
    assert_eq!(ppc_drag_gray_rgn_offset(start, (-10, 160), limit, slop, 0), Some((-463, 0)));
    assert_eq!(ppc_drag_gray_rgn_offset(start, (463, 815), limit, slop, 0), Some((0, 639)));
    // Outside slopRect: both words $8000.
    assert_eq!(ppc_drag_gray_rgn_offset(start, (463, 900), limit, slop, 0), None);
    // hAxisOnly and vAxisOnly.
    assert_eq!(ppc_drag_gray_rgn_offset(start, (403, 200), limit, slop, 1), Some((0, 40)));
    assert_eq!(ppc_drag_gray_rgn_offset(start, (403, 200), limit, slop, 2), Some((-60, 0)));
}

#[test]
fn draw_text_is_recorded_into_an_open_picture() {
    let pef = synthetic_pef_with_import(b"OpenPicture");
    let mut loaded = load_pef_application(&pef).unwrap();
    let scratch = PPC_DATA_BASE + 0x1800;
    loaded.memory.add_region(scratch, vec![0; 64]);
    ppc_write_rect(&mut loaded.memory, scratch, 0, 0, 40, 40).unwrap();
    loaded.memory.write_bytes(scratch + 16, b"Human").unwrap();
    let surface =
        ppc_live_quickdraw_surface(&mut loaded.memory, &loaded.gworlds, *loaded.current_gworld)
            .unwrap();
    let before = ppc_quickdraw_read_pixel(
        &mut loaded.memory,
        surface.front_buffer,
        surface.local_point((8, 20)),
    );
    loaded.cpu.gpr[3] = scratch;
    run_test_import(
        &mut loaded,
        PpcImportDispatcherTarget::QuickDrawCompatibility(
            PpcQuickDrawCompatibilityOperation::OpenPicture,
        ),
    );
    let handle = loaded.cpu.gpr[3];
    loaded.cpu.gpr[3] = 4;
    loaded.cpu.gpr[4] = 20;
    run_test_import(&mut loaded, PpcImportDispatcherTarget::MoveTo);
    loaded.cpu.gpr[3] = scratch + 16;
    loaded.cpu.gpr[4] = 0;
    loaded.cpu.gpr[5] = 5;
    run_test_import(&mut loaded, PpcImportDispatcherTarget::DrawText);
    assert_eq!(
        ppc_quickdraw_read_pixel(
            &mut loaded.memory,
            surface.front_buffer,
            surface.local_point((8, 20)),
        ),
        before,
        "recording must not paint into the live port"
    );
    run_test_import(
        &mut loaded,
        PpcImportDispatcherTarget::QuickDrawCompatibility(
            PpcQuickDrawCompatibilityOperation::ClosePicture,
        ),
    );
    let picture =
        ppc_handle_bytes(&mut loaded.memory, &test_handle_records!(loaded), handle).unwrap();
    // LongText: the opcode, the pen at (20, 4), the count, the text.
    let record = [&[0x00, 0x28, 0x00, 0x14, 0x00, 0x04, 0x05][..], b"Human"].concat();
    assert!(
        picture.windows(record.len()).any(|window| window == record),
        "DrawText should record a LongText"
    );
}

#[test]
fn calc_mask_keeps_what_paint_from_the_edges_cannot_reach() {
    let pef = synthetic_pef_with_import(b"CalcMask");
    let mut loaded = load_pef_application(&pef).unwrap();
    let (src, dst) = (PPC_DATA_BASE + 0x1800, PPC_DATA_BASE + 0x1900);
    loaded.memory.add_region(src, vec![0; 0x200]);
    // One word wide, five rows: a closed box at columns 2..=6, rows 1..=3,
    // and a lone pixel at column 12 on row 2.
    let rows: [u16; 5] = [0, 0x3E00, 0x2208, 0x3E00, 0];
    for (row, bits) in rows.iter().enumerate() {
        loaded.memory.write_u16_be(src + row as u32 * 2, *bits).unwrap();
        loaded.memory.write_u16_be(dst + row as u32 * 2, 0xFFFF).unwrap();
    }
    loaded.cpu.gpr[3] = src;
    loaded.cpu.gpr[4] = dst;
    loaded.cpu.gpr[5] = 2;
    loaded.cpu.gpr[6] = 2;
    loaded.cpu.gpr[7] = 5;
    loaded.cpu.gpr[8] = 1;
    run_test_import(&mut loaded, PpcImportDispatcherTarget::CalcMask);
    let mask: Vec<u16> = (0..5)
        .map(|row| loaded.memory.read_u16_be(dst + row * 2).unwrap())
        .collect();
    // The box and its inside are kept; the lone pixel has paint all
    // round it and is kept as itself.
    assert_eq!(mask, [0, 0x3E00, 0x3E08, 0x3E00, 0]);
}

#[test]
fn appearance_root_control_embeds_and_deactivates_what_it_holds() {
    use super::appearance_controls::PpcAppearanceControlOperation as Op;
    let pef = synthetic_pef_with_import(b"CreateRootControl");
    let mut loaded = load_pef_application(&pef).unwrap();
    let scratch = PPC_DATA_BASE + 0x1000;
    loaded.memory.add_region(scratch, vec![0; 0x100]);
    let window = create_test_cwindow(&mut loaded, scratch, (40, 40, 200, 300), 0, true, u32::MAX);
    let out = scratch + 0x40;

    loaded.cpu.gpr[3] = window;
    loaded.cpu.gpr[4] = out;
    run_test_import(&mut loaded, PpcImportDispatcherTarget::AppearanceControl(Op::CreateRootControl));
    assert_eq!(loaded.cpu.gpr[3], 0);
    let root = loaded.memory.read_u32_be(out).unwrap();
    assert_ne!(root, 0);

    ppc_write_rect(&mut loaded.memory, scratch + 0x50, 10, 10, 30, 122).unwrap();
    loaded.cpu.gpr[3] = window;
    loaded.cpu.gpr[4] = scratch + 0x50;
    loaded.cpu.gpr[5] = 0;
    loaded.cpu.gpr[6] = 1;
    loaded.cpu.gpr[7] = 5;
    loaded.cpu.gpr[8] = 0;
    loaded.cpu.gpr[9] = 10;
    loaded.cpu.gpr[10] = 48;
    run_test_import(
        &mut loaded,
        PpcImportDispatcherTarget::LegacyControl(PpcLegacyControlOperation::NewControl),
    );
    let slider = loaded.cpu.gpr[3];
    assert_ne!(slider, 0);

    loaded.cpu.gpr[3] = slider;
    loaded.cpu.gpr[4] = root;
    run_test_import(&mut loaded, PpcImportDispatcherTarget::AppearanceControl(Op::EmbedControl));
    assert_eq!(loaded.cpu.gpr[3], 0);

    loaded.cpu.gpr[3] = root;
    run_test_import(&mut loaded, PpcImportDispatcherTarget::AppearanceControl(Op::DeactivateControl));
    let slider_ptr = loaded.memory.read_u32_be(slider).unwrap();
    assert_eq!(loaded.memory.read_u8(slider_ptr + PPC_CONTROL_HILITE_OFFSET), Some(255));

    // A second root is refused, and the first still handed back.
    loaded.memory.write_u32_be(out, 0).unwrap();
    loaded.cpu.gpr[3] = window;
    loaded.cpu.gpr[4] = out;
    run_test_import(&mut loaded, PpcImportDispatcherTarget::AppearanceControl(Op::CreateRootControl));
    assert_eq!(loaded.cpu.gpr[3] as u16 as i16, -30587);
    assert_eq!(loaded.memory.read_u32_be(out), Some(root));

    // Active again, the slider is all indicator; the root pane is
    // invisible and not hit.
    loaded.cpu.gpr[3] = root;
    run_test_import(&mut loaded, PpcImportDispatcherTarget::AppearanceControl(Op::ActivateControl));
    assert_eq!(loaded.memory.read_u8(slider_ptr + PPC_CONTROL_HILITE_OFFSET), Some(0));
    loaded.cpu.gpr[3] = (20 << 16) | 60;
    loaded.cpu.gpr[4] = window;
    loaded.cpu.gpr[5] = scratch + 0x60;
    run_test_import(
        &mut loaded,
        PpcImportDispatcherTarget::AppearanceControl(Op::FindControlUnderMouse),
    );
    assert_eq!(loaded.cpu.gpr[3], slider);
    assert_eq!(loaded.memory.read_u16_be(scratch + 0x60), Some(129));
}

#[test]
fn slider_value_follows_the_pointer_along_the_track() {
    use super::appearance_controls::ppc_slider_value_at;
    // 112 wide less a 12-pixel thumb: 100 pixels of travel.
    let rect = (0, 0, 20, 112);
    assert_eq!(ppc_slider_value_at(rect, (10, 0), 0, 10), 0);
    assert_eq!(ppc_slider_value_at(rect, (10, 56), 0, 10), 5);
    assert_eq!(ppc_slider_value_at(rect, (10, 200), 0, 10), 10);
    assert_eq!(ppc_slider_value_at((0, 0, 112, 20), (106, 10), 0, 2), 2);
}

#[test]
fn appearance_window_types_draw_as_their_classic_equivalents() {
    for (appearance, classic) in [
        (1024, 4),
        (1025, 0),
        (1030, 12),
        (1031, 8),
        (1040, 2),
        (1041, 3),
        (1042, 1),
        (1043, 5),
        (1044, 1),
        (1045, 5),
        (1057, 4),
        (5, 5),
    ] {
        assert_eq!(ppc_classic_window_proc_id(appearance), classic, "{appearance}");
    }
}

#[test]
fn menu_hook_low_memory_accessors_round_trip() {
    let pef = synthetic_pef_with_import(b"LMSetMenuHook");
    let mut loaded = load_pef_application(&pef).unwrap();
    loaded.cpu.gpr[3] = 0x0100_ABC0;
    run_test_import(&mut loaded, PpcImportDispatcherTarget::LMSetMenuHook);
    loaded.cpu.gpr[3] = 0;
    run_test_import(&mut loaded, PpcImportDispatcherTarget::LMGetMenuHook);
    assert_eq!(loaded.cpu.gpr[3], 0x0100_ABC0);
}

#[test]
fn menu_items_are_drawn_whatever_clip_the_application_left_in_the_window_manager_port() {
    // The same menu held open twice: once with the port's clipRgn wide open
    // and once clipped to the menu bar strip, as Cythera leaves it. The
    // menu must come out the same.
    let draw = |strip_clip: bool| {
        let pef = synthetic_pef_with_import(b"MenuSelect");
        let mut loaded = load_pef_application(&pef).unwrap();
        install_test_menu(&mut loaded, PPC_DATA_BASE + 0x1000, 129, b"File", b"Quit;Save");
        let clip = loaded
            .memory
            .read_u32_be(PPC_MAIN_GWORLD + PPC_CGRAF_PORT_CLIP_RGN_OFFSET)
            .unwrap();
        if strip_clip {
            ppc_write_rgn_bbox(&mut loaded.memory, clip, 0, 0, 20, 640).unwrap();
        }
        loaded.cpu.gpr[3] = (10u32 << 16) | 12;
        loaded.set_input_snapshot(PpcInputSnapshot {
            mouse_button: true,
            mouse_v: 10,
            mouse_h: 12,
            ..PpcInputSnapshot::default()
        });
        let tick = loaded.current_tick().wrapping_add(1);
        loaded.set_tick_count(tick);
        let _ = loaded.run_with_hle_imports(256);
        assert_eq!(
            loaded
                .memory
                .read_u32_be(PPC_MAIN_GWORLD + PPC_CGRAF_PORT_CLIP_RGN_OFFSET),
            Some(clip),
            "the port's clipRgn is put back"
        );
        let front = ppc_front_buffer_for_gworld(&loaded.gworlds, PPC_MAIN_GWORLD).unwrap();
        (21..80)
            .flat_map(|y| (0..200).map(move |x| (x, y)))
            .map(|(x, y)| ppc_quickdraw_read_pixel(&mut loaded.memory, front, (x, y)))
            .collect::<Vec<_>>()
    };
    assert!(draw(false) == draw(true), "the clipped port lost the menu's items");
}

#[test]
fn pbh_open_rf_opens_a_new_files_empty_resource_fork() {
    // MoreFiles' FileCopy creates the copy and opens both its forks; a new
    // file's resource fork is empty but opens.
    let pef = synthetic_pef_with_import(b"PBHOpenRFSync");
    let mut loaded = load_pef_application(&pef).unwrap();
    loaded.push_test_vfs_file(PpcVfsFileRecord {
        path: "Saves/Bellerophon copy".to_string(),
        data: Vec::new().into(),
        creator: 0,
        file_type: 0,
        finder_flags: 0,
        dirty: false,
    });
    let pb = PPC_DATA_BASE + 0x1800;
    let name = pb + 0x80;
    loaded.memory.add_region(pb, vec![0; 0x100]);
    assert!(ppc_write_pstring_bytes(&mut loaded.memory, name, b"Bellerophon copy"));
    loaded.memory.write_u32_be(pb + 18, name).unwrap();
    loaded.memory.write_u8(pb + 27, 3).unwrap();
    loaded.cpu.gpr[3] = pb;
    run_test_import(
        &mut loaded,
        PpcImportDispatcherTarget::FileCompatibility(PpcFileCompatibilityOperation::PbHOpenRfSync),
    );
    assert_eq!(loaded.cpu.gpr[3], 0);
    assert_eq!(loaded.memory.read_u16_be(pb + 16), Some(0), "ioResult");
    assert_ne!(loaded.memory.read_u16_be(pb + 24), Some(0), "ioRefNum");
}

#[test]
fn hle_import_runner_maps_and_scales_points() {
    // Inside Macintosh Volume I (1985), pp. I-195--I-196.
    let pef = synthetic_pef_with_import(b"MapPt");
    let mut loaded = load_pef_application(&pef).unwrap();
    let pt_ptr = PPC_HEAP_BASE;
    let src_ptr = pt_ptr + 8;
    let dst_ptr = src_ptr + 8;
    loaded.memory.add_region(pt_ptr, vec![0; 24]);
    loaded.memory.write_u16_be(pt_ptr, 20).unwrap();
    loaded.memory.write_u16_be(pt_ptr + 2, 25).unwrap();
    ppc_write_rect(&mut loaded.memory, src_ptr, 10, 20, 110, 120).unwrap();
    ppc_write_rect(&mut loaded.memory, dst_ptr, -30, 200, 170, 600).unwrap();
    loaded.cpu.gpr[3] = pt_ptr;
    loaded.cpu.gpr[4] = src_ptr;
    loaded.cpu.gpr[5] = dst_ptr;
    run_test_import(&mut loaded, PpcImportDispatcherTarget::MapPt);
    assert_eq!(loaded.memory.read_u16_be(pt_ptr), Some((-10i16) as u16));
    assert_eq!(loaded.memory.read_u16_be(pt_ptr + 2), Some(220));

    // ScalePt: the point is a height (v) and a width (h), scaled by the
    // rectangles' ratio, 2 tall and 4 wide here.
    loaded.memory.write_u16_be(pt_ptr, 50).unwrap();
    loaded.memory.write_u16_be(pt_ptr + 2, 10).unwrap();
    loaded.cpu.gpr[3] = pt_ptr;
    loaded.cpu.gpr[4] = src_ptr;
    loaded.cpu.gpr[5] = dst_ptr;
    run_test_import(&mut loaded, PpcImportDispatcherTarget::ScalePt);
    assert_eq!(loaded.memory.read_u16_be(pt_ptr), Some(100));
    assert_eq!(loaded.memory.read_u16_be(pt_ptr + 2), Some(40));

    // And never below (1, 1).
    loaded.memory.write_u32_be(pt_ptr, 0).unwrap();
    loaded.cpu.gpr[3] = pt_ptr;
    loaded.cpu.gpr[4] = src_ptr;
    loaded.cpu.gpr[5] = dst_ptr;
    run_test_import(&mut loaded, PpcImportDispatcherTarget::ScalePt);
    assert_eq!(loaded.memory.read_u16_be(pt_ptr), Some(1));
    assert_eq!(loaded.memory.read_u16_be(pt_ptr + 2), Some(1));
}

#[test]
fn check_update_takes_the_next_update_event_and_leaves_the_rest() {
    // Macintosh Toolbox Essentials (1992), p. 4-116. Cythera calls it while
    // a window is dragged with Live Dragging on.
    let pef = synthetic_pef_with_import(b"CheckUpdate");
    let mut loaded = load_pef_application(&pef).unwrap();
    let event_ptr = PPC_DATA_BASE + 0x1000;
    loaded.memory.add_region(event_ptr, vec![0; 16]);
    loaded.set_event_queue([
        PpcQueuedEvent {
            what: 3,
            message: 0x0261,
            when: 5,
            where_v: 0,
            where_h: 0,
            modifiers: 0,
        },
        PpcQueuedEvent {
            what: 6,
            message: 0x0012_3456,
            when: 6,
            where_v: 0,
            where_h: 0,
            modifiers: 0,
        },
    ]);
    loaded.cpu.gpr[3] = event_ptr;
    run_test_import(&mut loaded, PpcImportDispatcherTarget::CheckUpdate);
    assert_eq!(loaded.cpu.gpr[3], 1);
    assert_eq!(loaded.memory.read_u16_be(event_ptr), Some(6));
    assert_eq!(loaded.memory.read_u32_be(event_ptr + 2), Some(0x0012_3456));
    assert_eq!(loaded.event_queue().len(), 1);
    assert_eq!(loaded.event_queue().get(0).unwrap().what, 3);

    loaded.cpu.gpr[3] = event_ptr;
    run_test_import(&mut loaded, PpcImportDispatcherTarget::CheckUpdate);
    assert_eq!(loaded.cpu.gpr[3], 0);
    assert_eq!(loaded.event_queue().len(), 1);
}

#[test]
fn dialog_delete_clears_the_selection_in_the_dialogs_edit_text() {
    let pef = synthetic_pef_with_import(b"GetNewDialog");
    let mut loaded = load_pef_application(&pef).unwrap();
    let mut dlog = vec![0; 22];
    dlog[4..6].copy_from_slice(&100i16.to_be_bytes());
    dlog[6..8].copy_from_slice(&260i16.to_be_bytes());
    dlog[10] = 1;
    dlog[18..20].copy_from_slice(&128i16.to_be_bytes());
    let mut ditl = vec![0; 22];
    ditl[6..8].copy_from_slice(&12i16.to_be_bytes());
    ditl[8..10].copy_from_slice(&20i16.to_be_bytes());
    ditl[10..12].copy_from_slice(&32i16.to_be_bytes());
    ditl[12..14].copy_from_slice(&220i16.to_be_bytes());
    ditl[14] = PPC_DIALOG_ITEM_EDIT_TEXT;
    ditl[15] = 5;
    ditl[16..21].copy_from_slice(b"Pilot");
    for (res_type, data) in [(*b"DLOG", dlog), (*b"DITL", ditl)] {
        let current_resource_refnum = *loaded.process_file_system.current_resource_file;
        loaded
            .process_file_system
            .push_vfs_resource(PpcVfsResourceRecord {
                ref_num: current_resource_refnum,
                path: String::new(),
                res_type: u32::from_be_bytes(res_type),
                res_id: 128,
                name: Vec::new(),
                data,
                raw_data: None,
                raw_attrs: None,
                attrs: 0,
                handle: 0,
            });
    }
    loaded.cpu.gpr[3] = 128;
    let probe = loaded.run_with_hle_imports(128);
    assert_eq!(probe.unsupported_import_index, None);
    let dialog = loaded.cpu.gpr[3];
    let te_handle = loaded
        .memory
        .read_u32_be(dialog + PPC_DIALOG_TEXT_HANDLE_OFFSET)
        .unwrap();
    assert_eq!(
        ppc_te_text_bytes(&mut loaded.memory, &test_handle_records!(loaded), te_handle),
        Some(b"Pilot".to_vec())
    );

    // GetNewDialog selects the whole of the first edit field; DialogDelete
    // (Macintosh Toolbox Essentials (1992), p. 6-134) takes it out.
    loaded.cpu.gpr[3] = dialog;
    run_test_import(
        &mut loaded,
        PpcImportDispatcherTarget::TEDelete { dialog: true },
    );
    assert_eq!(
        ppc_te_text_bytes(&mut loaded.memory, &test_handle_records!(loaded), te_handle),
        Some(Vec::new())
    );
}

#[test]
fn lclick_in_a_scroll_bar_scrolls_the_list() {
    // More Macintosh Toolbox (1993), p. 4-84. Cythera's character-creation
    // lists scroll only this way: its own list code handles just the thumb.
    let pef = synthetic_pef_with_import(b"LNew");
    let mut loaded = load_pef_application(&pef).unwrap();
    let scratch = PPC_DATA_BASE + 0x1000;
    let view_ptr = scratch;
    let bounds_ptr = scratch + 8;
    loaded.memory.add_region(scratch, vec![0; 32]);
    ppc_write_rect(&mut loaded.memory, view_ptr, 10, 20, 90, 120).unwrap();
    ppc_write_rect(&mut loaded.memory, bounds_ptr, 0, 0, 10, 1).unwrap();
    loaded.cpu.gpr[3] = view_ptr;
    loaded.cpu.gpr[4] = bounds_ptr;
    loaded.cpu.gpr[5] = (20u32 << 16) | 100;
    loaded.cpu.gpr[6] = 0;
    loaded.cpu.gpr[7] = PPC_MAIN_GWORLD;
    loaded.cpu.gpr[8] = 1;
    loaded.cpu.gpr[9] = 0;
    loaded.cpu.gpr[10] = 0;
    loaded
        .memory
        .write_u32_be(
            ppc_parameter_area_slot_addr(loaded.cpu.gpr[1], PPC_NATIVE_PARAMETER_GPR_COUNT)
                .unwrap(),
            1,
        )
        .unwrap();
    let probe = loaded.run_with_hle_imports(128);
    assert_eq!(probe.unsupported_import_index, None);
    let list = loaded.cpu.gpr[3];
    let list_ptr = loaded.memory.read_u32_be(list).unwrap();
    let visible_top = |loaded: &mut PpcLoadedApp| {
        loaded
            .memory
            .read_u16_be(list_ptr + PPC_LIST_VISIBLE_OFFSET)
            .unwrap() as i16
    };
    assert_eq!(visible_top(&mut loaded), 0);

    // The vertical bar runs from v 9 to 91 at h 120..136: the down arrow is
    // its last 16 pixels.
    let click = |loaded: &mut PpcLoadedApp, v: u32, h: u32| {
        loaded.cpu.gpr[3] = (v << 16) | h;
        loaded.cpu.gpr[4] = 0;
        loaded.cpu.gpr[5] = list;
        run_test_import(loaded, PpcImportDispatcherTarget::LClick);
    };
    click(&mut loaded, 85, 128);
    assert_eq!(visible_top(&mut loaded), 1);
    let bar = loaded
        .memory
        .read_u32_be(list_ptr + PPC_LIST_VSCROLL_OFFSET)
        .unwrap();
    let bar_ptr = ppc_control_ptr(&mut loaded.memory, bar).unwrap();
    assert_eq!(
        loaded.memory.read_u16_be(bar_ptr + PPC_CONTROL_VALUE_OFFSET),
        Some(1),
        "the bar's value follows the list"
    );
    // Below the thumb: a page, four rows shown less one.
    click(&mut loaded, 60, 128);
    assert_eq!(visible_top(&mut loaded), 4);
    click(&mut loaded, 12, 128);
    assert_eq!(visible_top(&mut loaded), 3);
    // A click in the list itself still selects and does not scroll.
    click(&mut loaded, 15, 30);
    assert_eq!(visible_top(&mut loaded), 3);
    assert!(loaded
        .list_manager
        .get_record(list)
        .unwrap()
        .selected
        .contains(&(3, 0)));
}

#[test]
fn find_control_asks_an_application_cdef_which_part_is_hit() {
    // Macintosh Toolbox Essentials (1992), pp. 5-110 and 5-112: FindControl
    // sends testCntl, and the CDEF answers with the part under the point or
    // 0. Cythera's scroll bars are drawn and hit-tested by its own CDEF.
    let pef = synthetic_pef_with_import(b"FindControl");
    let mut loaded = load_pef_application(&pef).unwrap();
    let scratch = PPC_DATA_BASE + 0x1000;
    loaded.memory.add_region(scratch, vec![0; 0x100]);
    let window = create_test_cwindow(&mut loaded, scratch, (40, 40, 200, 300), 0, true, u32::MAX);

    // 'CDEF' 128 as Cythera ships it: a 68K JMP to a PowerPC procedure,
    // here `li r3,21; blr`.
    let code = PPC_CODE_BASE + 0x1000;
    loaded.memory.add_region(
        code,
        [0x3860_0015u32, 0x4e80_0020]
            .into_iter()
            .flat_map(u32::to_be_bytes)
            .collect(),
    );
    let tvector = scratch + 0x80;
    loaded.memory.write_u32_be(tvector, code).unwrap();
    loaded.memory.write_u32_be(tvector + 4, loaded.cpu.gpr[2]).unwrap();
    let stub = scratch + 0x90;
    loaded.memory.write_u16_be(stub, 0x4ef9).unwrap();
    loaded.memory.write_u32_be(stub + 2, tvector).unwrap();
    let cdef_handle = scratch + 0xa0;
    loaded.memory.write_u32_be(cdef_handle, stub).unwrap();
    let current_resource_refnum = *loaded.process_file_system.current_resource_file;
    loaded
        .process_file_system
        .push_vfs_resource(PpcVfsResourceRecord {
            ref_num: current_resource_refnum,
            path: String::new(),
            res_type: u32::from_be_bytes(*b"CDEF"),
            res_id: 128,
            name: Vec::new(),
            data: vec![0x4e, 0xf9, 0, 0, 0, 0],
            raw_data: None,
            raw_attrs: None,
            attrs: 0,
            handle: cdef_handle,
        });
    let run = |loaded: &mut PpcLoadedApp, target: PpcImportDispatcherTarget| {
        loaded.cpu.pc = loaded.entry_pc;
        loaded.cpu.lr = PPC_HALT_PC;
        loaded.imports[0].dispatcher_target = target;
        let probe = loaded.run_with_hle_imports(512);
        assert_eq!(probe.unsupported_import_index, None);
    };

    ppc_write_rect(&mut loaded.memory, scratch + 0x50, 10, 10, 60, 26).unwrap();
    loaded.cpu.gpr[3] = window;
    loaded.cpu.gpr[4] = scratch + 0x50;
    loaded.cpu.gpr[5] = 0;
    loaded.cpu.gpr[6] = 1;
    loaded.cpu.gpr[7] = 0;
    loaded.cpu.gpr[8] = 0;
    loaded.cpu.gpr[9] = 10;
    loaded.cpu.gpr[10] = 128 << 4;
    run(
        &mut loaded,
        PpcImportDispatcherTarget::LegacyControl(PpcLegacyControlOperation::NewControl),
    );
    let control = loaded.cpu.gpr[3];
    assert_ne!(control, 0);

    let out = scratch + 0x40;
    loaded.cpu.gpr[3] = (50 << 16) | 15;
    loaded.cpu.gpr[4] = window;
    loaded.cpu.gpr[5] = out;
    run(
        &mut loaded,
        PpcImportDispatcherTarget::LegacyControl(PpcLegacyControlOperation::FindControl),
    );
    assert_eq!(loaded.cpu.gpr[3], 21, "the CDEF's inDownButton");
    assert_eq!(loaded.memory.read_u32_be(out), Some(control));

    // A CDEF that finds no part leaves theControl NIL.
    loaded.memory.write_u32_be(code, 0x3860_0000).unwrap();
    loaded.cpu.gpr[3] = (50 << 16) | 15;
    loaded.cpu.gpr[4] = window;
    loaded.cpu.gpr[5] = out;
    run(
        &mut loaded,
        PpcImportDispatcherTarget::LegacyControl(PpcLegacyControlOperation::FindControl),
    );
    assert_eq!(loaded.cpu.gpr[3], 0);
    assert_eq!(loaded.memory.read_u32_be(out), Some(0));
}

#[test]
fn draw_dialog_draws_controls_that_are_not_items() {
    // Macintosh Toolbox Essentials (1992), p. 6-142: DrawDialog calls
    // DrawControls, which draws every control in the window's list.
    let pef = synthetic_pef_with_import(b"GetNewDialog");
    let mut loaded = load_pef_application(&pef).unwrap();
    let mut dlog = vec![0; 22];
    dlog[0..2].copy_from_slice(&60i16.to_be_bytes());
    dlog[2..4].copy_from_slice(&60i16.to_be_bytes());
    dlog[4..6].copy_from_slice(&220i16.to_be_bytes());
    dlog[6..8].copy_from_slice(&320i16.to_be_bytes());
    dlog[10] = 1;
    dlog[18..20].copy_from_slice(&128i16.to_be_bytes());
    let mut ditl = vec![0; 22];
    ditl[6..8].copy_from_slice(&12i16.to_be_bytes());
    ditl[8..10].copy_from_slice(&20i16.to_be_bytes());
    ditl[10..12].copy_from_slice(&32i16.to_be_bytes());
    ditl[12..14].copy_from_slice(&120i16.to_be_bytes());
    ditl[14] = PPC_DIALOG_ITEM_STATIC_TEXT;
    ditl[15] = 5;
    ditl[16..21].copy_from_slice(b"Pilot");
    for (res_type, data) in [(*b"DLOG", dlog), (*b"DITL", ditl)] {
        let current_resource_refnum = *loaded.process_file_system.current_resource_file;
        loaded
            .process_file_system
            .push_vfs_resource(PpcVfsResourceRecord {
                ref_num: current_resource_refnum,
                path: String::new(),
                res_type: u32::from_be_bytes(res_type),
                res_id: 128,
                name: Vec::new(),
                data,
                raw_data: None,
                raw_attrs: None,
                attrs: 0,
                handle: 0,
            });
    }
    loaded.cpu.gpr[3] = 128;
    let probe = loaded.run_with_hle_imports(128);
    assert_eq!(probe.unsupported_import_index, None);
    let dialog = loaded.cpu.gpr[3];

    // A scroll bar of the application's own, not in the item list.
    let scratch = PPC_DATA_BASE + 0x1000;
    loaded.memory.add_region(scratch, vec![0; 16]);
    ppc_write_rect(&mut loaded.memory, scratch, 40, 200, 140, 216).unwrap();
    loaded.cpu.gpr[3] = dialog;
    loaded.cpu.gpr[4] = scratch;
    loaded.cpu.gpr[5] = 0;
    loaded.cpu.gpr[6] = 1;
    loaded.cpu.gpr[7] = 0;
    loaded.cpu.gpr[8] = 0;
    loaded.cpu.gpr[9] = 10;
    loaded.cpu.gpr[10] = 16;
    run_test_import(
        &mut loaded,
        PpcImportDispatcherTarget::LegacyControl(PpcLegacyControlOperation::NewControl),
    );
    assert_ne!(loaded.cpu.gpr[3], 0);

    let bounds = ppc_dialog_global_bounds(&mut loaded.memory, &loaded.gworlds, dialog).unwrap();
    let front = ppc_front_buffer_for_gworld(&loaded.gworlds, PPC_MAIN_GWORLD).unwrap();
    // Down the middle of the bar, clear of its arrows.
    // Front-buffer points are (h, v).
    let points: Vec<(i32, i32)> = (70..110)
        .map(|v| (i32::from(bounds.1) + 208, i32::from(bounds.0) + v))
        .collect();
    for &point in &points {
        assert!(ppc_quickdraw_write_raw_pixel(&mut loaded.memory, front, point, 0x7b));
    }
    loaded.cpu.gpr[3] = dialog;
    run_test_import(&mut loaded, PpcImportDispatcherTarget::DrawDialog);
    assert!(
        points
            .iter()
            .any(|&point| ppc_quickdraw_read_pixel(&mut loaded.memory, front, point) != Some(0x7b)),
        "DrawDialog left the application's scroll bar undrawn"
    );
}

#[test]
fn te_text_box_erases_with_the_ports_background_pixel_pattern() {
    // TETextBox erases its box as EraseRect does, with the port's bkPixPat
    // when there is one (Imaging With QuickDraw (1994), 4-73). Cythera's
    // character-creation text is drawn on its parchment this way.
    let pef = synthetic_pef_with_import(b"TETextBox");
    let mut loaded = load_pef_application(&pef).unwrap();
    let scratch = PPC_HEAP_BASE + 0x14000;
    let rect_ptr = scratch + 0x80;
    let pixpat_handle = scratch + 0x1c0;
    let pixpat_ptr = scratch + 0x1c4;
    loaded.memory.add_region(scratch, vec![0; 0x300]);
    ppc_write_pixmap(&mut loaded.memory, PPC_MAIN_PIXMAP, scratch, 8, 0, 0, 6, 6, 8).unwrap();
    for offset in 0..64u32 {
        loaded.memory.write_u8(scratch + offset, 100).unwrap();
    }
    // A full-colour (type 1) 8x8, 8-bit pattern, its PixMap and data given
    // as offsets into the record.
    loaded.memory.write_u32_be(pixpat_handle, pixpat_ptr).unwrap();
    loaded.memory.write_u16_be(pixpat_ptr, 1).unwrap();
    loaded.memory.write_u32_be(pixpat_ptr + 2, 0x20).unwrap();
    loaded.memory.write_u32_be(pixpat_ptr + 6, 0x60).unwrap();
    let pat_map = pixpat_ptr + 0x20;
    loaded.memory.write_u16_be(pat_map + 4, 8).unwrap();
    ppc_write_rect(&mut loaded.memory, pat_map + 6, 0, 0, 8, 8).unwrap();
    loaded.memory.write_u16_be(pat_map + 32, 8).unwrap();
    for offset in 0..64u32 {
        loaded
            .memory
            .write_u8(pixpat_ptr + 0x60 + offset, if offset == 0 { 0x55 } else { 0x33 })
            .unwrap();
    }
    loaded
        .memory
        .write_u32_be(PPC_MAIN_GWORLD + PPC_CGRAF_PORT_BK_PIXPAT_OFFSET, pixpat_handle)
        .unwrap();
    ppc_write_rect(&mut loaded.memory, rect_ptr, 0, 0, 4, 4).unwrap();
    loaded.cpu.gpr[3] = scratch + 0x100;
    loaded.cpu.gpr[4] = 0;
    loaded.cpu.gpr[5] = rect_ptr;
    loaded.cpu.gpr[6] = 0;
    run_test_import(&mut loaded, PpcImportDispatcherTarget::TETextBox);
    assert_eq!(loaded.memory.read_u8(scratch), Some(0x55));
    assert_eq!(loaded.memory.read_u8(scratch + 1), Some(0x33));
    // Outside the box nothing is touched.
    assert_eq!(loaded.memory.read_u8(scratch + 4), Some(100));
}

#[test]
fn char_extra_narrows_every_character_but_the_space() {
    // Inside Macintosh Volume V (1986), p. V-77. Cythera narrows a name that
    // would not fit under a portrait this way.
    let pef = synthetic_pef_with_import(b"DrawText");
    let mut loaded = load_pef_application(&pef).unwrap();
    let text = PPC_DATA_BASE + 0x1000;
    loaded.memory.add_region(text, b"ab c".to_vec());
    let pen_h = |loaded: &mut PpcLoadedApp| {
        loaded
            .memory
            .read_u16_be(PPC_MAIN_GWORLD + PPC_CGRAF_PORT_PN_LOC_OFFSET + 2)
            .unwrap() as i16
    };
    let draw = |loaded: &mut PpcLoadedApp| {
        loaded.cpu.gpr[3] = 20;
        loaded.cpu.gpr[4] = 10;
        run_test_import(loaded, PpcImportDispatcherTarget::MoveTo);
        loaded.cpu.gpr[3] = text;
        loaded.cpu.gpr[4] = 0;
        loaded.cpu.gpr[5] = 4;
        run_test_import(loaded, PpcImportDispatcherTarget::DrawText);
    };
    draw(&mut loaded);
    let plain = pen_h(&mut loaded) - 20;
    assert!(plain > 3);

    loaded.cpu.gpr[3] = (-1i32 << 16) as u32;
    run_test_import(&mut loaded, PpcImportDispatcherTarget::CharExtra);
    draw(&mut loaded);
    assert_eq!(pen_h(&mut loaded) - 20, plain - 3);
}

#[test]
fn appearance_static_text_wraps_in_its_style() {
    // Cythera's Preferences labels: static text (procID 288) with a
    // ControlFontStyleRec of flags 0x47, font -2 (small system), centred
    // (Universal Interfaces 3.4.2 Controls.h). "Better Performance" wraps
    // onto a second line in its narrow rectangle, as on Mac OS 8.5.
    use super::appearance_controls::PpcAppearanceControlOperation as Op;
    let pef = synthetic_pef_with_import(b"NewControl");
    let mut loaded = load_pef_application(&pef).unwrap();
    let scratch = PPC_DATA_BASE + 0x1000;
    loaded.memory.add_region(scratch, vec![0; 0x100]);
    let window = create_test_cwindow(&mut loaded, scratch, (40, 40, 200, 300), 0, true, u32::MAX);
    let title = scratch + 0x60;
    loaded.memory.write_u8(title, 18).unwrap();
    loaded.memory.write_bytes(title + 1, b"Better Performance").unwrap();
    ppc_write_rect(&mut loaded.memory, scratch + 0x50, 20, 20, 50, 80).unwrap();
    loaded.cpu.gpr[3] = window;
    loaded.cpu.gpr[4] = scratch + 0x50;
    loaded.cpu.gpr[5] = title;
    loaded.cpu.gpr[6] = 1;
    loaded.cpu.gpr[7] = 0;
    loaded.cpu.gpr[8] = 0;
    loaded.cpu.gpr[9] = 0;
    loaded.cpu.gpr[10] = 288;
    run_test_import(
        &mut loaded,
        PpcImportDispatcherTarget::LegacyControl(PpcLegacyControlOperation::NewControl),
    );
    let control = loaded.cpu.gpr[3];
    let style = scratch + 0x90;
    loaded.memory.write_bytes(style, &[0x00, 0x47, 0xff, 0xfe, 0, 0, 0, 0, 0, 0, 0, 1]).unwrap();
    loaded.cpu.gpr[3] = control;
    loaded.cpu.gpr[4] = 0;
    loaded.cpu.gpr[5] = u32::from_be_bytes(*b"font");
    loaded.cpu.gpr[6] = 24;
    loaded.cpu.gpr[7] = style;
    run_test_import(&mut loaded, PpcImportDispatcherTarget::AppearanceControl(Op::SetControlData));
    assert_eq!(loaded.cpu.gpr[3], 0);

    let front = ppc_front_buffer_for_gworld(&loaded.gworlds, PPC_MAIN_GWORLD).unwrap();
    let origin = (40 + 20, 40 + 20); // the window's content origin plus the rect
    let white = ppc_quickdraw_read_pixel(&mut loaded.memory, front, (0, 599));
    let inked_row = |loaded: &mut PpcLoadedApp, row: i32| {
        (origin.1..origin.1 + 60).any(|x| {
            ppc_quickdraw_read_pixel(&mut loaded.memory, front, (x, origin.0 + row)) != white
        })
    };
    loaded.cpu.gpr[3] = control;
    run_test_import(
        &mut loaded,
        PpcImportDispatcherTarget::LegacyControl(PpcLegacyControlOperation::DrawOneControl),
    );
    // Geneva 9 lines are 11 pixels apart; the second line's letters sit in
    // rows 13..20 of the rectangle.
    assert!((13..21).any(|row| inked_row(&mut loaded, row)), "no second line");
}

#[test]
fn find_window_asks_an_application_wdef_which_part_is_hit() {
    // Macintosh Toolbox Essentials (1992), pp. 4-122 and 4-131: FindWindow
    // sends wHit and returns the answer plus 2. Cythera's To Do and Journal
    // drawers answer wInGrow on their title bar, and open by the resize.
    let pef = synthetic_pef_with_import(b"FindWindow");
    let mut loaded = load_pef_application(&pef).unwrap();
    let scratch = PPC_DATA_BASE + 0x1000;
    loaded.memory.add_region(scratch, vec![0; 0x100]);

    // 'WDEF' 128 as Cythera ships its own: a 68K JMP to a PowerPC procedure,
    // here `li r3,3; blr` (wInGrow).
    let code = PPC_CODE_BASE + 0x1000;
    loaded.memory.add_region(
        code,
        [0x3860_0003u32, 0x4e80_0020]
            .into_iter()
            .flat_map(u32::to_be_bytes)
            .collect(),
    );
    let tvector = scratch + 0x80;
    loaded.memory.write_u32_be(tvector, code).unwrap();
    loaded.memory.write_u32_be(tvector + 4, loaded.cpu.gpr[2]).unwrap();
    let stub = scratch + 0x90;
    loaded.memory.write_u16_be(stub, 0x4ef9).unwrap();
    loaded.memory.write_u32_be(stub + 2, tvector).unwrap();
    let wdef_handle = scratch + 0xa0;
    loaded.memory.write_u32_be(wdef_handle, stub).unwrap();
    let current_resource_refnum = *loaded.process_file_system.current_resource_file;
    loaded
        .process_file_system
        .push_vfs_resource(PpcVfsResourceRecord {
            ref_num: current_resource_refnum,
            path: String::new(),
            res_type: u32::from_be_bytes(*b"WDEF"),
            res_id: 128,
            name: Vec::new(),
            data: vec![0x4e, 0xf9, 0, 0, 0, 0],
            raw_data: None,
            raw_attrs: None,
            attrs: 0,
            handle: wdef_handle,
        });
    let run = |loaded: &mut PpcLoadedApp, target: PpcImportDispatcherTarget| {
        loaded.cpu.pc = loaded.entry_pc;
        loaded.cpu.lr = PPC_HALT_PC;
        loaded.imports[0].dispatcher_target = target;
        let probe = loaded.run_with_hle_imports(512);
        assert_eq!(probe.unsupported_import_index, None);
    };

    ppc_write_rect(&mut loaded.memory, scratch, 100, 100, 200, 300).unwrap();
    loaded.cpu.gpr[3] = 0;
    loaded.cpu.gpr[4] = scratch;
    loaded.cpu.gpr[5] = 0;
    loaded.cpu.gpr[6] = 1;
    loaded.cpu.gpr[7] = 128 << 4;
    loaded.cpu.gpr[8] = u32::MAX;
    loaded.cpu.gpr[9] = 1;
    loaded.cpu.gpr[10] = 0;
    run(&mut loaded, PpcImportDispatcherTarget::NewCWindow);
    let window = loaded.cpu.gpr[3];
    assert_ne!(window, 0);

    let out = scratch + 0x40;
    let find = |loaded: &mut PpcLoadedApp| {
        loaded.cpu.gpr[3] = (150 << 16) | 200;
        loaded.cpu.gpr[4] = out;
        run(loaded, PpcImportDispatcherTarget::FindWindow);
    };
    find(&mut loaded);
    assert_eq!(loaded.cpu.gpr[3], 5, "wInGrow is inGrow");
    assert_eq!(loaded.memory.read_u32_be(out), Some(window));

    // wNoHit: not in this window, inDesk.
    loaded.memory.write_u32_be(code, 0x3860_0000).unwrap();
    find(&mut loaded);
    assert_eq!(loaded.cpu.gpr[3], 0);
    assert_eq!(loaded.memory.read_u32_be(out), Some(0));
}

#[test]
fn lupdate_redraws_the_scroll_bars_of_a_list_with_its_own_ldef() {
    // More Macintosh Toolbox (1993), p. 4-86: LUpdate redraws the cells and
    // the scroll bars that meet the region. Cythera's To Do and Journal
    // drawers are updated with LUpdate alone.
    let pef = synthetic_pef_with_import(b"LNew");
    let mut loaded = load_pef_application(&pef).unwrap();
    let scratch = PPC_DATA_BASE + 0x1000;
    loaded.memory.add_region(scratch, vec![0; 64]);
    let window = create_test_cwindow(&mut loaded, scratch + 32, (60, 60, 260, 360), 0, true, u32::MAX);
    ppc_write_rect(&mut loaded.memory, scratch, 10, 20, 90, 120).unwrap();
    ppc_write_rect(&mut loaded.memory, scratch + 8, 0, 0, 10, 1).unwrap();
    loaded.cpu.gpr[3] = scratch;
    loaded.cpu.gpr[4] = scratch + 8;
    loaded.cpu.gpr[5] = (20u32 << 16) | 100;
    loaded.cpu.gpr[6] = 0;
    loaded.cpu.gpr[7] = window;
    loaded.cpu.gpr[8] = 1;
    loaded.cpu.gpr[9] = 0;
    loaded.cpu.gpr[10] = 0;
    loaded
        .memory
        .write_u32_be(
            ppc_parameter_area_slot_addr(loaded.cpu.gpr[1], PPC_NATIVE_PARAMETER_GPR_COUNT)
                .unwrap(),
            1,
        )
        .unwrap();
    run_test_import(&mut loaded, PpcImportDispatcherTarget::LNew);
    let list = loaded.cpu.gpr[3];
    assert_ne!(list, 0);
    super::dispatch_defproc::ppc_register_list_proc(list, true);

    // The bar runs down the right of rView, h 120..136; mark its middle.
    let front = ppc_front_buffer_for_gworld(&loaded.gworlds, PPC_MAIN_GWORLD).unwrap();
    let points: Vec<(i32, i32)> = (40..60).map(|v| (60 + 128, 60 + v)).collect();
    for &point in &points {
        assert!(ppc_quickdraw_write_raw_pixel(&mut loaded.memory, front, point, 0x7b));
    }
    run_test_import(&mut loaded, PpcImportDispatcherTarget::NewRgn);
    let region = loaded.cpu.gpr[3];
    ppc_write_rect(&mut loaded.memory, scratch + 16, 0, 0, 200, 300).unwrap();
    loaded.cpu.gpr[3] = region;
    loaded.cpu.gpr[4] = scratch + 16;
    run_test_import(&mut loaded, PpcImportDispatcherTarget::RectRgn);
    loaded.cpu.gpr[3] = region;
    loaded.cpu.gpr[4] = list;
    run_test_import(&mut loaded, PpcImportDispatcherTarget::LUpdate);
    assert!(
        points
            .iter()
            .any(|&point| ppc_quickdraw_read_pixel(&mut loaded.memory, front, point) != Some(0x7b)),
        "LUpdate left the scroll bar undrawn"
    );
}

#[test]
fn track_go_away_asks_an_application_wdef_about_the_release() {
    // Cythera's drawer windows carry their close tag left of the content,
    // where no standard close box is; TrackGoAway asks the WDEF (wHit) where
    // the button was released and answers true for wInGoAway (4).
    let pef = synthetic_pef_with_import(b"TrackGoAway");
    let mut loaded = load_pef_application(&pef).unwrap();
    let scratch = PPC_DATA_BASE + 0x1000;
    loaded.memory.add_region(scratch, vec![0; 0x100]);
    let code = PPC_CODE_BASE + 0x1000;
    loaded.memory.add_region(
        code,
        [0x3860_0004u32, 0x4e80_0020]
            .into_iter()
            .flat_map(u32::to_be_bytes)
            .collect(),
    );
    let tvector = scratch + 0x80;
    loaded.memory.write_u32_be(tvector, code).unwrap();
    loaded.memory.write_u32_be(tvector + 4, loaded.cpu.gpr[2]).unwrap();
    let stub = scratch + 0x90;
    loaded.memory.write_u16_be(stub, 0x4ef9).unwrap();
    loaded.memory.write_u32_be(stub + 2, tvector).unwrap();
    let wdef_handle = scratch + 0xa0;
    loaded.memory.write_u32_be(wdef_handle, stub).unwrap();
    let current_resource_refnum = *loaded.process_file_system.current_resource_file;
    loaded
        .process_file_system
        .push_vfs_resource(PpcVfsResourceRecord {
            ref_num: current_resource_refnum,
            path: String::new(),
            res_type: u32::from_be_bytes(*b"WDEF"),
            res_id: 128,
            name: Vec::new(),
            data: vec![0x4e, 0xf9, 0, 0, 0, 0],
            raw_data: None,
            raw_attrs: None,
            attrs: 0,
            handle: wdef_handle,
        });
    let run = |loaded: &mut PpcLoadedApp, target: PpcImportDispatcherTarget| {
        loaded.cpu.pc = loaded.entry_pc;
        loaded.cpu.lr = PPC_HALT_PC;
        loaded.imports[0].dispatcher_target = target;
        let probe = loaded.run_with_hle_imports(512);
        assert_eq!(probe.unsupported_import_index, None);
    };
    ppc_write_rect(&mut loaded.memory, scratch, 100, 100, 200, 300).unwrap();
    loaded.cpu.gpr[3] = 0;
    loaded.cpu.gpr[4] = scratch;
    loaded.cpu.gpr[5] = 0;
    loaded.cpu.gpr[6] = 1;
    loaded.cpu.gpr[7] = 128 << 4;
    loaded.cpu.gpr[8] = u32::MAX;
    loaded.cpu.gpr[9] = 1;
    loaded.cpu.gpr[10] = 0;
    run(&mut loaded, PpcImportDispatcherTarget::NewCWindow);
    let window = loaded.cpu.gpr[3];

    let track = |loaded: &mut PpcLoadedApp| {
        loaded.cpu.gpr[3] = window;
        loaded.cpu.gpr[4] = (120 << 16) | 90;
        run(
            loaded,
            PpcImportDispatcherTarget::LegacyWindow(PpcLegacyWindowOperation::TrackGoAway),
        );
        loaded.cpu.gpr[3]
    };
    assert_eq!(track(&mut loaded), 1, "released in wInGoAway");
    loaded.memory.write_u32_be(code, 0x3860_0001).unwrap();
    assert_eq!(track(&mut loaded), 0, "released in wInContent");
}

#[test]
fn show_hide_false_repaints_what_the_window_uncovered() {
    // Hiding a window uncovers what was behind it, which is redrawn as when
    // the window is disposed of. Cythera closes its drawers with
    // ShowHide(false), and the backdrop under one kept the drawer's pixels.
    let pef = synthetic_pef_with_import(b"ShowHide");
    let mut loaded = load_pef_application(&pef).unwrap();
    let scratch = PPC_DATA_BASE + 0x1000;
    loaded.memory.add_region(scratch, vec![0; 32]);
    let window = create_test_cwindow(&mut loaded, scratch, (100, 100, 200, 300), 0, true, u32::MAX);
    let front = ppc_front_buffer_for_gworld(&loaded.gworlds, PPC_MAIN_GWORLD).unwrap();
    let point = (150, 150);
    assert!(ppc_quickdraw_write_raw_pixel(&mut loaded.memory, front, point, 0x7b));
    loaded.cpu.gpr[3] = window;
    loaded.cpu.gpr[4] = 0;
    run_test_import(&mut loaded, PpcImportDispatcherTarget::ShowHide);
    assert_ne!(
        ppc_quickdraw_read_pixel(&mut loaded.memory, front, point),
        Some(0x7b),
        "the hidden window's pixels were left on the screen"
    );
}
