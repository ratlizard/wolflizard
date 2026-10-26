//! Cythera's map lighting, run natively.
//!
//! `TViewer::ApplyLight` shades the map window in blocks of 4 by 4 pixels.
//! For each block a one-byte light level chooses what it does: 0 paints the
//! block with colour index 255, 1 to 31 maps its pixels through one shade
//! table (odd levels) or two tables laid out as a checkerboard (even levels),
//! and 32 and over leave it alone. Each block costs about 115 PowerPC
//! instructions, in `SubLightSolid` or `SubLightDither`, and the work grows
//! with the map window's area, so it was most of what the program executed in
//! a dark room and what a Board could not keep up with in a large window.
//!
//! The loader turns the routine's first instruction into a branch to a
//! reserved import slot, and this module does the same work on the guest's
//! memory. It only does so when the three routines match Cythera 1.0.4's
//! bytes, so any other build of the program keeps its own code. Setting
//! `SYSTEMLESS_NO_NATIVE_LIGHT` leaves the program as it is, which is how a
//! run is compared with the original code.

use super::*;

// Offsets in the program's PowerPC code section. The routines begin at the
// `stmw` that saves their registers, four bytes before the address the
// traceback symbols give them.
const SUB_LIGHT_START: usize = 0x647fc;
const SUB_LIGHT_END: usize = 0x6490c;
const APPLY_LIGHT_START: usize = 0x64eac;
const APPLY_LIGHT_END: usize = 0x65008;
const SUB_LIGHT_FNV: u64 = 0x9f89_c5cf_52e7_cf88;
const APPLY_LIGHT_FNV: u64 = 0x9f5a_a4a3_f943_d21b;
const APPLY_LIGHT_FIRST_WORD: u32 = 0xbf21_ffe4; // stmw r25,-28(r1)

// Fields of the viewer object, read from the routine's loads.
const VIEWER_BLOCKS_FIELD: u32 = 0; // halfword: blocks are (value + 1) * 8 on a side
const VIEWER_TABLE_POINTERS: u32 = 56; // sixteen pointers to 256-byte shade tables
const VIEWER_DESTINATION: u32 = 176;
const VIEWER_STRIDE: u32 = 180;
const VIEWER_LIGHT_LEVELS: u32 = 0x80bc;

fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325u64, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

/// Branches the first instruction of `ApplyLight` to the reserved import
/// slot, when `code` is Cythera 1.0.4's code section. Returns whether it did.
pub(super) fn install_native_apply_light(code: &mut [u8], code_base: u32) -> bool {
    if std::env::var_os("SYSTEMLESS_NO_NATIVE_LIGHT").is_some() {
        return false;
    }
    let (Some(sub_light), Some(apply_light)) = (
        code.get(SUB_LIGHT_START..SUB_LIGHT_END),
        code.get(APPLY_LIGHT_START..APPLY_LIGHT_END),
    ) else {
        return false;
    };
    if fnv1a(sub_light) != SUB_LIGHT_FNV
        || fnv1a(apply_light) != APPLY_LIGHT_FNV
        || u32::from_be_bytes([apply_light[0], apply_light[1], apply_light[2], apply_light[3]])
            != APPLY_LIGHT_FIRST_WORD
    {
        return false;
    }
    let entry = i64::from(code_base) + APPLY_LIGHT_START as i64;
    let delta = i64::from(PPC_CYTHERA_APPLY_LIGHT_PC) - entry;
    // `b` reaches 32 MB either way.
    if delta & 3 != 0 || !(-0x0200_0000..0x0200_0000).contains(&delta) {
        return false;
    }
    let branch = 0x4800_0000u32 | (delta as u32 & 0x03ff_fffc);
    code[APPLY_LIGHT_START..APPLY_LIGHT_START + 4].copy_from_slice(&branch.to_be_bytes());
    true
}

/// `TViewer::ApplyLight(this = r3)`, done on the guest's memory.
pub(super) fn apply_light(cpu: &PpcCpu, memory: &mut PpcSectionMem) {
    let viewer = cpu.gpr[3];
    let Some(size) = memory.read_u16_be(viewer.wrapping_add(VIEWER_BLOCKS_FIELD)) else {
        return;
    };
    let blocks = (i32::from(size as i16) + 1) * 8;
    if blocks <= 0 {
        return;
    }
    let blocks = blocks as usize;
    let (Some(destination), Some(stride)) = (
        memory.read_u32_be(viewer.wrapping_add(VIEWER_DESTINATION)),
        memory.read_u32_be(viewer.wrapping_add(VIEWER_STRIDE)),
    ) else {
        return;
    };
    let mut levels = vec![0u8; blocks * blocks];
    if memory
        .read_bytes_into(viewer.wrapping_add(VIEWER_LIGHT_LEVELS), &mut levels)
        .is_none()
    {
        return;
    }
    // Read each shade table the light levels use, once. A table that cannot
    // be read leaves its blocks as they are.
    let mut used = [false; 16];
    for level in 1..32usize {
        if levels.contains(&(level as u8)) {
            used[15 - level / 2] = true;
            if level & 1 == 0 {
                used[16 - level / 2] = true;
            }
        }
    }
    let mut tables = [[0u8; 256]; 16];
    let mut loaded = [false; 16];
    for index in (0..16).filter(|index| used[*index]) {
        if let Some(pointer) =
            memory.read_u32_be(viewer.wrapping_add(VIEWER_TABLE_POINTERS + 4 * index as u32))
        {
            loaded[index] = memory.read_bytes_into(pointer, &mut tables[index]).is_some();
        }
    }
    let mut line = vec![0u8; blocks * 4];
    for block_row in 0..blocks {
        let row_levels = &levels[block_row * blocks..(block_row + 1) * blocks];
        if row_levels.iter().all(|level| *level >= 32) {
            continue;
        }
        for pixel_row in 0..4u32 {
            let line_address = destination.wrapping_add(
                stride.wrapping_mul(block_row as u32 * 4 + pixel_row),
            );
            if memory.read_bytes_into(line_address, &mut line).is_none() {
                return;
            }
            let mut changed = false;
            for (column, level) in row_levels.iter().enumerate() {
                let level = usize::from(*level);
                let pixels = &mut line[column * 4..column * 4 + 4];
                if level == 0 {
                    pixels.fill(0xff);
                    changed = true;
                } else if level >= 32 {
                    continue;
                } else if level & 1 == 1 {
                    if !loaded[15 - level / 2] {
                        continue;
                    }
                    let table = &tables[15 - level / 2];
                    for pixel in pixels.iter_mut() {
                        *pixel = table[usize::from(*pixel)];
                    }
                    changed = true;
                } else {
                    if !loaded[15 - level / 2] || !loaded[16 - level / 2] {
                        continue;
                    }
                    let (first, second) = (&tables[15 - level / 2], &tables[16 - level / 2]);
                    // The routine swaps the two tables on every row, so the
                    // four pixels alternate between them like a checkerboard.
                    let (even, odd) = if pixel_row & 1 == 0 {
                        (second, first)
                    } else {
                        (first, second)
                    };
                    pixels[0] = even[usize::from(pixels[0])];
                    pixels[1] = odd[usize::from(pixels[1])];
                    pixels[2] = even[usize::from(pixels[2])];
                    pixels[3] = odd[usize::from(pixels[3])];
                    changed = true;
                }
            }
            if changed && memory.write_bytes(line_address, &line).is_none() {
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    //! Runs Cythera's `ApplyLight` in the PowerPC interpreter and the native
    //! version on the same random memory, and compares every byte. Set
    //! `SYSTEMLESS_CYTHERA_PEF` to the program's data fork (a PEF file);
    //! without it the tests skip, since the game is not in this repository.

    use super::super::pef_loader::map_instantiated_sections;
    use super::*;

    const VIEWER: u32 = 0x3000_0000;
    const TABLES: u32 = 0x3100_0000;
    const DESTINATION: u32 = 0x3200_0000;
    const STACK: u32 = 0x3300_0000;
    const HALT: u32 = 0x3400_0000;

    struct Random(u64);

    impl Random {
        fn next(&mut self) -> u32 {
            self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
            let mut z = self.0;
            z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
            (z ^ (z >> 31)) as u32
        }
    }

    fn program() -> Option<(u32, Vec<u8>)> {
        let path = std::env::var_os("SYSTEMLESS_CYTHERA_PEF")?;
        let data = std::fs::read(path).ok()?;
        let sections = map_instantiated_sections(&data).ok()?;
        let code = sections
            .into_iter()
            .find(|section| section.section_kind == SECTION_KIND_CODE)?;
        Some((code.base, code.bytes))
    }

    /// Memory with the program's code and a viewer whose blocks are
    /// `(size + 1) * 8` on a side, filled from `seed`.
    fn memory(code_base: u32, code: &[u8], seed: u64, size: i16, stride_extra: u32) -> PpcSectionMem {
        let mut random = Random(seed);
        let blocks = ((i32::from(size) + 1) * 8) as usize;
        let stride = blocks as u32 * 4 + stride_extra;
        let mut viewer = vec![0u8; VIEWER_LIGHT_LEVELS as usize + blocks * blocks + 16];
        viewer[0..2].copy_from_slice(&size.to_be_bytes());
        for index in 0..16u32 {
            let at = (VIEWER_TABLE_POINTERS + 4 * index) as usize;
            viewer[at..at + 4].copy_from_slice(&(TABLES + 256 * index).to_be_bytes());
        }
        viewer[VIEWER_DESTINATION as usize..][..4].copy_from_slice(&DESTINATION.to_be_bytes());
        viewer[VIEWER_STRIDE as usize..][..4].copy_from_slice(&stride.to_be_bytes());
        for level in &mut viewer[VIEWER_LIGHT_LEVELS as usize..][..blocks * blocks] {
            *level = match random.next() % 4 {
                0 => 0,
                1 => (random.next() % 40) as u8,
                2 => 32 + (random.next() % 224) as u8,
                _ => 1 + (random.next() % 31) as u8,
            };
        }
        let tables: Vec<u8> = (0..4096).map(|_| random.next() as u8).collect();
        let destination: Vec<u8> = (0..stride as usize * blocks * 4 + 64)
            .map(|_| random.next() as u8)
            .collect();
        let mut memory = PpcSectionMem::new();
        memory.add_readonly_region(code_base, code.to_vec());
        memory.add_region(VIEWER, viewer);
        memory.add_region(TABLES, tables);
        memory.add_region(DESTINATION, destination);
        memory.add_region(STACK, vec![0; 0x1000]);
        memory
    }

    fn original(code_base: u32, memory: &mut PpcSectionMem) {
        let mut cpu = PpcCpu::new();
        cpu.pc = code_base + APPLY_LIGHT_START as u32;
        cpu.gpr[1] = STACK + 0xf00;
        cpu.gpr[3] = VIEWER;
        cpu.lr = HALT;
        let result = cpu.run(memory, 50_000_000, HALT);
        assert!(matches!(result, PpcRunResult::Halted { .. }), "{result:?}");
    }

    fn native(memory: &mut PpcSectionMem) {
        let mut cpu = PpcCpu::new();
        cpu.gpr[3] = VIEWER;
        apply_light(&cpu, memory);
    }

    fn destination(memory: &mut PpcSectionMem, length: usize) -> Vec<u8> {
        let mut bytes = vec![0; length];
        memory.read_bytes_into(DESTINATION, &mut bytes).unwrap();
        bytes
    }

    #[test]
    fn native_light_matches_the_programs_code() {
        let Some((code_base, code)) = program() else {
            eprintln!("[TEST] skipping: set SYSTEMLESS_CYTHERA_PEF to Cythera's data fork");
            return;
        };
        assert_eq!(fnv1a(&code[SUB_LIGHT_START..SUB_LIGHT_END]), SUB_LIGHT_FNV);
        assert_eq!(fnv1a(&code[APPLY_LIGHT_START..APPLY_LIGHT_END]), APPLY_LIGHT_FNV);
        for (seed, size, extra) in [(1, 0, 0), (2, 1, 0), (3, 3, 12), (4, 5, 4), (5, 10, 0), (6, 2, 64)] {
            let blocks = ((i32::from(size) + 1) * 8) as usize;
            let length = (blocks as u32 * 4 + extra) as usize * blocks * 4 + 64;
            let mut a = memory(code_base, &code, seed, size, extra);
            let mut b = memory(code_base, &code, seed, size, extra);
            let before = destination(&mut a, length);
            original(code_base, &mut a);
            native(&mut b);
            let (from_original, from_native) =
                (destination(&mut a, length), destination(&mut b, length));
            assert_ne!(before, from_original, "seed {seed}: the original changed nothing");
            assert_eq!(from_original, from_native, "seed {seed}, size {size}, extra {extra}");
        }
    }

    #[test]
    fn a_wrong_table_entry_is_seen() {
        let Some((code_base, code)) = program() else {
            return;
        };
        let mut a = memory(code_base, &code, 7, 3, 0);
        let mut b = memory(code_base, &code, 7, 3, 0);
        let length = 32 * 4 * 32 * 4 + 64;
        original(code_base, &mut a);
        // Break one entry of one shade table for the native run only.
        let mut entry = [0u8; 1];
        b.read_bytes_into(TABLES + 256 * 5 + 77, &mut entry).unwrap();
        b.write_bytes(TABLES + 256 * 5 + 77, &[entry[0] ^ 1]).unwrap();
        native(&mut b);
        assert_ne!(destination(&mut a, length), destination(&mut b, length));
    }
}
