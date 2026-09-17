//! Bundled font family selection; assets retain their upstream names and licences.
use super::*;

pub(super) fn bytes(font_id: i16) -> Option<&'static [u8]> {
    Some(match font_id {
        FONT_CHICAGO => include_bytes!("urw/NimbusSans-Bold.ttf"),
        FONT_APPLICATION | FONT_GENEVA | FONT_HELVETICA => {
            include_bytes!("urw/NimbusSans-Regular.ttf")
        }
        FONT_MONACO | FONT_COURIER => include_bytes!("urw/NimbusMonoPS-Regular.ttf"),
        FONT_NEWYORK | FONT_TIMES => include_bytes!("urw/NimbusRoman-Regular.ttf"),
        FONT_PALATINO => include_bytes!("urw/P052-Roman.ttf"),
        FONT_VENICE => include_bytes!("urw/Z003-MediumItalic.ttf"),
        FONT_LONDON => include_bytes!("urw/C059-Bold.ttf"),
        FONT_CAIRO => include_bytes!("urw/URWGothic-Demi.ttf"),
        _ => return None,
    })
}

/// A recreation of a classic bitmap strike, drawn on a pixel grid, and the
/// pixels-per-em at which one grid square is one screen pixel. Its glyphs
/// replace the substitute outline's for that one size; characters it lacks
/// still come from the substitute.
pub(super) fn pixel_strike(font_id: i16, size: i16) -> Option<(&'static [u8], f32)> {
    match (font_id, size) {
        (FONT_APPLICATION | FONT_GENEVA, 9) => {
            Some((include_bytes!("fontstruct/geneva-9.ttf"), 16.0))
        }
        (FONT_CHICAGO, 12) => Some((include_bytes!("chicago-kare/ChicagoKare-Regular.ttf"), 16.0)),
        _ => None,
    }
}
