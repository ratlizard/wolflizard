//! CDToons (`'QkBk'`) video decoding.
//!
//! Brøderbund shipped CDToons as a QuickTime decompressor component inside
//! its applications (Where in Time is Carmen Sandiego? registers `'thng'`
//! "CDToons PPC" with RegisterComponentResource). HLE does not run guest
//! components, so the frames are decoded here.
//!
//! The format follows FFmpeg's `libavcodec/cdtoons.c` (Alyssa Milburn, 2020,
//! LGPL-2.1-or-later): a frame introduces sprites that persist until an end
//! frame, then either draws "Diff" blocks embedded in the sample (an intra
//! frame) or redraws the referenced sprites. Sprites and diffs are run-length
//! scanlines of 8-bit indices; index 0 is transparent. Palettes are sprites
//! of 256 QuickTime 16-bit RGB triples.

const HEADER_SIZE: usize = 44;
const MAX_SPRITES: usize = 1200;
const PALETTE_SIZE: usize = 256 * 2 * 3;

#[derive(Debug, Clone, PartialEq, Eq)]
struct Sprite {
    end_frame: u16,
    data: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PpcCdToonsDecoder {
    pub(crate) width: usize,
    pub(crate) height: usize,
    /// The sample most recently decoded into `frame`.
    pub(crate) sample_index: usize,
    frame: Vec<u8>,
    palette: Vec<[u8; 3]>,
    last_palette_id: u16,
    sprites: Vec<Option<Sprite>>,
}

fn be16(data: &[u8], offset: usize) -> Option<u16> {
    Some(u16::from_be_bytes(data.get(offset..offset + 2)?.try_into().ok()?))
}

fn be32(data: &[u8], offset: usize) -> Option<u32> {
    Some(u32::from_be_bytes(data.get(offset..offset + 4)?.try_into().ok()?))
}

impl PpcCdToonsDecoder {
    pub(crate) fn new(width: usize, height: usize) -> Option<Self> {
        if width == 0 || height == 0 || width > 4096 || height > 4096 {
            return None;
        }
        Some(Self {
            width,
            height,
            sample_index: 0,
            frame: vec![0; width.checked_mul(height)?],
            palette: vec![[0; 3]; 256],
            last_palette_id: 0,
            sprites: vec![None; MAX_SPRITES],
        })
    }

    /// Draws run-length scanlines at (`left`, `top`). Returns false when the
    /// data ran out, which FFmpeg reports and otherwise ignores.
    fn render(&mut self, data: &[u8], left: i32, top: i32, width: i32, height: i32) -> bool {
        let frame_width = self.width as i32;
        let frame_height = self.height as i32;
        let width = width.min(frame_width - left);
        let height = height.min(frame_height - top);
        let (skip, left) = if left < 0 { (-left, 0) } else { (0, left) };
        if width <= skip {
            return true;
        }
        let mut line = 0usize;
        for y in 0..height {
            let Some(line_size) = be16(data, line) else {
                return false;
            };
            let mut pos = line + 2;
            let next = pos + usize::from(line_size);
            if next > data.len() {
                return false;
            }
            line = next;
            if top + y < 0 {
                continue;
            }
            let row = ((top + y) * frame_width + left) as usize;
            let mut to_skip = skip;
            let mut x = 0;
            while x < width - skip {
                let Some(&val) = data.get(pos) else {
                    return false;
                };
                pos += 1;
                let raw = val & 0x80 == 0;
                let mut size = i32::from(val & 0x7F) + 1;
                if to_skip >= size {
                    to_skip -= size;
                    pos += if raw { size as usize } else { 1 };
                    if pos > next {
                        return false;
                    }
                    continue;
                } else if to_skip > 0 {
                    size -= to_skip;
                    if raw {
                        pos += to_skip as usize;
                        if pos > next {
                            return false;
                        }
                    }
                    to_skip = 0;
                }
                if x + size >= width - skip {
                    size = width - skip - x;
                }
                let dest = row + x as usize;
                let n = size as usize;
                if raw {
                    let Some(src) = data.get(pos..pos + n).filter(|_| pos + n <= next) else {
                        return false;
                    };
                    self.frame[dest..dest + n].copy_from_slice(src);
                    pos += n;
                } else {
                    let Some(&color) = data.get(pos) else {
                        return false;
                    };
                    pos += 1;
                    if color != 0 {
                        self.frame[dest..dest + n].fill(color);
                    }
                }
                x += size;
            }
        }
        true
    }

    pub(crate) fn decode_sample(&mut self, sample: &[u8]) -> Option<()> {
        if sample.len() < HEADER_SIZE {
            return None;
        }
        let frame_id = be16(sample, 2)?;
        let background = sample[7];
        let sprite_count = be16(sample, 28)?;
        let sprite_offset = usize::from(be16(sample, 30)?);
        let referenced_count = sample[34];
        let palette_id = be16(sample, 36)?;
        let palette_set = sample[38];
        if sprite_offset > sample.len() {
            return None;
        }

        // Sprites introduced by this frame.
        let mut pos = sprite_offset;
        for _ in 0..sprite_count {
            let id = usize::from(be16(sample, pos)?);
            if id >= MAX_SPRITES || self.sprites[id].is_some() {
                return None;
            }
            let size = be32(sample, pos + 4)? as usize;
            if size < 14 {
                return None;
            }
            let end_frame = be16(sample, pos + 10)?;
            pos += 14;
            let data = sample.get(pos..pos + size - 14)?.to_vec();
            pos += size - 14;
            self.sprites[id] = Some(Sprite { end_frame, data });
        }

        // Diff blocks embedded in the sample make it an intra frame.
        let mut intra = false;
        while pos < sample.len() {
            let tag = be32(sample, pos)?;
            let size = be32(sample, pos + 4)? as usize;
            pos += 8;
            if tag == u32::from_be_bytes(*b"Diff") {
                let count = be16(sample, pos)?;
                pos += 10;
                for _ in 0..count {
                    let top = be16(sample, pos)? as i16;
                    let left = be16(sample, pos + 2)? as i16;
                    let diff_size = be32(sample, pos + 8)? as usize;
                    let width = be16(sample, pos + 12)?;
                    let height = be16(sample, pos + 14)?;
                    pos += 16;
                    if diff_size < 8 || diff_size - 4 > sample.len() - pos {
                        return None;
                    }
                    let body = &sample[pos + 4..pos + diff_size - 4];
                    let body = body.to_vec();
                    self.render(
                        &body,
                        i32::from(left),
                        i32::from(top),
                        i32::from(width),
                        i32::from(height),
                    );
                    pos += diff_size - 4;
                }
                intra = true;
            } else {
                if size < 8 || size - 8 > sample.len() - pos {
                    return None;
                }
                pos += size - 8;
            }
        }

        if !intra {
            // Sprites referenced by this frame, drawn in order.
            let mut pos = HEADER_SIZE;
            for i in 0..referenced_count {
                if pos + 10 > sprite_offset {
                    return None;
                }
                let id = usize::from(be16(sample, pos)?);
                let top = be16(sample, pos + 2)? as i16;
                let left = be16(sample, pos + 4)? as i16;
                let right = be16(sample, pos + 8)? as i16;
                pos += 10;
                if i == 0 && id == 0 {
                    self.frame.fill(background);
                }
                if right == 0 {
                    continue;
                }
                if id >= MAX_SPRITES {
                    return None;
                }
                let Some(sprite) = self.sprites[id].as_ref() else {
                    continue;
                };
                if sprite.data.len() < 14 {
                    continue;
                }
                let height = be16(&sprite.data, 0)?;
                let width = be16(&sprite.data, 2)?;
                let body = sprite.data[14..].to_vec();
                self.render(
                    &body,
                    i32::from(left),
                    i32::from(top),
                    i32::from(width),
                    i32::from(height),
                );
            }

            if palette_id != 0 && palette_id != self.last_palette_id {
                let id = usize::from(palette_id);
                if id >= MAX_SPRITES {
                    return None;
                }
                if let Some(sprite) = self.sprites[id].as_ref() {
                    if sprite.data.len() != PALETTE_SIZE {
                        return None;
                    }
                    self.last_palette_id = palette_id;
                    if palette_set == 0 {
                        for (entry, rgb) in self.palette.iter_mut().zip(sprite.data.chunks(6)) {
                            *entry = [rgb[0], rgb[2], rgb[4]];
                        }
                        // Entry 0 is the transparent colour; FFmpeg shows it black.
                        self.palette[0] = [0; 3];
                    }
                }
            }
        }

        // Sprites whose last frame has passed are discarded.
        for slot in &mut self.sprites {
            if slot.as_ref().is_some_and(|sprite| sprite.end_frame <= frame_id) {
                *slot = None;
            }
        }
        Some(())
    }

    /// Diagnostic: non-zero pixels, live sprites and the palette in use.
    pub(crate) fn summary(&self) -> String {
        format!(
            "nonzero={} sprites={} palette={} pal1={:?}",
            self.frame.iter().filter(|&&index| index != 0).count(),
            self.sprites.iter().filter(|slot| slot.is_some()).count(),
            self.last_palette_id,
            self.palette[1]
        )
    }

    pub(crate) fn rgb(&self) -> Vec<u8> {
        let mut rgb = Vec::with_capacity(self.frame.len() * 3);
        for &index in &self.frame {
            rgb.extend_from_slice(&self.palette[usize::from(index)]);
        }
        rgb
    }
}
