//! Shared rasterization and caching for bundled and guest TrueType outlines.
use super::bundled::bytes;
use super::*;

type Faces = (&'static FontFace, &'static MacRomanFace);
static FACES: LazyLock<Mutex<HashMap<(i16, i16), Faces>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

// Stable glyph descriptors identify their outline source without scanning faces.
#[derive(Clone, Copy)]
struct Source {
    bytes: &'static [u8],
    ppem: f32,
    hinted: bool,
    id: skrifa::GlyphId,
}
static SOURCES: LazyLock<Mutex<HashMap<usize, Source>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
static MASKS: LazyLock<Mutex<HashMap<(usize, u32), crate::memory::presentation::OutlineGlyph>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

pub(super) fn face(font_id: i16, size: i16) -> Option<Faces> {
    // Bound permanent cached storage and keep bearings within Glyph's i8
    // representation. Larger requests retain the existing bitmap scaling.
    if !(1..=96).contains(&size) {
        return None;
    }
    let bytes = bytes(font_id)?;
    let mut cache = FACES.lock().expect("URW font cache poisoned");
    if let Some(faces) = cache.get(&(font_id, size)) {
        return Some(*faces);
    }
    let mut faces = rasterize_with_strike(
        font_id,
        size,
        bytes,
        super::bundled::pixel_strike(font_id, size),
        super::compatibility::bundled_advances(font_id, size),
        super::compatibility::bundled_wid_max(font_id, size),
    )?;
    // Coppet is an optical-size-specific ASCII substitute. Retain the
    // established GetFontInfo metrics and extended Mac Roman fallback.
    // Guest resources and explicit overrides are resolved before this path.
    // A bundled pixel strike (the Geneva 9 FontStruction) takes precedence.
    if size == 9
        && matches!(font_id, FONT_APPLICATION | FONT_GENEVA)
        && super::bundled::pixel_strike(font_id, size).is_none()
    {
        let (ascii, _) = rasterize(
            font_id,
            size,
            include_bytes!("coppet/Coppet-Regular.ttf"),
            super::compatibility::bundled_advances(font_id, size),
            super::compatibility::bundled_wid_max(font_id, size),
        )?;
        faces.0 = Box::leak(Box::new(FontFace {
            font_id,
            size,
            metrics: faces.0.metrics,
            glyphs: ascii.glyphs,
            data: ascii.data,
        }));
    }
    cache.insert((font_id, size), faces);
    Some(faces)
}

#[derive(Default)]
struct OutlinePath(Vec<zeno::Command>);

impl skrifa::outline::OutlinePen for OutlinePath {
    fn move_to(&mut self, x: f32, y: f32) {
        self.0.push(zeno::Command::MoveTo((x, y).into()));
    }
    fn line_to(&mut self, x: f32, y: f32) {
        self.0.push(zeno::Command::LineTo((x, y).into()));
    }
    fn quad_to(&mut self, cx: f32, cy: f32, x: f32, y: f32) {
        self.0
            .push(zeno::Command::QuadTo((cx, cy).into(), (x, y).into()));
    }
    fn curve_to(&mut self, cx0: f32, cy0: f32, cx1: f32, cy1: f32, x: f32, y: f32) {
        self.0.push(zeno::Command::CurveTo(
            (cx0, cy0).into(),
            (cx1, cy1).into(),
            (x, y).into(),
        ));
    }
    fn close(&mut self) {
        self.0.push(zeno::Command::Close);
    }
}

/// Coverage below this is antialiasing fringe rather than a stroke crossing a
/// row, and dropout control leaves it off.
const MONO_DROPOUT_FLOOR: u8 = 64;

/// Threshold an eight-bit coverage mask to QuickDraw's one-bit mask, with
/// dropout control for glyphs the threshold would otherwise erase.
///
/// A plain threshold loses any stroke about half a pixel wide: every pixel it
/// crosses sits near 50% coverage and falls just under the cut. Geneva 9's
/// slash rasterised to rows peaking at 102, 100, 128, 116, 81, 127 and 125 and
/// kept one pixel -- a dot where Cythera's "25/25" should read -- and Monaco 9's
/// slash and backslash kept none at all. Grid-fitting cannot help a diagonal
/// the way it snaps a stem.
///
/// So, as a TrueType scan converter's dropout control does, each row the stroke
/// crosses but that is empty after thresholding gets its most-covered pixel
/// turned on. Only for a glyph the threshold has actually destroyed, though:
/// one that keeps fewer pixels than half its coverage adds up to. Applied to
/// every glyph, the same rule also turned periods into L-shapes and filled one
/// side of an A. Gated, among the bundled faces' ASCII glyphs from 9 to 14
/// points it changes only slashes, backslashes, grave accents and carets.
fn mono_mask_with_dropout(coverage: &[u8], width: usize, height: usize) -> Vec<u8> {
    let mut mask: Vec<u8> = coverage
        .iter()
        .map(|&alpha| {
            if alpha >= MONO_COVERAGE_THRESHOLD {
                255
            } else {
                0
            }
        })
        .collect();
    if width == 0 || height == 0 || coverage.len() < width * height {
        return mask;
    }
    let mass: u32 = coverage[..width * height]
        .iter()
        .map(|&c| u32::from(c))
        .sum();
    let kept = mask.iter().filter(|&&v| v != 0).count() as u32;
    if kept * 255 * 2 >= mass {
        return mask;
    }
    for y in 0..height {
        let row = y * width..(y + 1) * width;
        if mask[row.clone()].iter().any(|&v| v != 0) {
            continue;
        }
        let (x, &peak) = coverage[row]
            .iter()
            .enumerate()
            .max_by_key(|&(_, &c)| c)
            .expect("a row of a non-empty mask has a pixel");
        if peak >= MONO_DROPOUT_FLOOR {
            mask[y * width + x] = 255;
        }
    }
    mask
}

// Inside Macintosh: Text (1993), pp. 4-7–4-9 and 4-18–4-19:
// outline fonts generate a strike at the requested point size. At the logical
// 72-dpi screen, one point is one em pixel. Keep QuickDraw's binary masks and
// boolean transfer modes; hint before thresholding to retain small stems.
pub(super) fn rasterize(
    font_id: i16,
    size: i16,
    bytes: &'static [u8],
    compatibility_advances: Option<&'static [u8; 95]>,
    compatibility_wid_max: Option<i16>,
) -> Option<Faces> {
    rasterize_with_strike(
        font_id,
        size,
        bytes,
        None,
        compatibility_advances,
        compatibility_wid_max,
    )
}

/// Rasterize a face from `bytes`, taking each glyph instead from
/// `pixel_strike` where that font has it.
///
/// A pixel-grid font is a bitmap strike stored as outlines: every edge lies on
/// a grid, so drawn unhinted at the size where a grid square is a pixel, its
/// coverage is exactly 0 or 255 and the threshold returns the designer's
/// pixels. Hinting it at the nominal point size instead would scale the grid
/// and lose them.
fn rasterize_with_strike(
    font_id: i16,
    size: i16,
    bytes: &'static [u8],
    pixel_strike: Option<(&'static [u8], f32)>,
    compatibility_advances: Option<&'static [u8; 95]>,
    compatibility_wid_max: Option<i16>,
) -> Option<Faces> {
    use skrifa::{
        instance::{LocationRef, Size},
        outline::{DrawSettings, HintingInstance, Target},
        FontRef, MetadataProvider,
    };
    let font = FontRef::new(bytes).ok()?;
    let ppem = Size::new(f32::from(size));
    let location = LocationRef::default();
    let metrics = font.metrics(ppem, location);
    let advances = font.glyph_metrics(ppem, location);
    let charmap = font.charmap();
    let parsed = ttf_parser::Face::parse(bytes, 0).ok()?;
    let macintosh_cmap = parsed.tables().cmap.and_then(|cmap| {
        cmap.subtables
            .into_iter()
            .find(|table| table.platform_id == ttf_parser::PlatformId::Macintosh)
    });
    let outlines = font.outline_glyphs();
    // The output is a one-bit QuickDraw mask. LCD hinting preserves fractional
    // stem positions and therefore loses strokes when thresholded. Mono fits
    // both axes to the pixel grid and returns matching adjusted advances.
    let hinter = HintingInstance::new(&outlines, ppem, location, Target::Mono).ok()?;
    let strike = match pixel_strike {
        Some((strike_bytes, strike_ppem)) => {
            let strike_font = FontRef::new(strike_bytes).ok()?;
            Some((
                strike_bytes,
                strike_ppem,
                strike_font.charmap(),
                strike_font.outline_glyphs(),
                strike_font.glyph_metrics(Size::new(strike_ppem), location),
                strike_font.metrics(Size::new(strike_ppem), location),
            ))
        }
        None => None,
    };
    let mut data = Vec::new();
    let mut sources = Vec::new();
    let mut glyph = |ch: char| {
        let mut path = OutlinePath::default();
        let from_strike = strike.as_ref().and_then(
            |(strike_bytes, strike_ppem, strike_charmap, strike_outlines, strike_advances, _)| {
                let id = strike_charmap.map(ch).filter(|id| id.to_u32() != 0)?;
                strike_outlines
                    .get(id)?
                    .draw(
                        DrawSettings::unhinted(Size::new(*strike_ppem), location),
                        &mut path,
                    )
                    .ok()?;
                let advance = strike_advances.advance_width(id)?;
                Some((
                    Source {
                        bytes: strike_bytes,
                        ppem: *strike_ppem,
                        hinted: false,
                        id,
                    },
                    advance,
                ))
            },
        );
        let (source, advance) = match from_strike {
            Some(found) => found,
            None => {
                path.0.clear();
                let id = charmap
                    .map(ch)
                    .or_else(|| {
                        let code = crate::mac_roman::encode_mac_roman_char(ch)?;
                        let mapped = macintosh_cmap?.glyph_index(u32::from(code))?;
                        Some(skrifa::GlyphId::new(u32::from(mapped.0)))
                    })
                    .unwrap_or_default();
                let adjusted = outlines.get(id).and_then(|outline| {
                    outline
                        .draw(DrawSettings::hinted(&hinter, false), &mut path)
                        .ok()
                });
                let advance = adjusted
                    .and_then(|metrics| metrics.advance_width)
                    .or_else(|| advances.advance_width(id))
                    .unwrap_or(0.0);
                (
                    Source {
                        bytes,
                        ppem: f32::from(size),
                        hinted: true,
                        id,
                    },
                    advance,
                )
            }
        };
        sources.push(source);
        let advance = advance.round().clamp(0.0, 255.0) as u8;
        let mut result = Glyph {
            width: 0,
            height: 0,
            advance,
            origin_x: 0,
            origin_y: 0,
            data_offset: data.len(),
        };
        if !path.0.is_empty() {
            let mut coverage = Vec::new();
            let placement = zeno::Mask::new(path.0.as_slice())
                .origin(zeno::Origin::BottomLeft)
                .inspect(|format, width, height| {
                    coverage.resize(format.buffer_size(width, height), 0)
                })
                .render_into(&mut coverage, None);
            result.width = u8::try_from(placement.width).ok()?;
            result.height = u8::try_from(placement.height).ok()?;
            result.origin_x = i8::try_from(placement.left).ok()?;
            result.origin_y = i8::try_from(-placement.top).ok()?;
            data.extend(mono_mask_with_dropout(
                &coverage,
                placement.width as usize,
                placement.height as usize,
            ));
        }
        Some(result)
    };
    let mut ascii = (super::compatibility::FIRST_ASCII_CODE
        ..=super::compatibility::LAST_ASCII_CODE)
        .map(|code| glyph(char::from(code)))
        .collect::<Option<Vec<_>>>()?;
    if let Some(compatibility_advances) = compatibility_advances {
        for (glyph, advance) in ascii.iter_mut().zip(compatibility_advances) {
            glyph.advance = *advance;
        }
    }
    let extended = (0x80u8..=0xff)
        .map(|code| {
            Some(MacRomanGlyph {
                mac_code: code,
                glyph: glyph(
                    crate::mac_roman::decode_mac_roman(&[code])
                        .chars()
                        .next()
                        .unwrap(),
                )?,
            })
        })
        .collect::<Option<Vec<_>>>()?;
    let all = || {
        ascii
            .iter()
            .chain(extended.iter().map(|entry| &entry.glyph))
    };
    // A strike's own frame is the face's, as a bitmap font's ascent and
    // descent are; only the substitute's glyphs may reach past theirs.
    let (frame_ascent, frame_descent, frame_leading) = match &strike {
        Some((.., strike_metrics)) => (
            strike_metrics.ascent,
            strike_metrics.descent,
            strike_metrics.leading,
        ),
        None => (metrics.ascent, metrics.descent, metrics.leading),
    };
    let framed = || {
        all()
            .zip(sources.iter())
            .filter(|(_, source)| strike.is_none() || !source.hinted)
            .map(|(glyph, _)| glyph)
    };
    let ascent = (frame_ascent.ceil() as i16)
        .max(framed().map(|g| -i16::from(g.origin_y)).max().unwrap_or(0));
    let descent = ((-frame_descent).ceil() as i16).max(
        framed()
            .map(|g| i16::from(g.origin_y) + i16::from(g.height))
            .max()
            .unwrap_or(0),
    );
    let mapped_wid_max = all().map(|g| i16::from(g.advance)).max().unwrap_or(0);
    // Inside Macintosh: Text (1993), pp. 3-66 and 3-74: GetFontInfo.widMax
    // is the integer width of the largest glyph in the selected font.
    let selected_wid_max = if let Some(wid_max) = compatibility_wid_max {
        wid_max
    } else if compatibility_advances.is_some() {
        mapped_wid_max
    } else {
        metrics
            .max_width
            .map(|width| width.round().clamp(0.0, f32::from(i16::MAX)) as i16)
            .unwrap_or(mapped_wid_max)
    };
    let wid_max = selected_wid_max.max(mapped_wid_max);
    let data = Box::leak(data.into_boxed_slice());
    let face = Box::leak(Box::new(FontFace {
        font_id,
        size,
        metrics: FontMetrics {
            ascent,
            descent,
            wid_max,
            leading: frame_leading.round().max(0.0) as i16,
        },
        glyphs: Box::leak(ascii.into_boxed_slice()),
        data,
    }));
    let extended = Box::leak(Box::new(MacRomanFace {
        font_id,
        size,
        glyphs: Box::leak(extended.into_boxed_slice()),
        data,
    }));
    let mut registry = SOURCES.lock().ok()?;
    for (glyph, source) in face
        .glyphs
        .iter()
        .chain(extended.glyphs.iter().map(|g| &g.glyph))
        .zip(sources)
    {
        registry.insert(glyph as *const Glyph as usize, source);
    }
    Some((face, extended))
}

/// Resolve only glyphs actually supplied by this fallback, preserving resource fonts.
pub(crate) fn presentation_glyph(
    glyph: &Glyph,
    data: &[u8],
    scale: u32,
) -> Option<crate::memory::presentation::OutlineGlyph> {
    let key = (glyph as *const Glyph as usize, scale);
    let _ = data;
    let mut masks = MASKS.lock().ok()?;
    if let Some(mask) = masks.get(&key) {
        return Some(mask.clone());
    }
    let source = *SOURCES.lock().ok()?.get(&key.0)?;
    use skrifa::{
        instance::{LocationRef, Size},
        outline::{DrawSettings, HintingInstance, SmoothMode, Target},
        FontRef, MetadataProvider,
    };
    let font = FontRef::new(source.bytes).ok()?;
    let outlines = font.outline_glyphs();
    let presented = Size::new(source.ppem * scale as f32);
    let mut path = OutlinePath::default();
    if source.hinted {
        let hint = HintingInstance::new(
            &outlines,
            presented,
            LocationRef::default(),
            Target::from(SmoothMode::Normal),
        )
        .ok()?;
        outlines
            .get(source.id)?
            .draw(DrawSettings::hinted(&hint, false), &mut path)
            .ok()?;
    } else {
        outlines
            .get(source.id)?
            .draw(
                DrawSettings::unhinted(presented, LocationRef::default()),
                &mut path,
            )
            .ok()?;
    }
    let mut pixels = Vec::new();
    let placement = zeno::Mask::new(path.0.as_slice())
        .origin(zeno::Origin::BottomLeft)
        .inspect(|format, w, h| pixels.resize(format.buffer_size(w, h), 0))
        .render_into(&mut pixels, None);
    let mask = crate::memory::presentation::OutlineGlyph {
        pixels,
        width: placement.width as i32,
        height: placement.height as i32,
        left: placement.left,
        top: -placement.top,
    };
    masks.insert(key, mask.clone());
    Some(mask)
}

type UnicodeGlyph = (&'static Glyph, &'static [u8]);
static UNICODE_GLYPHS: LazyLock<Mutex<HashMap<(i16, i16, char), UnicodeGlyph>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// UI symbols not present in the Roman face use the bundled Noto symbol font.
pub(crate) fn unicode_glyph(font_id: i16, size: i16, ch: char) -> Option<UnicodeGlyph> {
    use skrifa::{
        instance::{LocationRef, Size},
        outline::{DrawSettings, HintingInstance, Target},
        FontRef, MetadataProvider,
    };
    let size = size.clamp(1, 96);
    let mut cache = UNICODE_GLYPHS.lock().ok()?;
    if let Some(glyph) = cache.get(&(font_id, size, ch)) {
        return Some(*glyph);
    }
    let primary = bytes(font_id).unwrap_or(bytes(FONT_APPLICATION)?);
    let primary_font = FontRef::new(primary).ok()?;
    let source_bytes: &'static [u8] = if primary_font.charmap().map(ch).is_some() {
        primary
    } else {
        include_bytes!("noto/NotoSansSymbols2-Regular.ttf")
    };
    let font = FontRef::new(source_bytes).ok()?;
    let id = font.charmap().map(ch)?;
    let outlines = font.outline_glyphs();
    let size_px = Size::new(size as f32);
    let hint =
        HintingInstance::new(&outlines, size_px, LocationRef::default(), Target::Mono).ok()?;
    let mut path = OutlinePath::default();
    let adjusted = outlines
        .get(id)?
        .draw(DrawSettings::hinted(&hint, false), &mut path)
        .ok()?;
    let advance = adjusted
        .advance_width
        .or_else(|| {
            font.glyph_metrics(size_px, LocationRef::default())
                .advance_width(id)
        })
        .unwrap_or(0.0)
        .round()
        .clamp(0.0, 255.0) as u8;
    let mut pixels = Vec::new();
    let placement = zeno::Mask::new(path.0.as_slice())
        .origin(zeno::Origin::BottomLeft)
        .inspect(|format, w, h| pixels.resize(format.buffer_size(w, h), 0))
        .render_into(&mut pixels, None);
    let pixels =
        mono_mask_with_dropout(&pixels, placement.width as usize, placement.height as usize);
    let data: &'static [u8] = Box::leak(pixels.into_boxed_slice());
    let glyph: &'static Glyph = Box::leak(Box::new(Glyph {
        width: placement.width.try_into().ok()?,
        height: placement.height.try_into().ok()?,
        origin_x: placement.left.try_into().ok()?,
        origin_y: (-placement.top).try_into().ok()?,
        advance,
        data_offset: 0,
    }));
    SOURCES.lock().ok()?.insert(
        glyph as *const Glyph as usize,
        Source {
            bytes: source_bytes,
            ppem: f32::from(size),
            hinted: true,
            id,
        },
    );
    cache.insert((font_id, size, ch), (glyph, data));
    Some((glyph, data))
}

#[cfg(test)]
mod tests {
    use super::*;

    const GENEVA9_ADVANCES: &[u8; 95] = include_bytes!("compatibility/geneva9-advances.bin");
    const EXPECTED_GENEVA9_ADVANCES: [u8; 95] = [
        3, 3, 5, 7, 6, 9, 8, 3, 4, 4, 7, 6, 4, 5, 3, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 4, 4, 5, 6,
        5, 6, 8, 7, 6, 6, 6, 5, 5, 6, 6, 3, 6, 6, 5, 8, 6, 6, 6, 6, 6, 6, 6, 6, 6, 8, 6, 6, 5, 4,
        6, 4, 4, 6, 3, 5, 5, 5, 5, 5, 4, 5, 5, 3, 4, 5, 3, 8, 5, 5, 5, 5, 5, 5, 4, 5, 6, 8, 6, 6,
        5, 4, 2, 4, 6,
    ];

    fn ascii_width(face: &FontFace, text: &[u8]) -> i16 {
        text.iter()
            .map(|byte| i16::from(face.glyphs[usize::from(*byte - b' ')].advance))
            .sum()
    }

    fn assert_same_glyph_except_advance(left: &Glyph, right: &Glyph) {
        assert_eq!(left.width, right.width);
        assert_eq!(left.height, right.height);
        assert_eq!(left.origin_x, right.origin_x);
        assert_eq!(left.origin_y, right.origin_y);
        assert_eq!(left.data_offset, right.data_offset);
    }

    #[test]
    fn geneva9_compatibility_component_has_exact_ofl_advance_bytes() {
        assert_eq!(*GENEVA9_ADVANCES, EXPECTED_GENEVA9_ADVANCES);
        assert_eq!(GENEVA9_ADVANCES.len(), usize::from(0x7eu8 - 0x20 + 1));
    }

    #[test]
    fn bundled_geneva9_uses_compatibility_advances() {
        let (face, _) = super::face(FONT_GENEVA, 9).expect("bundled Geneva 9");
        assert_eq!(
            face.glyphs
                .iter()
                .map(|glyph| glyph.advance)
                .collect::<Vec<_>>(),
            GENEVA9_ADVANCES.as_slice()
        );
    }

    #[test]
    fn bundled_application9_uses_geneva9_compatibility_advances() {
        let (face, _) = super::face(FONT_APPLICATION, 9).expect("bundled Application 9");
        assert_eq!(
            face.glyphs
                .iter()
                .map(|glyph| glyph.advance)
                .collect::<Vec<_>>(),
            GENEVA9_ADVANCES.as_slice()
        );
    }

    #[test]
    fn coppet9_keeps_guest_ink_and_retained_outline_sources() {
        for family in [FONT_GENEVA, FONT_APPLICATION] {
            let (face, _) = super::face(family, 9).unwrap();
            for (index, glyph) in face.glyphs.iter().enumerate().skip(1) {
                let count = usize::from(glyph.width) * usize::from(glyph.height);
                assert!(
                    face.data[glyph.data_offset..glyph.data_offset + count]
                        .iter()
                        .any(|&p| p == 255),
                    "character {} lost its guest ink",
                    index + 32
                );
                let high = presentation_glyph(glyph, face.data, 4)
                    .expect("Coppet retains its own outline source");
                assert!(high.pixels.iter().any(|&p| p > 0));
            }
            let i = &face.glyphs[usize::from(b'i' - b' ')];
            let w = usize::from(i.width);
            let pixels = &face.data[i.data_offset..i.data_offset + w * usize::from(i.height)];
            let rows = pixels.chunks_exact(w).collect::<Vec<_>>();
            assert!(rows[0].contains(&255), "i dot must survive");
            assert!(rows[1].iter().all(|&p| p == 0), "i dot stays separate");
            assert!(
                rows[2].iter().filter(|&&p| p == 255).count() >= 2,
                "i entry stroke must survive"
            );
        }
    }

    #[test]
    fn geneva9_apeiron_phrase_spans_match_recorded_origins() {
        const FIRST: &[u8] = b"This is your shooter.  There are many like it, ";
        const SECOND: &[u8] = b"but this one is yours.  Mind it well, cuz it is ";
        let (face, _) = super::face(FONT_GENEVA, 9).expect("bundled Geneva 9");

        assert_eq!((FIRST.len(), SECOND.len()), (47, 48));
        assert_eq!(ascii_width(face, FIRST), 207);
        assert_eq!(ascii_width(face, SECOND), 200);
        let bold = crate::quickdraw::text::QuickDrawTextStyle::from_bits(
            crate::quickdraw::text::QuickDrawTextStyle::BOLD_BIT,
        );
        assert_eq!(
            FIRST
                .iter()
                .map(|byte| bold
                    .glyph_advance(i32::from(face.glyphs[usize::from(*byte - b' ')].advance)))
                .sum::<i32>(),
            254
        );
        assert_eq!(
            SECOND
                .iter()
                .map(|byte| bold
                    .glyph_advance(i32::from(face.glyphs[usize::from(*byte - b' ')].advance)))
                .sum::<i32>(),
            248
        );
    }

    fn glyph_rows(face_data: &[u8], glyph: &Glyph) -> Vec<String> {
        let width = usize::from(glyph.width);
        (0..usize::from(glyph.height))
            .map(|y| {
                (0..width)
                    .map(|x| {
                        if face_data[glyph.data_offset + y * width + x] != 0 {
                            '#'
                        } else {
                            '.'
                        }
                    })
                    .collect()
            })
            .collect()
    }

    /// Geneva 9's pixels come from the pixel-grid recreation, its advances
    /// still from the compatibility component, and its frame is a bitmap
    /// strike's: ten above the baseline, two below, no leading.
    #[test]
    fn geneva9_draws_the_recreations_pixels_in_the_classic_frame() {
        let (face, _) = super::face(FONT_GENEVA, 9).unwrap();
        let glyph = |ch: u8| &face.glyphs[usize::from(ch - b' ')];
        assert_eq!(
            glyph_rows(face.data, glyph(b'H')),
            ["#...#", "#...#", "#...#", "#####", "#...#", "#...#", "#...#"]
        );
        assert_eq!((glyph(b'H').origin_x, glyph(b'H').origin_y), (0, -7));
        assert_eq!(
            glyph_rows(face.data, glyph(b'/')),
            ["...#", "...#", "..#.", "..#.", ".#..", ".#..", "#...", "#..."]
        );
        assert_eq!(
            (
                face.metrics.ascent,
                face.metrics.descent,
                face.metrics.leading
            ),
            (10, 2, 0)
        );
        let bytes = super::bytes(FONT_GENEVA).unwrap();
        let (substitute, _) = super::rasterize(FONT_GENEVA, 9, bytes, None, None).unwrap();
        assert_ne!(
            glyph_rows(
                substitute.data,
                &substitute.glyphs[usize::from(b'/' - b' ')]
            ),
            glyph_rows(face.data, glyph(b'/')),
            "the substitute outline must differ, or this test proves nothing"
        );
    }

    /// A character the recreation does not have is still drawn, from the
    /// substitute outline, rather than as the recreation's missing glyph.
    #[test]
    fn characters_missing_from_the_strike_fall_back_to_the_substitute() {
        use skrifa::MetadataProvider;
        let (strike_bytes, _) = super::super::bundled::pixel_strike(FONT_GENEVA, 9).unwrap();
        let strike = skrifa::FontRef::new(strike_bytes).unwrap();
        let (_, extended) = super::face(FONT_GENEVA, 9).unwrap();
        let bytes = super::bytes(FONT_GENEVA).unwrap();
        let (_, substitute) = super::rasterize(FONT_GENEVA, 9, bytes, None, None).unwrap();
        let missing = extended
            .glyphs
            .iter()
            .zip(substitute.glyphs)
            .filter(|(entry, _)| {
                let ch = crate::mac_roman::decode_mac_roman(&[entry.mac_code])
                    .chars()
                    .next()
                    .unwrap();
                strike.charmap().map(ch).is_none()
            })
            .collect::<Vec<_>>();
        assert!(!missing.is_empty(), "the recreation covers every character");
        for (ours, theirs) in missing {
            assert_eq!(ours.mac_code, theirs.mac_code);
            assert!(ours.glyph.width > 0 || ours.glyph.advance > 0);
            assert_eq!(
                glyph_rows(extended.data, &ours.glyph),
                glyph_rows(substitute.data, &theirs.glyph)
            );
        }
    }

    /// The smooth presentation surface draws a strike glyph as whole pixels
    /// enlarged, not as a re-hinted outline.
    #[test]
    fn strike_glyphs_present_as_enlarged_pixels() {
        let (face, _) = super::face(FONT_GENEVA, 9).unwrap();
        let glyph = &face.glyphs[usize::from(b'H' - b' ')];
        let ink = face.data[glyph.data_offset
            ..glyph.data_offset + usize::from(glyph.width) * usize::from(glyph.height)]
            .iter()
            .filter(|&&value| value != 0)
            .count();
        let presented = super::presentation_glyph(glyph, face.data, 4).unwrap();
        assert!(presented
            .pixels
            .iter()
            .all(|&alpha| alpha == 0 || alpha == 255));
        assert_eq!(
            presented
                .pixels
                .iter()
                .filter(|&&alpha| alpha == 255)
                .count(),
            ink * 16
        );
    }

    #[test]
    fn monaco_compatibility_changes_only_the_selected_maximum_advance() {
        let bytes = super::bytes(FONT_MONACO).expect("bundled Monaco bytes");
        for size in [14, 23] {
            let (raw, raw_extended) =
                super::rasterize(FONT_MONACO, size, bytes, None, None).unwrap();
            let (bundled, bundled_extended) = super::face(FONT_MONACO, size).unwrap();

            assert_eq!(raw.data, bundled.data);
            assert_eq!(raw.glyphs.len(), bundled.glyphs.len());
            for (raw_glyph, bundled_glyph) in raw.glyphs.iter().zip(bundled.glyphs) {
                assert_same_glyph_except_advance(raw_glyph, bundled_glyph);
                assert_eq!(raw_glyph.advance, bundled_glyph.advance);
            }
            assert_eq!(raw_extended.data, bundled_extended.data);
            assert_eq!(raw_extended.glyphs.len(), bundled_extended.glyphs.len());
            for (raw_glyph, bundled_glyph) in
                raw_extended.glyphs.iter().zip(bundled_extended.glyphs)
            {
                assert_eq!(raw_glyph.mac_code, bundled_glyph.mac_code);
                assert_same_glyph_except_advance(&raw_glyph.glyph, &bundled_glyph.glyph);
                assert_eq!(raw_glyph.glyph.advance, bundled_glyph.glyph.advance);
            }
            assert_eq!(
                bundled.metrics.wid_max,
                super::super::compatibility::bundled_wid_max(FONT_MONACO, size)
                    .expect("classic scalable Monaco maximum advance")
            );
            assert_ne!(raw.metrics.wid_max, bundled.metrics.wid_max);
        }
    }

    #[test]
    fn compatibility_does_not_change_other_bundled_faces_or_guest_sfnt_rasterization() {
        for (font_id, size) in [
            (FONT_GENEVA, 8),
            (FONT_GENEVA, 10),
            (FONT_APPLICATION, 10),
            (FONT_HELVETICA, 9),
            (FONT_COURIER, 23),
        ] {
            let bytes = super::bytes(font_id).unwrap();
            let (raw, _) = super::rasterize(font_id, size, bytes, None, None).unwrap();
            let (bundled, _) = super::face(font_id, size).unwrap();
            assert_eq!(raw.data, bundled.data);
            assert_eq!(
                raw.glyphs
                    .iter()
                    .map(|glyph| glyph.advance)
                    .collect::<Vec<_>>(),
                bundled
                    .glyphs
                    .iter()
                    .map(|glyph| glyph.advance)
                    .collect::<Vec<_>>()
            );
            assert_eq!(raw.metrics.wid_max, bundled.metrics.wid_max);
        }

        let guest_bytes = include_bytes!("urw/NimbusMonoPS-Regular.ttf");
        let (guest, _) = super::rasterize(FONT_GENEVA, 9, guest_bytes, None, None).unwrap();
        assert!(
            guest
                .glyphs
                .windows(2)
                .all(|pair| pair[0].advance == pair[1].advance),
            "a guest sfnt registered as Geneva 9 must retain its monospaced metrics"
        );
    }

    #[test]
    fn menu_symbols_have_outline_coverage_at_both_resolutions() {
        for ch in ['\u{2318}', '\u{2713}', '\u{2122}'] {
            let (glyph, data) = unicode_glyph(FONT_CHICAGO, 12, ch).expect("bundled menu symbol");
            assert!(glyph.advance > 0 && data.iter().any(|p| *p == 255));
            let high = presentation_glyph(glyph, data, 4).expect("symbol outline source");
            assert!(high.pixels.iter().any(|p| *p > 0 && *p < 255));
        }
    }

    #[test]
    fn outlines_supply_exact_sizes_and_extended_mac_roman() {
        for family in [
            FONT_CHICAGO,
            FONT_GENEVA,
            FONT_MONACO,
            FONT_NEWYORK,
            FONT_PALATINO,
            FONT_VENICE,
            FONT_LONDON,
            FONT_CAIRO,
        ] {
            for size in [9, 12, 17, 24, 40, 96] {
                let (face, extended) = super::face(family, size).unwrap();
                assert_eq!(face.size, size);
                assert_eq!(face.glyphs.len(), 95);
                assert_eq!(extended.glyphs.len(), 128);
                assert!(face.data.iter().all(|&value| value == 0 || value == 255));
                for (glyph, data) in face.glyphs.iter().map(|g| (g, face.data)).chain(
                    extended
                        .glyphs
                        .iter()
                        .map(|entry| (&entry.glyph, extended.data)),
                ) {
                    assert!(
                        glyph.data_offset + usize::from(glyph.width) * usize::from(glyph.height)
                            <= data.len()
                    );
                }
                assert!(std::ptr::eq(face, super::face(family, size).unwrap().0));
            }
        }
        let (face, numerator, denominator) = get_font_face_scale_ratio(FONT_GENEVA, 17);
        assert_eq!((face.size, numerator, denominator), (17, 17, 17));
        let (accent, data) = get_macroman_glyph(FONT_GENEVA, 17, 0x8e).unwrap(); // é
        assert!(data[accent.data_offset
            ..accent.data_offset + usize::from(accent.width) * usize::from(accent.height)]
            .iter()
            .any(|&v| v > 0));
    }

    /// Dropout control is only for a glyph the threshold erased. A comma's
    /// tail fades through a row of fringe below its head; lighting that row
    /// made Geneva 9's comma and semicolon a pixel taller, so a glyph that kept
    /// at least half its coverage is thresholded and nothing more.
    #[test]
    fn dropout_leaves_a_glyph_the_threshold_kept() {
        let comma = [200, 90, 10];
        assert_eq!(super::mono_mask_with_dropout(&comma, 1, 3), vec![255, 0, 0]);

        // Geneva 9's slash as rasterised: one pixel reaches the threshold.
        #[rustfmt::skip]
        let slash = [
            0, 0, 24, 102,
            0, 0, 100, 27,
            0, 0, 128, 0,
            0, 10, 116, 0,
            0, 81, 47, 0,
            0, 127, 0, 0,
            2, 125, 0, 0,
        ];
        #[rustfmt::skip]
        let expected = vec![
            0, 0, 0, 255,
            0, 0, 255, 0,
            0, 0, 255, 0,
            0, 0, 255, 0,
            0, 255, 0, 0,
            0, 255, 0, 0,
            0, 255, 0, 0,
        ];
        assert_eq!(super::mono_mask_with_dropout(&slash, 4, 7), expected);
    }

    /// A stroke about half a pixel wide must survive thresholding. Geneva 9's
    /// slash used to keep one pixel and Monaco 9's none; with dropout control
    /// both are a diagonal with ink on every row, rising left to right.
    #[test]
    fn thin_diagonals_survive_the_one_bit_threshold() {
        for family in [FONT_GENEVA, FONT_MONACO] {
            let (face, _) = super::face(family, 9).unwrap();
            let g = &face.glyphs[(b'/' - b' ') as usize];
            let w = usize::from(g.width);
            let h = usize::from(g.height);
            let px = &face.data[g.data_offset..g.data_offset + w * h];
            let columns: Vec<Option<usize>> = (0..h)
                .map(|y| (0..w).find(|&x| px[y * w + x] != 0))
                .collect();
            assert!(
                columns.iter().all(Option::is_some),
                "{family}/9: every row of the slash must have ink, got {columns:?}"
            );
            let first = columns[0].unwrap();
            let last = columns[h - 1].unwrap();
            assert!(
                first > last,
                "{family}/9: the slash must rise left to right"
            );
        }
    }

    #[test]
    fn monochrome_small_text_keeps_stems_counters_and_baselines() {
        for family in [FONT_GENEVA, FONT_MONACO] {
            for size in 9..=12 {
                let (face, _) = super::face(family, size).unwrap();
                let h = &face.glyphs[(b'H' - b' ') as usize];
                let w = usize::from(h.width);
                let pixels = &face.data[h.data_offset..h.data_offset + w * usize::from(h.height)];
                let rows = pixels
                    .chunks_exact(w)
                    .filter(|row| row.iter().any(|&v| v != 0))
                    .collect::<Vec<_>>();
                let stems = (0..w)
                    .filter(|&x| rows.iter().all(|row| row[x] == 255))
                    .count();
                assert!(
                    stems >= 2,
                    "{family}/{size}: H must have two unbroken stems"
                );
                assert!(h.origin_y < 0);
                assert!(
                    i16::from(h.origin_y) + i16::from(h.height) <= 1,
                    "H must sit above the baseline"
                );

                let o = &face.glyphs[(b'o' - b' ') as usize];
                let w = usize::from(o.width);
                let h = usize::from(o.height);
                let pixels = &face.data[o.data_offset..o.data_offset + w * h];
                let mut outside = vec![false; pixels.len()];
                let mut queue = (0..pixels.len())
                    .filter(|&i| i < w || i >= w * (h - 1) || i % w == 0 || i % w == w - 1)
                    .collect::<Vec<_>>();
                while let Some(i) = queue.pop() {
                    if outside[i] || pixels[i] != 0 {
                        continue;
                    }
                    outside[i] = true;
                    if i >= w {
                        queue.push(i - w);
                    }
                    if i + w < pixels.len() {
                        queue.push(i + w);
                    }
                    if i % w != 0 {
                        queue.push(i - 1);
                    }
                    if i % w + 1 < w {
                        queue.push(i + 1);
                    }
                }
                assert!(
                    pixels
                        .iter()
                        .zip(outside)
                        .any(|(&ink, outside)| ink == 0 && !outside),
                    "{family}/{size}: o must retain its enclosed counter"
                );
            }
        }
    }

    #[test]
    fn monospaced_advances_and_em_scale_are_preserved() {
        let (face, _) = super::face(FONT_COURIER, 20).unwrap();
        assert!(face.glyphs.iter().all(|glyph| glyph.advance == 12)); // 600/1000 em
        assert!(super::face(FONT_GENEVA, 97).is_none());
        assert!(super::face(30000, 12).is_none());
    }

    #[test]
    fn guest_outline_wins_for_an_extended_character_drawn_first() {
        assert!(register_resource_outline_font(
            30002,
            include_bytes!("urw/NimbusMonoPS-Regular.ttf")
        ));
        let (accent, _) = get_macroman_glyph(30002, 23, 0x8e).unwrap();
        let face = get_font_face(30002, 23).unwrap();
        assert_eq!(accent.advance, face.glyphs[0].advance);
        assert!(face
            .glyphs
            .iter()
            .all(|glyph| glyph.advance == accent.advance));
        // Registration is process-global, so use a unique compatibility family
        // for this test instead of changing a family shared by other tests.
    }

    #[test]
    fn guest_bitmap_strike_wins_even_after_urw_is_cached() {
        super::face(FONT_COURIER, 19).unwrap();
        // Keep this family/size unique so parallel tests do not share a strike.
        assert!(register_resource_font_strike_for_family(
            FONT_COURIER,
            19,
            &super::super::tests::minimal_nfnt()
        ));
        let face = get_font_face(FONT_COURIER, 19).unwrap();
        assert_eq!(face.metrics.ascent, 1);
        assert_eq!(face.glyphs[0].advance, 1);
    }
}
