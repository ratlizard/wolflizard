// grimoire's undither filter (js/delv-graphics.js, `UD`) on the presented
// frame, for the art Ambrosia dithered down to 256 colours.
//
// This file is compiled appended to metal_present.metal, whose vertex shader,
// `unpack_argb` and `guest_argb` it uses. It is the browser player's version
// of the filter (ratlizard.github.io, www/index.html, `makeUndither`) moved
// from WebGL to Metal with the same settings, the same four passes and the
// same two departures from grimoire:
//
//   1  mean    a 3x3 box mean of the frame
//   2  detect  how dithered each pixel is (residuals that alternate along a
//              line through it, three pixels each way), and the
//              checkerboard-notch blend by that amount
//   3  detail  re-sharpen what the blend softened
//   4  super   grimoire's 2x structure-guided upscale averaged straight back
//              down to one pixel, pulled back toward the pixel before this
//              pass by 1 - d * strength, so a pixel the detector left alone
//              comes out exactly as it went in
//
// The departures: stray-colour repair is left out, because on the live screen
// it scatters specks over the conversation window's patterned background; and
// blends and averages are taken in light (the sRGB transfer curve) when
// `light` is set, while detection stays on stored values where grimoire's
// thresholds were tuned, because averaging stored values darkens a dithered
// area -- the conversation window's blue by 40%.
//
// One thing the browser does not need is added. The desktop window draws text
// at the drawable's scale over the guest's pixels, and draws the cursor over
// them; neither is part of the picture. Each pixel of the filter's source
// carries a class in its alpha: an overlay pixel is treated as lying outside
// the frame by every pass and is shown as drawn; a locked pixel is one of the
// colours the game animates (palette indices 0xE0 to 0xFB), which grimoire
// gives no blend and never blends across; everything else is picture.

struct UnditherParams {
    float sens;
    float strength;
    float detail;
    float edge;
    uint light;
    uint scale;
};

constant uint UD_OVERLAY = 0;
constant uint UD_LOCKED = 1;
constant uint UD_PICTURE = 2;

static uint ud_class(float4 source) {
    return uint(source.a * 2.0 + 0.5);
}

static float ud_class_alpha(uint cls) {
    return float(cls) * 0.5;
}

static bool ud_animated(uint index) {
    return index >= 0xE0 && index < 0xFC;
}

static bool ud_inside(int2 q, int2 size) {
    return all(q >= 0) && all(q < size);
}

// An overlay neighbour is left out of every count, as an out-of-frame one is.
static bool ud_usable(texture2d<float> source, int2 q, int2 size) {
    return ud_inside(q, size) && ud_class(source.read(uint2(q))) != UD_OVERLAY;
}

static float ud_lin(float v) {
    return v <= 0.04045 ? v / 12.92 : precise::pow((v + 0.055) / 1.055, 2.4);
}

static float3 ud_to_light(float3 c) {
    return float3(ud_lin(c.r), ud_lin(c.g), ud_lin(c.b));
}

static float ud_enc(float v) {
    v = clamp(v, 0.0, 1.0);
    return v <= 0.0031308 ? v * 12.92 : 1.055 * precise::pow(v, 1.0 / 2.4) - 0.055;
}

static float3 ud_to_stored(float3 l) {
    return float3(ud_enc(l.r), ud_enc(l.g), ud_enc(l.b));
}

// The source from a native guest frame: the guest's colour with the cursor
// composited, as guest_raster_fragment draws it. A pixel the cursor changed is
// an overlay. Eight-bit screens only; the presenter does not ask otherwise.
fragment float4 undither_source_guest(
    RasterVertex in [[stage_in]],
    device const uchar* framebuffer [[buffer(0)]],
    constant uint* palette [[buffer(1)]],
    constant GuestFrameUniforms& frame [[buffer(2)]],
    constant GuestCursorData& cursor [[buffer(3)]])
{
    uint2 p = uint2(in.position.xy);
    uint x = frame.content_left + p.x;
    uint y = frame.content_top + p.y;
    uint argb = guest_argb(x, y, framebuffer, palette, frame, cursor);
    uint index = framebuffer[y * frame.row_bytes + x];
    uint cls = argb != palette[index] ? UD_OVERLAY
        : ud_animated(index) ? UD_LOCKED : UD_PICTURE;
    return float4(unpack_argb(argb).rgb, ud_class_alpha(cls));
}

// The source from a raster presented at `scale` presented pixels per guest
// pixel, and the guest's indices under it. A guest pixel whose block is one
// colour, and that colour its palette entry, is the guest's pixel; any other
// block has text or the cursor drawn in it and is an overlay.
fragment float4 undither_source_presented(
    RasterVertex in [[stage_in]],
    texture2d<float> presented [[texture(0)]],
    device const uchar* indices [[buffer(0)]],
    constant uint* palette [[buffer(1)]],
    constant UnditherParams& params [[buffer(2)]])
{
    uint2 g = uint2(in.position.xy);
    uint width = presented.get_width() / params.scale;
    uint index = indices[g.y * width + g.x];
    uint2 origin = g * params.scale;
    float4 first = presented.read(origin);
    bool uniform_block = true;
    for (uint sy = 0; sy < params.scale; ++sy) {
        for (uint sx = 0; sx < params.scale; ++sx) {
            if (any(presented.read(origin + uint2(sx, sy)).rgb != first.rgb)) {
                uniform_block = false;
            }
        }
    }
    uint argb = palette[index];
    uint3 expected = uint3((argb >> 16) & 0xFF, (argb >> 8) & 0xFF, argb & 0xFF);
    bool guest_colour = all(uint3(round(first.rgb * 255.0)) == expected);
    uint cls = !(uniform_block && guest_colour) ? UD_OVERLAY
        : ud_animated(index) ? UD_LOCKED : UD_PICTURE;
    return float4(first.rgb, ud_class_alpha(cls));
}

fragment float4 undither_mean(
    RasterVertex in [[stage_in]],
    texture2d<float> source [[texture(0)]])
{
    int2 size = int2(source.get_width(), source.get_height());
    int2 p = int2(in.position.xy);
    float3 sum = float3(0.0);
    float count = 0.0;
    for (int dy = -1; dy <= 1; ++dy) {
        for (int dx = -1; dx <= 1; ++dx) {
            int2 q = p + int2(dx, dy);
            if (!ud_usable(source, q, size)) {
                continue;
            }
            sum += source.read(uint2(q)).rgb;
            count += 1.0;
        }
    }
    return float4(count > 0.0 ? sum / count : source.read(uint2(p)).rgb, 1.0);
}

// A dither alternates against its neighbours along a line through it; a drawn
// edge agrees with them. The residual is a pixel less its own neighbourhood's
// mean, and the detector compares residuals along both axes, three pixels each
// way (grimoire's LINE_REACH). The result is the blended colour -- in light
// when `light` is set -- with the amount `d` in alpha for the next two passes.
fragment float4 undither_detect(
    RasterVertex in [[stage_in]],
    texture2d<float> source [[texture(0)]],
    texture2d<float> mean [[texture(1)]],
    constant UnditherParams& params [[buffer(0)]])
{
    int2 size = int2(source.get_width(), source.get_height());
    int2 p = int2(in.position.xy);
    float4 s = source.read(uint2(p));
    uint cls = ud_class(s);
    float3 r0 = (s.rgb - mean.read(uint2(p)).rgb) * 255.0;
    float m = length(r0);
    float d = 0.0;
    if (cls == UD_PICTURE && m >= 0.5) {
        float best = -2.0;
        for (int a = 0; a < 2; ++a) {
            int2 axis = a == 0 ? int2(1, 0) : int2(0, 1);
            float sum = 0.0;
            float count = 0.0;
            for (int k = 1; k <= 3; ++k) {
                for (int sg = -1; sg <= 1; sg += 2) {
                    int2 q = p + axis * (k * sg);
                    if (!ud_usable(source, q, size)) {
                        continue;
                    }
                    count += 1.0;
                    float3 rj = (source.read(uint2(q)).rgb - mean.read(uint2(q)).rgb) * 255.0;
                    float mj = length(rj);
                    if (mj < 0.5) {
                        continue; // a flat neighbour votes neutral
                    }
                    sum += dot(r0, rj) / (m * mj);
                }
            }
            if (count > 0.0) {
                best = max(best, sum / count);
            }
        }
        float coherence = best < -1.0 ? 1.0 : best;
        d = clamp((params.sens - coherence) * 8.0, 0.0, 1.0) * smoothstep(1.5, 5.0, m);
        // A pixel whose whole neighbourhood is its own colour has nothing to undither.
        bool uniform_area = true;
        for (int dy = -1; dy <= 1; ++dy) {
            for (int dx = -1; dx <= 1; ++dx) {
                int2 q = p + int2(dx, dy);
                if ((dx != 0 || dy != 0) && ud_usable(source, q, size)
                    && any(source.read(uint2(q)).rgb != s.rgb)) {
                    uniform_area = false;
                }
            }
        }
        if (uniform_area) {
            d = 0.0;
        }
    }
    // The checkerboard notch: half the pixel, half its four orthogonal
    // neighbours.
    float3 cv = params.light != 0 ? ud_to_light(s.rgb) : s.rgb;
    float3 n4 = float3(0.0);
    float n4_count = 0.0;
    constexpr int2 orthogonal[4] = {{1, 0}, {-1, 0}, {0, 1}, {0, -1}};
    for (int e = 0; e < 4; ++e) {
        int2 q = p + orthogonal[e];
        if (!ud_usable(source, q, size)) {
            continue;
        }
        float3 nv = source.read(uint2(q)).rgb;
        n4 += params.light != 0 ? ud_to_light(nv) : nv;
        n4_count += 1.0;
    }
    float3 filtered = n4_count > 0.0 ? 0.5 * cv + 0.5 * (n4 / n4_count) : cv;
    return float4(cv + d * params.strength * (filtered - cv), d);
}

fragment float4 undither_detail(
    RasterVertex in [[stage_in]],
    texture2d<float> source [[texture(0)]],
    texture2d<float> detected [[texture(1)]],
    constant UnditherParams& params [[buffer(0)]])
{
    int2 size = int2(source.get_width(), source.get_height());
    int2 p = int2(in.position.xy);
    float4 self = detected.read(uint2(p));
    float3 sum = float3(0.0);
    float count = 0.0;
    for (int dy = -1; dy <= 1; ++dy) {
        for (int dx = -1; dx <= 1; ++dx) {
            int2 q = p + int2(dx, dy);
            if (!ud_usable(source, q, size)) {
                continue;
            }
            sum += detected.read(uint2(q)).rgb;
            count += 1.0;
        }
    }
    if (count == 0.0) {
        return self;
    }
    float w = self.a * params.strength * params.detail;
    return float4(self.rgb + w * (self.rgb - sum / count), self.a);
}

// grimoire's guidedUpscale at 2x and the supersample that follows it, per
// output pixel: each of four sub-samples, a quarter pixel off centre each way,
// is a 3x3 average weighted by distance and by colour difference (the edge
// threshold), never across the lock's boundary; the four are averaged.
fragment float4 undither_super(
    RasterVertex in [[stage_in]],
    texture2d<float> source [[texture(0)]],
    texture2d<float> detailed [[texture(1)]],
    constant UnditherParams& params [[buffer(0)]])
{
    int2 size = int2(source.get_width(), source.get_height());
    int2 p = int2(in.position.xy);
    float4 s = source.read(uint2(p));
    uint cls = ud_class(s);
    if (cls == UD_OVERLAY) {
        return float4(s.rgb, 1.0);
    }
    float4 self = detailed.read(uint2(p));
    float3 cv = self.rgb; // light, or stored when `light` is off
    float3 cs = params.light != 0 ? ud_to_stored(cv) : cv; // stored, for the edge weights
    bool base_locked = cls == UD_LOCKED;
    float inv_sr = 1.0 / (2.0 * params.edge * params.edge);
    constexpr float2 offsets[4] = {{-0.25, -0.25}, {0.25, -0.25}, {-0.25, 0.25}, {0.25, 0.25}};
    float3 acc[4] = {float3(0.0), float3(0.0), float3(0.0), float3(0.0)};
    float ws[4] = {0.0, 0.0, 0.0, 0.0};
    for (int dy = -1; dy <= 1; ++dy) {
        for (int dx = -1; dx <= 1; ++dx) {
            int2 q = p + int2(dx, dy);
            if (!ud_inside(q, size)) {
                continue;
            }
            uint qc = ud_class(source.read(uint2(q)));
            if (qc == UD_OVERLAY || (qc == UD_LOCKED) != base_locked) {
                continue; // never blend across structure
            }
            float3 nv = detailed.read(uint2(q)).rgb;
            float3 ns = params.light != 0 ? ud_to_stored(nv) : nv;
            float3 dc = (ns - cs) * 255.0;
            float rw = precise::exp(-dot(dc, dc) * inv_sr);
            for (int i = 0; i < 4; ++i) {
                float2 o = float2(dx, dy) - offsets[i];
                float w = precise::exp(-dot(o, o) * 2.0) * rw;
                acc[i] += nv * w;
                ws[i] += w;
            }
        }
    }
    float3 up = float3(0.0);
    for (int i = 0; i < 4; ++i) {
        up += ws[i] > 0.0 ? acc[i] / ws[i] : cv;
    }
    up *= 0.25;
    float w = min(1.0, self.a * params.strength);
    float3 result = up + (1.0 - w) * (cv - up);
    return float4(params.light != 0 ? ud_to_stored(result) : result, 1.0);
}

// Back to the presented raster's size: an overlay pixel keeps its presented
// block, and every other guest pixel becomes a block of its filtered colour.
fragment float4 undither_compose(
    RasterVertex in [[stage_in]],
    texture2d<float> presented [[texture(0)]],
    texture2d<float> source [[texture(1)]],
    texture2d<float> filtered [[texture(2)]],
    constant UnditherParams& params [[buffer(0)]])
{
    uint2 p = uint2(in.position.xy);
    uint2 g = min(p / params.scale, uint2(source.get_width(), source.get_height()) - 1);
    if (ud_class(source.read(g)) == UD_OVERLAY) {
        return presented.read(p);
    }
    return filtered.read(g);
}
