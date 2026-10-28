//! A QTMA tune played through QuickTime Musical Instruments.
//!
//! A tune has notes and the instrument each part requests, never a sound.
//! QuickTime played it through QuickTime Musical Instruments, a set of 8-bit
//! samples (Roland's "GM/GS Sound Set", 1997) in an extension beside
//! QuickTime. `tune_player.rs`'s synthesiser plays the right notes with a
//! plain harmonic series; this plays them with the samples, so the music
//! sounds as it did on a Mac.
//!
//! This is a port of grimoire's `js/mac-qtmusic.js` (a copy of
//! `ratlizard/mac-files`), whose header holds the reading of QuickTime's
//! synthesizer behind every rule below, and of `js/mac-installshield.js`,
//! which reads QuickTime 3's Windows installer. The names follow those files
//! so the two can be held side by side; a change to the synthesis is made
//! there first and carried here.
//!
//! The instruments are Apple's and Roland's and are not in this repository.
//! `SYSTEMLESS_QT_INSTRUMENTS` names a file holding them: QuickTime 3's
//! installer (`QUICKTIM.EXE`), its `data.z`, the `.qtx`, or a Mac resource
//! fork with the `'ssai'` resources.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use crate::systems::macintosh::managers::resource::ResourceFork;

fn be16(b: &[u8], o: usize) -> u32 {
    match b.get(o..o + 2) {
        Some(s) => u32::from(u16::from_be_bytes([s[0], s[1]])),
        None => 0,
    }
}
fn be32(b: &[u8], o: usize) -> u32 {
    match b.get(o..o + 4) {
        Some(s) => u32::from_be_bytes([s[0], s[1], s[2], s[3]]),
        None => 0,
    }
}
fn le16(b: &[u8], o: usize) -> u32 {
    match b.get(o..o + 2) {
        Some(s) => u32::from(u16::from_le_bytes([s[0], s[1]])),
        None => 0,
    }
}
fn le32(b: &[u8], o: usize) -> u32 {
    match b.get(o..o + 4) {
        Some(s) => u32::from_le_bytes([s[0], s[1], s[2], s[3]]),
        None => 0,
    }
}

// ---------------------------------------------------------------------------
// QuickTime 3's installer. QUICKTIM.EXE is a self-extracting zip whose one
// entry, `data.z`, is an InstallShield 3 archive; one of its files is
// `QuickTimeMusicalInstruments.qtx`, a Windows stub with the Mac resource
// fork after it. `js/mac-installshield.js` has the reading of the archive.

/// The fixed Huffman tables of PKWARE DCL implode's length and distance
/// codes, in DCL's compact form: each byte is (count - 1) << 4 | bit length.
const DCL_LEN_REP: [u8; 6] = [2, 35, 36, 53, 38, 23];
const DCL_DIST_REP: [u8; 7] = [2, 20, 53, 230, 247, 151, 248];
const DCL_LEN_BASE: [u32; 16] = [3, 2, 4, 5, 6, 7, 8, 9, 10, 12, 16, 24, 40, 72, 136, 264];
const DCL_LEN_EXTRA: [u32; 16] = [0, 0, 0, 0, 0, 0, 0, 0, 1, 2, 3, 4, 5, 6, 7, 8];

struct DclTable {
    count: [u32; 14],
    symbol: Vec<u32>,
}

fn dcl_table(rep: &[u8]) -> DclTable {
    let mut lengths = Vec::new();
    for &b in rep {
        for _ in 0..=(b >> 4) {
            lengths.push(u32::from(b & 15));
        }
    }
    let mut count = [0u32; 14];
    for &l in &lengths {
        count[l as usize] += 1;
    }
    let mut offs = [0u32; 14];
    for l in 1..13 {
        offs[l + 1] = offs[l] + count[l];
    }
    let mut symbol = vec![0u32; lengths.len()];
    for (s, &l) in lengths.iter().enumerate() {
        if l != 0 {
            symbol[offs[l as usize] as usize] = s as u32;
            offs[l as usize] += 1;
        }
    }
    DclTable { count, symbol }
}

/// Explode one DCL stream to `output_length` bytes. Every file in QuickTime
/// 3's archive has uncoded literals, so a stream with coded ones is refused.
fn dcl_explode(input: &[u8], output_length: usize) -> Result<Vec<u8>, String> {
    if input.len() < 2 {
        return Err("too short for a DCL stream".into());
    }
    let (coded, dict_bits) = (input[0], u32::from(input[1]));
    if coded != 0 {
        return Err(format!(
            "a DCL stream with literal flag {coded}, which is not read"
        ));
    }
    if !(4..=6).contains(&dict_bits) {
        return Err(format!("not a DCL stream (dictionary {dict_bits})"));
    }
    let len_table = dcl_table(&DCL_LEN_REP);
    let dist_table = dcl_table(&DCL_DIST_REP);
    let mut out = vec![0u8; output_length];
    let (mut pos, mut buf, mut cnt, mut o) = (2usize, 0u32, 0u32, 0usize);
    let bits = |n: u32, pos: &mut usize, buf: &mut u32, cnt: &mut u32| -> Result<u32, String> {
        while *cnt < n {
            let byte = *input.get(*pos).ok_or("the DCL stream ran out of input")?;
            *pos += 1;
            *buf |= u32::from(byte) << *cnt;
            *cnt += 8;
        }
        let v = *buf & ((1u32 << n) - 1);
        *buf >>= n;
        *cnt -= n;
        Ok(v)
    };
    // DCL's codes are stored bit-inverted, hence the ^ 1.
    let decode =
        |h: &DclTable, pos: &mut usize, buf: &mut u32, cnt: &mut u32| -> Result<u32, String> {
            let (mut code, mut first, mut index) = (0u32, 0u32, 0u32);
            for len in 1..=13 {
                code |= bits(1, pos, buf, cnt)? ^ 1;
                let c = h.count[len];
                if code.wrapping_sub(first) < c {
                    return Ok(h.symbol[(index + code - first) as usize]);
                }
                index += c;
                first += c;
                first <<= 1;
                code <<= 1;
            }
            Err("a bad code in the DCL stream".into())
        };
    loop {
        if bits(1, &mut pos, &mut buf, &mut cnt)? != 0 {
            let sym = decode(&len_table, &mut pos, &mut buf, &mut cnt)? as usize;
            let len = DCL_LEN_BASE[sym] + bits(DCL_LEN_EXTRA[sym], &mut pos, &mut buf, &mut cnt)?;
            if len == 519 {
                break; // the end code
            }
            let low_bits = if len == 2 { 2 } else { dict_bits };
            let dist = ((decode(&dist_table, &mut pos, &mut buf, &mut cnt)? << low_bits)
                + bits(low_bits, &mut pos, &mut buf, &mut cnt)?
                + 1) as usize;
            let len = len as usize;
            if dist > o {
                return Err("a DCL match reaches before the start of the file".into());
            }
            if o + len > output_length {
                return Err(format!(
                    "the DCL stream runs past the {output_length} bytes it should make"
                ));
            }
            for _ in 0..len {
                out[o] = out[o - dist];
                o += 1;
            }
        } else {
            if o >= output_length {
                return Err(format!(
                    "the DCL stream runs past the {output_length} bytes it should make"
                ));
            }
            out[o] = bits(8, &mut pos, &mut buf, &mut cnt)? as u8;
            o += 1;
        }
    }
    if o != output_length {
        return Err(format!(
            "the DCL stream made {o} bytes, not the {output_length} it should make"
        ));
    }
    Ok(out)
}

const IS3_SIGNATURE: u32 = 0x8C65_5D13;

fn looks_like_install_shield3(bytes: &[u8]) -> bool {
    bytes.len() >= 255 && le32(bytes, 0) == IS3_SIGNATURE
}

/// The file in an InstallShield 3 archive whose name ends `name`, exploded.
fn install_shield3_file(bytes: &[u8], name: &str) -> Result<Vec<u8>, String> {
    let files = le16(bytes, 0x0C);
    let file_off = le32(bytes, 0x33) as usize;
    let mut p = file_off;
    for _ in 0..files {
        if p + 30 > bytes.len() {
            return Err("the file table runs past the end of the archive".into());
        }
        let n = bytes[p + 29] as usize;
        let entry_name = bytes.get(p + 30..p + 30 + n).unwrap_or(&[]);
        let (len, packed, offset) = (
            le32(bytes, p + 3) as usize,
            le32(bytes, p + 7) as usize,
            le32(bytes, p + 11) as usize,
        );
        if entry_name.eq_ignore_ascii_case(name.as_bytes()) {
            let packed = bytes
                .get(offset..offset + packed)
                .ok_or_else(|| format!("{name} runs past the end of the archive"))?;
            return dcl_explode(packed, len);
        }
        let step = le16(bytes, p + 23) as usize;
        if step == 0 {
            return Err("a file entry of length 0".into());
        }
        p += step;
    }
    Err(format!("{name} is not in this installer"))
}

/// QUICKTIM.EXE's `data.z`, taken from its zip local header (its central
/// directory does not match where its entries are) and held to its CRC-32.
/// None for an .exe that has no data.z.
fn qt_installer_data_z(exe: &[u8]) -> Result<Option<Vec<u8>>, String> {
    let mut h = 0;
    while h + 30 < exe.len() {
        if exe[h..h + 4] != [0x50, 0x4B, 3, 4] {
            h += 1;
            continue;
        }
        let name_len = le16(exe, h + 26) as usize;
        let name = exe.get(h + 30..h + 30 + name_len).unwrap_or(&[]);
        let is_data_z = name.len() >= 6
            && name[name.len() - 6..].eq_ignore_ascii_case(b"data.z")
            && (name.len() == 6 || matches!(name[name.len() - 7], b'/' | b'\\'));
        if !is_data_z {
            h += 1;
            continue;
        }
        let (method, crc, packed, len) = (
            le16(exe, h + 8),
            le32(exe, h + 14),
            le32(exe, h + 18) as usize,
            le32(exe, h + 22) as usize,
        );
        let start = h + 30 + name_len + le16(exe, h + 28) as usize;
        let out = match method {
            0 => exe
                .get(start..start + len)
                .ok_or("data.z runs past the end")?
                .to_vec(),
            8 => {
                use std::io::Read;
                let body = exe
                    .get(start..start + packed)
                    .ok_or("data.z runs past the end")?;
                let mut out = Vec::with_capacity(len);
                flate2::read::DeflateDecoder::new(body)
                    .read_to_end(&mut out)
                    .map_err(|e| format!("data.z does not inflate: {e}"))?;
                out
            }
            m => return Err(format!("data.z is packed with zip method {m}")),
        };
        let mut sum = flate2::Crc::new();
        sum.update(&out);
        if sum.sum() != crc {
            return Err("data.z fails its CRC-32".into());
        }
        return Ok(Some(out));
    }
    Ok(None)
}

/// The resource fork of a QuickTime extension. A Mac file has it as a fork;
/// a Windows .qtx is a small PE image with the fork after its last section.
fn qtx_resource_fork(bytes: &[u8]) -> &[u8] {
    if bytes.len() > 0x40 && bytes[0] == 0x4D && bytes[1] == 0x5A {
        let pe = le32(bytes, 0x3C) as usize;
        if bytes.get(pe..pe + 2) != Some(b"PE") {
            return bytes;
        }
        let sections = le16(bytes, pe + 6) as usize;
        let opt = le16(bytes, pe + 20) as usize;
        let mut end = 0usize;
        for i in 0..sections {
            let s = pe + 24 + opt + 40 * i;
            end = end.max(le32(bytes, s + 20) as usize + le32(bytes, s + 16) as usize);
        }
        return bytes.get(end..).unwrap_or(&[]);
    }
    bytes
}

/// The resource fork holding the instruments, from whatever file was given.
fn instruments_fork(bytes: &[u8]) -> Result<Vec<u8>, String> {
    if bytes.len() > 1 && bytes[0] == 0x4D && bytes[1] == 0x5A {
        if let Some(data_z) = qt_installer_data_z(bytes)? {
            return instruments_fork(&data_z);
        }
        return Ok(qtx_resource_fork(bytes).to_vec());
    }
    if looks_like_install_shield3(bytes) {
        let qtx = install_shield3_file(bytes, "QuickTimeMusicalInstruments.qtx")?;
        return Ok(qtx_resource_fork(&qtx).to_vec());
    }
    Ok(bytes.to_vec())
}

// ---------------------------------------------------------------------------
// The instruments. Each 'ssai' resource is a QuickTime atom container
// (QuickTimeMusic.h): a 12-byte header, then atoms of { u32 size, type,
// u32 id, u16 0, u16 child count, u32 0 } and a body.

struct Atom {
    kind: [u8; 4],
    id: u32,
    start: usize,
    end: usize,
    depth: u32,
}

fn qt_atoms(
    bytes: &[u8],
    start: usize,
    end: usize,
    depth: u32,
    out: &mut Vec<Atom>,
) -> Result<(), String> {
    let mut o = start;
    while o + 20 <= end {
        let size = be32(bytes, o) as usize;
        if size < 20 || o + size > end {
            return Err(format!("an atom of {size} bytes at {o} does not fit"));
        }
        out.push(Atom {
            kind: [bytes[o + 4], bytes[o + 5], bytes[o + 6], bytes[o + 7]],
            id: be32(bytes, o + 8),
            start: o + 20,
            end: o + size,
            depth,
        });
        if be16(bytes, o + 14) != 0 {
            qt_atoms(bytes, o + 20, o + size, depth + 1, out)?;
        }
        o += size;
    }
    Ok(())
}

/// The knob numbers, kQTMSKnob* less 0x02000000.
mod knob {
    pub const ATTACK: u32 = 1;
    pub const DECAY: u32 = 2;
    pub const SUSTAIN: u32 = 3;
    pub const KEY_TO_DECAY: u32 = 5;
    pub const RELEASE: u32 = 6;
    pub const TRANSPOSE: u32 = 0x12;
    pub const DEFAULT_PAN: u32 = 0x19;
    pub const PAN_KEY_SCALING: u32 = 0x1A;
    pub const EXCLUSION_GROUP: u32 = 0x1C;
    pub const SUSTAIN_TIME: u32 = 0x1D;
    pub const SUSTAIN_INFINITE: u32 = 0x1E;
    pub const VELOCITY_LOW: u32 = 0x20;
    pub const VELOCITY_HIGH: u32 = 0x21;
    pub const VELOCITY_SENSITIVITY: u32 = 0x22;
    pub const PITCH_SENSITIVITY: u32 = 0x23;
    pub const LOG_CURVES: u32 = 0x26;
    pub const PITCH_ENVELOPE: u32 = 0x35;
    pub const PITCH_ENVELOPE_DEPTH: u32 = 0x36;
    pub const VEL_TO_ATTACK: u32 = 0x3F;
}

/// One key range of an instrument: its sample, as floats, and its knobs over
/// the instrument's.
#[derive(Debug)]
pub(crate) struct Region {
    rate: f64,
    loop_start: usize,
    loop_end: usize,
    root: i32,
    low: i32,
    high: i32,
    knobs: HashMap<u32, i32>,
    pcm: Vec<f32>,
}

impl Region {
    fn knob(&self, id: u32, default: i32) -> f64 {
        f64::from(*self.knobs.get(&id).unwrap_or(&default))
    }
}

#[derive(Debug)]
pub(crate) struct Instrument {
    pub(crate) number: i32,
    regions: Vec<Region>,
}

fn knobs_in(bytes: &[u8], a: &Atom) -> HashMap<u32, i32> {
    let mut out = HashMap::new();
    let n = be32(bytes, a.start) as usize;
    for k in 0..n {
        let id = be32(bytes, a.start + 8 + 8 * k);
        if id & 0xFF00_0000 == 0x0200_0000 {
            out.insert(id & 0xFFFF, be32(bytes, a.start + 12 + 8 * k) as i32);
        }
    }
    out
}

/// One 'ssai' resource, read. A key range's samples may be in another
/// instrument's resource (the GS variations borrow the base instrument's),
/// and sample ids are unique across the set, so `sample_of` finds one
/// wherever it is.
fn qt_atomic_instrument(
    bytes: &[u8],
    sample_of: &dyn Fn(u32) -> Option<Vec<u8>>,
) -> Result<Instrument, String> {
    let mut atoms = Vec::new();
    qt_atoms(bytes, 12, bytes.len(), 0, &mut atoms)?;
    let s32 = |o: usize| be32(bytes, o) as i32;
    let mut number = 0;
    let mut inst_knobs = HashMap::new();
    struct Desc {
        format: [u8; 4],
        channels: u32,
        bits: u32,
        rate: f64,
        sample_id: u32,
        offset: usize,
        length: usize,
        loop_start: i32,
        loop_end: i32,
        root: i32,
        low: i32,
        high: i32,
        knobs: HashMap<u32, i32>,
    }
    let mut descs: Vec<Desc> = Vec::new();
    let mut in_region = false;
    for a in &atoms {
        if a.depth == 1 && &a.kind == b"tone" {
            number = s32(a.start + 68);
        } else if a.depth == 1 && &a.kind == b"knbl" {
            inst_knobs = knobs_in(bytes, a);
        } else if a.depth == 1 {
            in_region = false;
        } else if a.depth == 2 && &a.kind == b"sdsc" {
            let o = a.start;
            descs.push(Desc {
                format: [bytes[o], bytes[o + 1], bytes[o + 2], bytes[o + 3]],
                channels: be16(bytes, o + 4),
                bits: be16(bytes, o + 6),
                rate: f64::from(be32(bytes, o + 8)) / 65536.0,
                sample_id: be16(bytes, o + 12),
                offset: s32(o + 14).max(0) as usize,
                length: s32(o + 18).max(0) as usize,
                loop_start: s32(o + 26),
                loop_end: s32(o + 30),
                root: s32(o + 34),
                low: s32(o + 38),
                high: s32(o + 42),
                knobs: HashMap::new(),
            });
            in_region = true;
        } else if a.depth == 2 && &a.kind == b"knbl" && in_region {
            if let Some(d) = descs.last_mut() {
                d.knobs = knobs_in(bytes, a);
            }
        }
    }
    let mut regions = Vec::with_capacity(descs.len());
    for d in descs {
        let mut knobs = inst_knobs.clone();
        knobs.extend(d.knobs);
        let raw = sample_of(d.sample_id).ok_or_else(|| {
            format!(
                "instrument {number} names sample {}, which is not in the set",
                d.sample_id
            )
        })?;
        let eight = &d.format == b"raw " && d.bits == 8;
        let sixteen = &d.format == b"twos" && d.bits == 16;
        if d.channels != 1 || !(eight || sixteen) {
            return Err(format!(
                "instrument {number} has a {}-channel {}-bit sample",
                d.channels, d.bits
            ));
        }
        let width = (d.bits / 8) as usize;
        let n = d.length.min(raw.len().saturating_sub(d.offset) / width);
        let pcm: Vec<f32> = if eight {
            (0..n)
                .map(|i| (f32::from(raw[d.offset + i]) - 128.0) / 128.0)
                .collect()
        } else {
            (0..n)
                .map(|i| {
                    f32::from(i16::from_be_bytes([
                        raw[d.offset + 2 * i],
                        raw[d.offset + 2 * i + 1],
                    ])) / 32768.0
                })
                .collect()
        };
        regions.push(Region {
            rate: d.rate,
            loop_start: d.loop_start.max(0) as usize,
            loop_end: d.loop_end.max(0) as usize,
            root: d.root,
            low: d.low,
            high: d.high,
            knobs,
            pcm,
        });
    }
    Ok(Instrument { number, regions })
}

/// A set of instruments from a resource fork. Instruments are read the first
/// time one is asked for; the samples' index is built across every
/// instrument the first time any is.
pub(crate) struct InstrumentLibrary {
    fork: ResourceFork,
    ids: Vec<i16>,
    samples: OnceLock<HashMap<u32, (i16, usize, usize)>>,
    read: Mutex<HashMap<i32, Option<Arc<Instrument>>>>,
}

impl std::fmt::Debug for InstrumentLibrary {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "InstrumentLibrary({} instruments)", self.ids.len())
    }
}

impl InstrumentLibrary {
    /// The instruments in whatever file holds them: QUICKTIM.EXE, its data.z,
    /// the .qtx, or a Mac resource fork.
    pub(crate) fn from_file(bytes: &[u8]) -> Result<Self, String> {
        let fork_bytes = instruments_fork(bytes)?;
        let fork = ResourceFork::parse(&fork_bytes)
            .ok_or("the instruments' resource fork does not parse")?;
        let mut ids: Vec<i16> = fork.get_all(*b"ssai").iter().map(|r| r.id).collect();
        if ids.is_empty() {
            return Err("no instruments ('ssai' resources) in this file".into());
        }
        ids.sort_unstable();
        Ok(Self {
            fork,
            ids,
            samples: OnceLock::new(),
            read: Mutex::new(HashMap::new()),
        })
    }

    pub(crate) fn len(&self) -> usize {
        self.ids.len()
    }

    fn has(&self, n: i32) -> bool {
        i16::try_from(n).is_ok_and(|n| self.ids.binary_search(&n).is_ok())
    }

    fn data(&self, id: i16) -> &[u8] {
        self.fork
            .get(*b"ssai", id)
            .map(|r| r.data.as_slice())
            .unwrap_or(&[])
    }

    fn sample_of(&self, id: u32) -> Option<Vec<u8>> {
        let index = self.samples.get_or_init(|| {
            let mut index = HashMap::new();
            for &rid in &self.ids {
                let d = self.data(rid);
                let mut atoms = Vec::new();
                if qt_atoms(d, 12, d.len(), 0, &mut atoms).is_err() {
                    continue;
                }
                let mut smin = None;
                for a in &atoms {
                    if a.depth == 1 {
                        smin = (&a.kind == b"smin").then_some(a.id);
                    } else if a.depth == 2 && &a.kind == b"sdat" {
                        if let Some(s) = smin {
                            index.insert(s, (rid, a.start, a.end));
                        }
                    }
                }
            }
            index
        });
        let &(rid, start, end) = index.get(&id)?;
        self.data(rid).get(start..end).map(<[u8]>::to_vec)
    }

    pub(crate) fn get(&self, n: i32) -> Option<Arc<Instrument>> {
        let mut read = self.read.lock().ok()?;
        if let Some(inst) = read.get(&n) {
            return inst.clone();
        }
        let inst = if self.has(n) {
            match qt_atomic_instrument(self.data(n as i16), &|id| self.sample_of(id)) {
                Ok(inst) => Some(Arc::new(inst)),
                Err(e) => {
                    eprintln!("[TUNE] QuickTime instrument {n} not read: {e}");
                    None
                }
            }
        } else {
            None
        };
        read.insert(n, inst.clone());
        inst
    }

    /// The instrument a part's note request is given: its own number, any
    /// kit as the Standard Kit, or else its General MIDI number.
    fn tone_instrument(&self, tone: Option<&Tone>) -> Option<Arc<Instrument>> {
        let tone = tone?;
        if self.has(tone.instrument) {
            return self.get(tone.instrument);
        }
        if tone.instrument >= 16384 && self.has(16385) {
            return self.get(16385);
        }
        if (1..=128).contains(&tone.gm) && self.has(tone.gm) {
            return self.get(tone.gm);
        }
        None
    }
}

/// The library `SYSTEMLESS_QT_INSTRUMENTS` names, read once. None when the
/// variable is unset or the file does not hold the instruments, which is
/// said once on stderr.
pub(crate) fn library() -> Option<&'static InstrumentLibrary> {
    static LIBRARY: OnceLock<Option<InstrumentLibrary>> = OnceLock::new();
    LIBRARY
        .get_or_init(|| {
            let path = std::env::var_os("SYSTEMLESS_QT_INSTRUMENTS")?;
            let read = std::fs::read(&path)
                .map_err(|e| e.to_string())
                .and_then(|bytes| InstrumentLibrary::from_file(&bytes));
            match read {
                Ok(lib) => {
                    eprintln!(
                        "[TUNE] QuickTime instruments: {} from {}",
                        lib.len(),
                        std::path::Path::new(&path).display()
                    );
                    Some(lib)
                }
                Err(e) => {
                    eprintln!(
                        "[TUNE] {} has no QuickTime instruments this host can read ({e}); \
                         playing tunes with the plain synthesiser",
                        std::path::Path::new(&path).display()
                    );
                    None
                }
            }
        })
        .as_ref()
}

// ---------------------------------------------------------------------------
// A tune's events, read as `qParseTune` reads them. The bit fields are
// Apple's (QuickTimeMusic.h) and are `tune_player.rs`'s; this reader keeps
// the controllers' raw values and the note requests' instrument numbers,
// which that decoder converts or drops.

#[derive(Clone, Copy, Debug)]
pub(crate) struct Tone {
    pub(crate) instrument: i32,
    pub(crate) gm: i32,
}

#[derive(Clone, Copy, Debug)]
enum EventKind {
    Note { pitch: u32, vol: u32, dur: u32 },
    Ctl { ctl: u32, val: u32 },
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Event {
    t: u32,
    part: u32,
    kind: EventKind,
}

impl Event {
    fn dur(&self) -> u32 {
        match self.kind {
            EventKind::Note { dur, .. } => dur,
            EventKind::Ctl { .. } => 0,
        }
    }
}

#[derive(Debug, Default)]
pub(crate) struct ParsedTune {
    pub(crate) events: Vec<Event>,
    pub(crate) tones: HashMap<u32, Tone>,
}

fn ext(val: u32, pos: u32, width: u32) -> u32 {
    if width >= 32 {
        val >> pos
    } else {
        (val >> pos) & ((1 << width) - 1)
    }
}

pub(crate) fn parse_tune(words: &[u32]) -> ParsedTune {
    let mut out = ParsedTune::default();
    let (mut t, mut i, mut guard) = (0u32, 0usize, 0u32);
    let n = words.len();
    while i < n && guard < 500_000 {
        guard += 1;
        let x = words[i];
        if x == 0x6000_0000 {
            break;
        }
        let ln = match ext(x, 30, 2) {
            3 => ext(x, 0, 16) as usize,
            2 => 2,
            _ => 1,
        };
        if ln == 0 || i + ln > n {
            break;
        }
        let short = ext(x, 29, 3);
        let et = if short > 3 { ext(x, 28, 4) } else { short };
        match et {
            0 => t = t.wrapping_add(ext(x, 0, 24)),
            1 => out.events.push(Event {
                t,
                part: ext(x, 24, 5),
                kind: EventKind::Note {
                    pitch: ext(x, 18, 6) + 32,
                    vol: ext(x, 11, 7),
                    dur: ext(x, 0, 11),
                },
            }),
            0x9 => {
                let (w1, w2) = (words[i], words[i + 1]);
                out.events.push(Event {
                    t,
                    part: ext(w1, 16, 12),
                    kind: EventKind::Note {
                        pitch: ext(w1, 0, 16),
                        vol: ext(w2, 22, 7),
                        dur: ext(w2, 0, 22),
                    },
                });
            }
            2 => out.events.push(Event {
                t,
                part: ext(x, 24, 5),
                kind: EventKind::Ctl {
                    ctl: ext(x, 16, 8),
                    val: ext(x, 0, 16),
                },
            }),
            3 => {
                if ext(x, 16, 8) == 0 && ext(x, 0, 16) == 0 {
                    break;
                }
            }
            0xF => {
                let part = ext(words[i], 16, 12);
                let sub = ext(words[i + ln - 1], 16, 14);
                // A note request: NoteRequestInfo (8 bytes), then a
                // ToneDescription (76) whose instrument number is at 68 and
                // General MIDI number at 72.
                if sub == 1 && ln >= 4 && (ln - 2) * 4 >= 8 + 76 {
                    let word = |byte: usize| words[i + 1 + byte / 4];
                    out.tones.insert(
                        part,
                        Tone {
                            instrument: word(8 + 68) as i32,
                            gm: word(8 + 72) as i32,
                        },
                    );
                }
            }
            _ => {}
        }
        i += ln;
    }
    out
}

// ---------------------------------------------------------------------------
// The synthesis, `qtmaRender`. The reading of each rule is in
// mac-qtmusic.js's header and comments; what is here is the arithmetic.

/// The level span of a logarithmic envelope stage, and the level a voice
/// ends under (QT_ENV).
const QT_ENV_SPAN: f64 = 1.0 / 65536.0;
const QT_ENV_FLOOR: f64 = 1.0 / 65536.0;
/// Every stage runs this many times its knob's time: the synthesizer's
/// 128-frame block over its whole milliseconds at 44,100 Hz (QT_ENV_STRETCH).
fn qt_env_stretch() -> f64 {
    (128000.0 / 44100.0) / (128000.0f64 / 44100.0).floor()
}
const QT_LEVEL_MAX: f64 = 0x3FFFF as f64 / 65536.0;
fn qt_serve_level(p: f64) -> f64 {
    if p < 1.0 {
        p * p
    } else {
        p.min(QT_LEVEL_MAX)
    }
}
const QT_VOICES: usize = 32;
fn qt_cut_frames(rate: f64) -> usize {
    ((rate * 0.004).round() as usize).max(8)
}
/// Seconds rendered after the last note for releases to ring out.
const TAIL_SECONDS: f64 = 2.0;

/// An envelope, stepped a frame at a time; `logs` is Log Curves, 1 attack,
/// 2 decay, 4 sustain, 8 release.
struct Envelope {
    level: f64,
    stage: u8,
    logs: i32,
    attack: f64,
    atk_mul: f64,
    decay_mul: f64,
    decay_add: f64,
    sus_level: f64,
    sus_mul: f64,
    sus_add: f64,
    forever: bool,
    release: f64,
    rel_mul: f64,
    rel_add: f64,
}

impl Envelope {
    #[allow(clippy::too_many_arguments)]
    fn new(
        attack: f64,
        decay: f64,
        sus_level: f64,
        sus_time: f64,
        forever: bool,
        release: f64,
        logs: i32,
    ) -> Self {
        let stretch = qt_env_stretch();
        let (attack, decay, sus_time, release) = (
            attack * stretch,
            decay * stretch,
            sus_time * stretch,
            release * stretch,
        );
        let fall = |n: f64| QT_ENV_SPAN.powf(1.0 / n.max(1.0));
        let mut en = Self {
            level: if logs & 1 != 0 { QT_ENV_FLOOR } else { 0.0 },
            stage: if attack >= 1.0 { 1 } else { 2 },
            logs,
            attack,
            atk_mul: (1.0 / QT_ENV_SPAN).powf(1.0 / attack.max(1.0)),
            decay_mul: fall(decay),
            decay_add: (sus_level - 1.0) / decay.max(1.0),
            sus_level,
            sus_mul: fall(sus_time),
            sus_add: -sus_level / sus_time.max(1.0),
            forever,
            release,
            rel_mul: fall(release),
            rel_add: 0.0,
        };
        if en.stage == 2 {
            en.level = 1.0;
        }
        en
    }

    fn off(&mut self) {
        if self.stage != 0 && self.stage != 4 {
            self.stage = 4;
            self.rel_add = -self.level / self.release.max(1.0);
        }
    }

    fn tick(&mut self) {
        let logs = self.logs;
        match self.stage {
            1 => {
                self.level = if logs & 1 != 0 {
                    self.level * self.atk_mul
                } else {
                    self.level + 1.0 / self.attack
                };
                if self.level >= 1.0 {
                    self.level = 1.0;
                    self.stage = 2;
                }
            }
            2 => {
                self.level = if logs & 2 != 0 {
                    self.level * self.decay_mul
                } else {
                    self.level + self.decay_add
                };
                if self.level <= self.sus_level {
                    self.level = self.sus_level;
                    self.stage = 3;
                }
            }
            3 => {
                if !self.forever {
                    self.level = if logs & 4 != 0 {
                        self.level * self.sus_mul
                    } else {
                        self.level + self.sus_add
                    };
                }
            }
            4 => {
                self.level = if logs & 8 != 0 {
                    self.level * self.rel_mul
                } else {
                    self.level + self.rel_add
                };
            }
            _ => {}
        }
        if self.level <= QT_ENV_FLOOR && self.stage > 1 {
            self.level = 0.0;
            self.stage = 0;
        }
    }
}

/// An LFO (AdvanceLFO): shape, delay and ramp in ms, period in ms, offset.
struct Lfo {
    shape: i32,
    delay: f64,
    ramp: f64,
    period: f64,
    offset: f64,
    seed: f64,
}

impl Lfo {
    fn at(&self, ms: f64, depth: f64) -> f64 {
        if ms < self.delay || self.period.is_nan() || self.period <= 0.0 {
            return 0.0;
        }
        let ph = (ms / self.period) % 1.0;
        let x = match self.shape {
            1 => (2.0 * std::f64::consts::PI * ph).sin(),
            2 => {
                if ph < 0.5 {
                    4.0 * ph - 1.0
                } else {
                    3.0 - 4.0 * ph
                }
            }
            3 => 2.0 * ph - 1.0,
            4 => 1.0 - 2.0 * ph,
            5 => {
                if ph > 0.5 {
                    0.0
                } else {
                    1.0
                }
            }
            6 => {
                if ph > 0.5 {
                    1.0
                } else {
                    -1.0
                }
            }
            7 => {
                let n = (ms / self.period).floor() + self.seed;
                let r = (n * 12.9898).sin() * 43758.5453;
                2.0 * (r - r.floor()) - 1.0
            }
            _ => return 0.0,
        };
        let full = x * depth + self.offset;
        if self.ramp > 0.0 && ms - self.delay < self.ramp {
            full * (ms - self.delay) / self.ramp
        } else {
            full
        }
    }
}

/// The 8.8 value of a controller, signed.
fn fixed(v: u32) -> f64 {
    (if v >= 0x8000 {
        v as f64 - 65536.0
    } else {
        v as f64
    }) / 256.0
}

struct PartState {
    volume: f64,
    expression: f64,
    pan: f64,
    bend: f64,
    wheel: f64,
    sustain_on: bool,
    sustain_off_after: Option<u32>,
}

fn state_at(timeline: &[Event], t: u32) -> PartState {
    let mut s = PartState {
        volume: 1.0,
        expression: 1.0,
        pan: 0.0,
        bend: 0.0,
        wheel: 0.0,
        sustain_on: false,
        sustain_off_after: None,
    };
    for e in timeline {
        let EventKind::Ctl { ctl, val } = e.kind else {
            continue;
        };
        if e.t > t {
            if s.sustain_on && ctl == 64 && fixed(val) <= 0.0 {
                s.sustain_off_after = Some(e.t);
                break;
            }
            continue;
        }
        let v = fixed(val);
        match ctl {
            7 => s.volume = v.clamp(0.0, 127.0) / 127.0,
            11 => s.expression = v.clamp(0.0, 127.0) / 127.0,
            10 => s.pan = v,
            32 => s.bend = v,
            1 => s.wheel = v.max(0.0),
            64 => s.sustain_on = v > 0.0,
            _ => {}
        }
    }
    s
}

struct Voice {
    part: u32,
    pitch: u32,
    region_key: (i32, usize),
    group: i32,
    born: usize,
    stage: u8,
    inst: Arc<Instrument>,
    region_index: usize,
    env: Envelope,
    penv: Option<Envelope>,
    pitch_env_depth: f64,
    gain: f64,
    pl: f64,
    pr: f64,
    f0: usize,
    off_frame: i64,
    steps: Vec<(i64, u32, f64)>,
    next_step: usize,
    bend: f64,
    wheel: f64,
    key: f64,
    transpose: f64,
    lfo_p: Lfo,
    lfo_v: Lfo,
    knob_p_depth: f64,
    knob_p_wheel: f64,
    knob_v_depth: f64,
    knob_v_wheel: f64,
    base: f64,
    ms_per_frame: f64,
    pos: f64,
    i: usize,
    step: f64,
    trem: f64,
}

impl Voice {
    fn region(&self) -> &Region {
        &self.inst.regions[self.region_index]
    }

    fn control(&mut self) {
        let t = self.i as f64 * self.ms_per_frame;
        let depth_p =
            (self.knob_p_depth + self.wheel * 256.0 * self.knob_p_wheel / 32768.0) / 256.0;
        let depth_v =
            (self.knob_v_depth + self.wheel * 256.0 * self.knob_v_wheel / 32768.0) / 65536.0;
        let penv = self
            .penv
            .as_ref()
            .map_or(0.0, |p| p.level * self.pitch_env_depth);
        let semis = self.key - f64::from(self.region().root)
            + self.transpose
            + self.bend
            + self.lfo_p.at(t, depth_p)
            + penv;
        self.step = self.base * (semis / 12.0).exp2();
        self.trem = (1.0 + self.lfo_v.at(t, depth_v)).max(0.0);
    }

    /// One frame of the sample into the output. The loop runs from
    /// loopStart up to loopEnd, not including it.
    fn one(&mut self, left: &mut [f32], right: &mut [f32]) {
        let inst = Arc::clone(&self.inst);
        let region = &inst.regions[self.region_index];
        let pcm = &region.pcm;
        let looped = region.loop_end > region.loop_start;
        let mut ip = self.pos as usize;
        if looped && ip >= region.loop_end {
            let loop_len = (region.loop_end - region.loop_start) as f64;
            self.pos -= loop_len * ((self.pos - region.loop_start as f64) / loop_len).floor();
            ip = self.pos as usize;
        }
        if ip + 1 >= pcm.len() && !looped {
            self.stage = 0;
            return;
        }
        let frac = self.pos - ip as f64;
        let a = f64::from(pcm.get(ip).copied().unwrap_or(0.0));
        let b = pcm.get(ip + 1).map_or(a, |&b| f64::from(b));
        let smp = (a + (b - a) * frac) * self.env.level * self.gain * self.trem;
        let at = self.f0 + self.i;
        left[at] += (smp * self.pl) as f32;
        right[at] += (smp * self.pr) as f32;
        self.pos += self.step;
    }

    /// Play on to frame `upto` of the tune, or to the voice's end.
    fn run(&mut self, upto: usize, left: &mut [f32], right: &mut [f32]) {
        while self.stage != 0 && self.f0 + self.i < upto {
            let i = self.i as i64;
            let mut moved = self.i & 31 == 0;
            while self.next_step < self.steps.len() && i >= self.steps[self.next_step].0 {
                let (_, ctl, v) = self.steps[self.next_step];
                self.next_step += 1;
                if ctl == 32 {
                    self.bend = v
                } else {
                    self.wheel = v.max(0.0)
                }
                moved = true;
            }
            if i >= self.off_frame && self.env.stage != 4 {
                self.env.off();
                if let Some(p) = self.penv.as_mut() {
                    p.off();
                }
            }
            self.env.tick();
            if let Some(p) = self.penv.as_mut() {
                p.tick();
            }
            self.stage = self.env.stage;
            if self.stage == 0 {
                break;
            }
            if moved {
                self.control();
            }
            self.one(left, right);
            self.i += 1;
        }
    }

    /// Cut short with a few milliseconds' straight fall, as the
    /// synthesizer's fast release does, so the cut does not click.
    fn cut(&mut self, at: usize, cut_frames: usize, left: &mut [f32], right: &mut [f32]) {
        self.run(at, left, right);
        let n = cut_frames.min(left.len().saturating_sub(self.f0 + self.i));
        let from = self.env.level;
        let mut c = 0;
        while self.stage != 0 && c < n {
            self.env.level = from * (1.0 - (c + 1) as f64 / n as f64);
            self.one(left, right);
            c += 1;
            self.i += 1;
        }
        self.stage = 0;
    }
}

/// A rendered tune: left and right at `rate`, scaled so the loudest sample
/// is just under full, and the parts whose instrument the set lacks.
pub(crate) struct Rendered {
    /// The frame the last note ends on; the frames after it are releases.
    pub(crate) last_frame: usize,
    pub(crate) left: Vec<f32>,
    pub(crate) right: Vec<f32>,
    pub(crate) missing: Vec<u32>,
}

/// Play a tune's events (times and lengths in 1/`units_per_second`) through
/// the instruments, the parts' instruments from `tones`.
pub(crate) fn render(
    events: &[Event],
    tones: &HashMap<u32, Tone>,
    lib: &InstrumentLibrary,
    rate: u32,
    units_per_second: u32,
) -> Rendered {
    let rate = f64::from(rate);
    let ups = f64::from(units_per_second.max(1));
    let last_unit = events.iter().map(|e| e.t + e.dur()).max().unwrap_or(0);
    let frames = ((f64::from(last_unit) / ups + TAIL_SECONDS) * rate).ceil() as usize;
    let mut left = vec![0f32; frames];
    let mut right = vec![0f32; frames];

    let mut parts: HashMap<u32, Option<Arc<Instrument>>> = HashMap::new();
    let mut missing = Vec::new();
    let mut ctl_timeline: HashMap<u32, Vec<Event>> = HashMap::new();
    for e in events {
        if matches!(e.kind, EventKind::Ctl { .. }) {
            ctl_timeline.entry(e.part).or_default().push(*e);
        }
    }
    let empty = Vec::new();

    let ms = |v: f64| v / 1000.0 * rate;
    let to_frame = |unit: u32| (f64::from(unit) / ups * rate).round() as i64;
    let cut_frames = qt_cut_frames(rate);
    let mut voices: Vec<Voice> = Vec::new();
    let mut notes: Vec<&Event> = events
        .iter()
        .filter(|e| matches!(e.kind, EventKind::Note { vol, .. } if vol != 0))
        .collect();
    notes.sort_by_key(|e| e.t);
    for e in notes {
        let EventKind::Note { pitch, vol, dur } = e.kind else {
            continue;
        };
        let inst = parts
            .entry(e.part)
            .or_insert_with(|| {
                let inst = lib.tone_instrument(tones.get(&e.part));
                if inst.is_none() && tones.contains_key(&e.part) {
                    missing.push(e.part);
                }
                inst
            })
            .clone();
        let Some(inst) = inst else { continue };
        let p = pitch as i32;
        let region_index = inst
            .regions
            .iter()
            .position(|r| p >= r.low && p <= r.high)
            .or_else(|| {
                let mut best: Option<usize> = None;
                for (k, r) in inst.regions.iter().enumerate() {
                    if best.is_none_or(|b| (r.root - p).abs() < (inst.regions[b].root - p).abs()) {
                        best = Some(k);
                    }
                }
                best
            });
        let Some(region_index) = region_index else {
            continue;
        };
        let region = &inst.regions[region_index];
        if region.pcm.is_empty() {
            continue;
        }
        let k = |id, default| region.knob(id, default);
        let (vol_f, pitch_f) = (f64::from(vol), f64::from(pitch));
        if vol_f < k(knob::VELOCITY_LOW, 0) || vol_f > k(knob::VELOCITY_HIGH, 127) {
            continue;
        }
        let timeline = ctl_timeline.get(&e.part).unwrap_or(&empty);
        let st = state_at(timeline, e.t);
        let mut off_unit = e.t + dur.max(1);
        if st.sustain_on {
            off_unit = off_unit.max(st.sustain_off_after.unwrap_or(last_unit));
        }

        let env = Envelope::new(
            ms(k(knob::ATTACK, 0) * (k(knob::VEL_TO_ATTACK, 65536) / 65536.0).powf(vol_f / 128.0)),
            ms(k(knob::DECAY, 1000)
                * (k(knob::KEY_TO_DECAY, 65536) / 65536.0).powf(pitch_f / 128.0)),
            k(knob::SUSTAIN, 32768) / 65536.0,
            ms(k(knob::SUSTAIN_TIME, 5000)),
            k(knob::SUSTAIN_INFINITE, 0) != 0.0,
            ms(k(knob::RELEASE, 180)),
            k(knob::LOG_CURVES, 4) as i32,
        );
        let pick = k(knob::PITCH_ENVELOPE, 0) as i32;
        let pitch_env_depth = k(knob::PITCH_ENVELOPE_DEPTH, 0) / 256.0;
        let eb = if pick == 1 { 0x27 } else { 0x2E };
        let penv = ((pick == 1 || pick == 2) && pitch_env_depth != 0.0).then(|| {
            Envelope::new(
                ms(k(eb, 0)),
                ms(k(eb + 1, 1000)),
                k(eb + 2, 32768) / 65536.0,
                ms(k(eb + 3, 5000)),
                k(eb + 4, 0) != 0.0,
                ms(k(eb + 5, 180)),
                k(eb + 6, 4) as i32,
            )
        });

        let vel = (((vol_f + 1.0) / 128.0 - 0.5) * k(knob::VELOCITY_SENSITIVITY, 100) / 100.0
            + 0.5)
            .max(0.0);
        let gain = qt_serve_level(vel * st.volume * st.expression);
        let pan_raw = if st.pan >= 1.0 {
            st.pan * 256.0
        } else {
            k(knob::DEFAULT_PAN, 384)
        };
        let pan_raw = if !(256.0..=512.0).contains(&pan_raw) {
            384.0
        } else {
            pan_raw
        };
        let pan = ((pan_raw - 256.0) / 128.0
            + k(knob::PAN_KEY_SCALING, 0) * (pitch_f - 60.0) / 8192.0)
            .clamp(0.0, 2.0);
        let f0 = to_frame(e.t).max(0) as usize;
        let off_frame = to_frame(off_unit) - f0 as i64;
        let steps = timeline
            .iter()
            .filter_map(|c| match c.kind {
                EventKind::Ctl { ctl, val }
                    if (ctl == 32 || ctl == 1) && c.t > e.t && c.t < off_unit =>
                {
                    Some((to_frame(c.t) - f0 as i64, ctl, fixed(val)))
                }
                _ => None,
            })
            .collect();
        let v = Voice {
            part: e.part,
            pitch,
            region_key: (inst.number, region_index),
            group: k(knob::EXCLUSION_GROUP, 0) as i32,
            born: f0,
            stage: env.stage,
            key: 60.0 + (pitch_f - 60.0) * k(knob::PITCH_SENSITIVITY, 100) / 100.0,
            transpose: k(knob::TRANSPOSE, 0) / 256.0,
            lfo_p: Lfo {
                shape: k(0x16, 1) as i32,
                delay: k(0x13, 0),
                ramp: k(0x14, 50),
                period: k(0x15, 250),
                offset: k(0x1B, 0) / 256.0,
                seed: f0 as f64,
            },
            lfo_v: Lfo {
                shape: k(0x0A, 1) as i32,
                delay: k(0x07, 20),
                ramp: k(0x08, 50),
                period: k(0x09, 250),
                offset: 0.0,
                seed: f0 as f64 + 7.0,
            },
            knob_p_depth: k(0x17, 0),
            knob_p_wheel: k(0x25, 16),
            knob_v_depth: k(0x0B, 0),
            knob_v_wheel: k(0x24, 49152),
            base: region.rate / rate,
            ms_per_frame: 1000.0 / rate,
            env,
            penv,
            pitch_env_depth,
            gain,
            pl: 2.0 - pan,
            pr: pan,
            f0,
            off_frame,
            steps,
            next_step: 0,
            bend: st.bend,
            wheel: st.wheel,
            pos: 0.0,
            i: 0,
            step: 0.0,
            trem: 1.0,
            inst: Arc::clone(&inst),
            region_index,
        };

        // A note struck again on its part and key while the last still
        // sounds cuts the last, as does one of the same part in the same
        // exclusion group; with no voice free, the one already in its
        // release goes first, and the oldest among equals.
        for o in &mut voices {
            o.run(f0, &mut left, &mut right);
        }
        voices.retain(|o| o.stage != 0);
        for o in &mut voices {
            if o.part == v.part
                && ((o.pitch == v.pitch && o.region_key == v.region_key)
                    || (v.group != 0 && o.group == v.group))
            {
                o.cut(f0, cut_frames, &mut left, &mut right);
            }
        }
        voices.retain(|o| o.stage != 0);
        if voices.len() >= QT_VOICES {
            let mut best = 0;
            for (n, o) in voices.iter().enumerate() {
                let b = &voices[best];
                let (so, sb) = (u8::from(o.stage == 4), u8::from(b.stage == 4));
                if so > sb || (so == sb && o.born < b.born) {
                    best = n;
                }
            }
            voices[best].cut(f0, cut_frames, &mut left, &mut right);
            voices.retain(|o| o.stage != 0);
        }
        voices.push(v);
    }
    for o in &mut voices {
        o.run(frames, &mut left, &mut right);
    }

    let peak = left
        .iter()
        .chain(right.iter())
        .fold(0f32, |m, &x| m.max(x.abs()));
    if peak > 0.0 {
        let g = 0.95 / peak;
        for x in left.iter_mut().chain(right.iter_mut()) {
            *x *= g;
        }
    }
    let last_frame = to_frame(last_unit).max(0) as usize;
    Rendered {
        last_frame,
        left,
        right,
        missing,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_dcl_stream_is_refused_when_it_is_not_one_this_reads() {
        let table = dcl_table(&DCL_LEN_REP);
        assert_eq!(table.count.iter().sum::<u32>(), 16);
        assert!(dcl_explode(&[0, 4], 0).is_err());
        assert!(dcl_explode(&[1, 4, 0], 1).is_err());
    }

    #[test]
    fn a_note_request_names_the_instrument_and_its_general_midi_number() {
        // General event, part 3, 23 longs: the length long, a NoteRequest of
        // 84 bytes (21 longs), and the closing long with subtype 1.
        let mut words = vec![0xF000_0000 | (3 << 16) | 23];
        let mut body = vec![0u32; 21];
        body[(8 + 68) / 4] = 16385;
        body[(8 + 72) / 4] = 1;
        words.extend(body);
        words.push(0xC000_0000 | (1 << 16) | 23);
        words.push(0x6000_0000);
        let tune = parse_tune(&words);
        let tone = tune.tones[&3];
        assert_eq!((tone.instrument, tone.gm), (16385, 1));
    }

    #[test]
    fn notes_and_rests_are_read_at_their_times() {
        // note part 2, pitch 60, volume 100, duration 300; rest 600; note.
        let note = |part: u32, pitch: u32, vol: u32, dur: u32| {
            (1 << 29) | (part << 24) | ((pitch - 32) << 18) | (vol << 11) | dur
        };
        let tune = parse_tune(&[
            note(2, 60, 100, 300),
            600,
            note(2, 64, 90, 300),
            0x6000_0000,
        ]);
        assert_eq!(tune.events.len(), 2);
        assert_eq!(tune.events[1].t, 600);
        assert!(matches!(
            tune.events[1].kind,
            EventKind::Note {
                pitch: 64,
                vol: 90,
                dur: 300
            }
        ));
    }

    /// Renders the tunes `QT_PORT_DIR` holds (`<hex>.tune`, the resource)
    /// and compares each with grimoire's render beside it (`<hex>-js.f32`,
    /// left then right at 44,100 Hz), which the workspace's
    /// `work/qt-record/js_render_raw.mjs` writes. Run by hand, with
    /// `SYSTEMLESS_QT_INSTRUMENTS` set: the instruments and the game's tunes
    /// are not in this repository.
    #[test]
    #[ignore]
    fn renders_as_grimoire_does() {
        let dir = std::env::var("QT_PORT_DIR").expect("QT_PORT_DIR");
        let lib = library().expect("SYSTEMLESS_QT_INSTRUMENTS");
        let mut tunes: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| {
                e.ok()?
                    .file_name()
                    .into_string()
                    .ok()?
                    .strip_suffix(".tune")
                    .map(str::to_owned)
            })
            .collect();
        tunes.sort();
        assert!(!tunes.is_empty());
        for t in tunes {
            let bytes = std::fs::read(format!("{dir}/{t}.tune")).unwrap();
            let words = |from: usize, to: usize| -> Vec<u32> {
                (from..to.min(bytes.len()))
                    .step_by(4)
                    .filter(|o| o + 4 <= to)
                    .map(|o| be32(&bytes, o))
                    .collect()
            };
            let musi = be32(&bytes, 0) as usize;
            let header = parse_tune(&words(8, musi));
            let seq = parse_tune(&words(musi, bytes.len()));
            let mut tones = header.tones;
            tones.extend(seq.tones);
            let started = std::time::Instant::now();
            let r = render(&seq.events, &tones, lib, 44100, 600);
            let ms = started.elapsed().as_millis();
            let js: Vec<f32> = std::fs::read(format!("{dir}/{t}-js.f32"))
                .unwrap()
                .chunks_exact(4)
                .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
                .collect();
            let (jl, jr) = js.split_at(js.len() / 2);
            assert_eq!(jl.len(), r.left.len(), "tune {t}: frame count");
            let (mut worst, mut err, mut sig) = (0f64, 0f64, 0f64);
            for (a, b) in r.left.iter().chain(&r.right).zip(jl.iter().chain(jr)) {
                let d = f64::from(a - b);
                worst = worst.max(d.abs());
                err += d * d;
                sig += f64::from(*b) * f64::from(*b);
            }
            let snr = 10.0 * (sig / err.max(1e-30)).log10();
            println!("tune {t}: {} frames in {ms} ms, largest difference {worst:.2e}, {snr:.1} dB under the signal", r.left.len());
            assert!(snr > 90.0, "tune {t} differs from grimoire's render");
        }
    }

    #[test]
    fn an_envelope_attacks_decays_to_its_sustain_and_releases_to_nothing() {
        let mut env = Envelope::new(10.0, 10.0, 0.5, 100.0, true, 10.0, 14);
        for _ in 0..200 {
            env.tick();
        }
        assert_eq!(env.stage, 3);
        assert!((env.level - 0.5).abs() < 1e-9);
        env.off();
        for _ in 0..200 {
            env.tick();
        }
        assert_eq!(env.stage, 0);
    }
}
