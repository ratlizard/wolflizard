#include <metal_stdlib>

using namespace metal;

struct RasterVertex {
    float4 position [[position]];
    float2 tex_coord;
};

vertex RasterVertex raster_vertex(uint vertex_id [[vertex_id]]) {
    constexpr float2 positions[] = {
        {-1.0,  1.0},
        {-1.0, -1.0},
        { 1.0,  1.0},
        { 1.0, -1.0},
    };
    constexpr float2 tex_coords[] = {
        {0.0, 0.0},
        {0.0, 1.0},
        {1.0, 0.0},
        {1.0, 1.0},
    };

    RasterVertex out;
    out.position = float4(positions[vertex_id], 0.0, 1.0);
    out.tex_coord = tex_coords[vertex_id];
    return out;
}

// Where to sample, in texels, so that an enlargement keeps every source pixel
// a solid block and blends only the one drawable pixel that a block's edge
// crosses ("sharp bilinear"). The window scales continuously, so the factor is
// rarely whole: nearest sampling then draws source columns alternately n and
// n+1 drawable pixels wide, which makes one-pixel text strokes uneven, and
// plain bilinear filtering blurs every edge instead. `footprint` is source
// texels per drawable pixel. At a whole-number factor every drawable pixel
// centre falls inside a block, and the result is nearest sampling exactly.
static float2 sharp_texel(float2 texel, float2 footprint) {
    float2 scale = 1.0 / max(footprint, float2(1.0e-6));
    float2 offset = fract(texel) - 0.5;
    float2 solid = max(0.5 - 0.5 / scale, float2(0.0));
    return floor(texel) + 0.5 + (offset - clamp(offset, -solid, solid)) * scale;
}

fragment float4 raster_fragment(
    RasterVertex in [[stage_in]],
    texture2d<float> framebuffer [[texture(0)]])
{
    constexpr sampler linear_sampler(
        coord::normalized,
        address::clamp_to_edge,
        filter::linear);
    float2 dimensions = float2(framebuffer.get_width(), framebuffer.get_height());
    float2 footprint = abs(float2(dfdx(in.tex_coord.x), dfdy(in.tex_coord.y))) * dimensions;
    if (all(footprint <= 1.0)) {
        float2 texel = sharp_texel(in.tex_coord * dimensions, footprint);
        return framebuffer.sample(linear_sampler, texel / dimensions);
    }

    // The outline surface is larger than the drawable. Integrate each source
    // texel's covered area: nearest sampling drops thin strokes, and bilinear
    // sampling still skips texels when shrinking by more than two times.
    footprint = max(footprint, float2(1.0));
    float2 center = in.tex_coord * dimensions;
    float2 lower = center - footprint * 0.5;
    float2 upper = center + footprint * 0.5;
    int2 first = int2(floor(lower));
    int2 last = int2(ceil(upper));
    float4 color = float4(0.0);
    for (int y = first.y; y < last.y; ++y) {
        float wy = min(upper.y, float(y + 1)) - max(lower.y, float(y));
        for (int x = first.x; x < last.x; ++x) {
            float wx = min(upper.x, float(x + 1)) - max(lower.x, float(x));
            uint2 texel = uint2(clamp(int2(x, y), int2(0), int2(dimensions) - 1));
            color += framebuffer.read(texel) * (wx * wy);
        }
    }
    return color / (footprint.x * footprint.y);
}

struct GuestFrameUniforms {
    uint row_bytes;
    uint width;
    uint height;
    uint pixel_size;
    uint content_left;
    uint content_top;
    uint cursor_kind;
    uint cursor_width;
    uint cursor_height;
    int cursor_left;
    int cursor_top;
};

struct GuestCursorData {
    uint data_rows[16];
    uint mask_rows[16];
    uint color_pixels[256];
};

static float4 unpack_argb(uint argb) {
    return float4(
        float((argb >> 16) & 0xFF) / 255.0,
        float((argb >> 8) & 0xFF) / 255.0,
        float(argb & 0xFF) / 255.0,
        1.0);
}

static uint guest_argb(
    uint x,
    uint y,
    device const uchar* framebuffer,
    constant uint* palette,
    constant GuestFrameUniforms& frame,
    constant GuestCursorData& cursor)
{
    uint argb;

    if (frame.pixel_size == 8) {
        uint index = framebuffer[y * frame.row_bytes + x];
        argb = palette[index];
    } else if (frame.pixel_size == 4) {
        uchar packed = framebuffer[y * frame.row_bytes + x / 2];
        uint index = (x & 1) == 0 ? packed >> 4 : packed & 0x0F;
        argb = palette[index];
    } else if (frame.pixel_size == 2) {
        uchar packed = framebuffer[y * frame.row_bytes + x / 4];
        uint shift = 6 - 2 * (x & 3);
        uint index = (packed >> shift) & 0x03;
        argb = palette[index];
    } else {
        uchar packed = framebuffer[y * frame.row_bytes + x / 8];
        uint index = (packed >> (7 - (x & 7))) & 0x01;
        argb = palette[index];
    }

    int cursor_x = int(x) - frame.cursor_left;
    int cursor_y = int(y) - frame.cursor_top;
    if (frame.cursor_kind != 0
        && cursor_x >= 0 && cursor_y >= 0
        && cursor_x < int(frame.cursor_width)
        && cursor_y < int(frame.cursor_height)) {
        uint row = uint(cursor_y);
        uint column = uint(cursor_x);
        uint bit = column < 16 ? (0x8000 >> column) : 0;
        bool mask_set = row < 16 && column < 16 && (cursor.mask_rows[row] & bit) != 0;

        if (frame.cursor_kind == 1) {
            if (mask_set) {
                argb = (cursor.data_rows[row] & bit) != 0 ? 0xFF000000 : 0xFFFFFFFF;
            }
        } else {
            uint cursor_argb = cursor.color_pixels[row * frame.cursor_width + column];
            if (mask_set) {
                argb = cursor_argb | 0xFF000000;
            } else if (cursor_argb == 0xFF000000) {
                argb ^= 0x00FFFFFF;
            }
        }
    }

    return argb;
}

fragment float4 guest_raster_fragment(
    RasterVertex in [[stage_in]],
    device const uchar* framebuffer [[buffer(0)]],
    constant uint* palette [[buffer(1)]],
    constant GuestFrameUniforms& frame [[buffer(2)]],
    constant GuestCursorData& cursor [[buffer(3)]])
{
    // The same sharp enlargement as raster_fragment, filtering by hand
    // because the guest's pixels arrive as indexed bytes, not a texture.
    float2 dimensions = float2(frame.width, frame.height);
    float2 footprint = abs(float2(dfdx(in.tex_coord.x), dfdy(in.tex_coord.y))) * dimensions;
    float2 texel = sharp_texel(in.tex_coord * dimensions, footprint) - 0.5;
    float2 cell = floor(texel);
    float2 weight = texel - cell;
    int2 last = int2(frame.width, frame.height) - 1;
    uint2 near = uint2(clamp(int2(cell), int2(0), last));
    uint2 far = uint2(clamp(int2(cell) + 1, int2(0), last));
    uint2 origin = uint2(frame.content_left, frame.content_top);
    float4 top = mix(
        unpack_argb(guest_argb(origin.x + near.x, origin.y + near.y, framebuffer, palette, frame, cursor)),
        unpack_argb(guest_argb(origin.x + far.x, origin.y + near.y, framebuffer, palette, frame, cursor)),
        weight.x);
    float4 bottom = mix(
        unpack_argb(guest_argb(origin.x + near.x, origin.y + far.y, framebuffer, palette, frame, cursor)),
        unpack_argb(guest_argb(origin.x + far.x, origin.y + far.y, framebuffer, palette, frame, cursor)),
        weight.x);
    return mix(top, bottom, weight.y);
}

// Integer area coverage over a compact retained surface, matching
// d3d_present.hlsl and resize_argb_coverage on the CPU: no bilinear filtering
// or sRGB conversion, one exact quantization per channel.
struct CompactFrameUniforms {
    uint width;
    uint height;
    uint scale;
    uint crop_left;
    uint crop_top;
    uint crop_width;
    uint crop_height;
    uint dw;
    uint dh;
    uint ox;
    uint oy;
};

static uint compact_sample_rgb(
    device const uint* cells,
    device const uint* detail,
    constant CompactFrameUniforms& frame,
    uint x,
    uint y)
{
    uint scale = frame.scale;
    x += frame.crop_left * scale;
    y += frame.crop_top * scale;
    uint cell = cells[(y / scale) * frame.width + x / scale];
    return (cell & 0x80000000) != 0
        ? detail[(cell & 0x7fffffff) + (y % scale) * scale + x % scale]
        : cell;
}

fragment float4 compact_fragment(
    RasterVertex in [[stage_in]],
    device const uint* cells [[buffer(0)]],
    device const uint* detail [[buffer(1)]],
    constant CompactFrameUniforms& frame [[buffer(2)]])
{
    uint x = uint(in.position.x) - frame.ox;
    uint y = uint(in.position.y) - frame.oy;
    uint sw = frame.crop_width * frame.scale;
    uint sh = frame.crop_height * frame.scale;
    uint dw = frame.dw;
    uint dh = frame.dh;
    bool shrink_x = sw > dw;
    bool shrink_y = sh > dh;
    uint left = x * sw;
    uint right = (x + 1) * sw;
    uint top = y * sh;
    uint bottom = (y + 1) * sh;
    uint x0 = shrink_x ? left / dw : min((2 * x + 1) * sw / (2 * dw), sw - 1);
    uint x1 = shrink_x ? (right + dw - 1) / dw : x0 + 1;
    uint y0 = shrink_y ? top / dh : min((2 * y + 1) * sh / (2 * dh), sh - 1);
    uint y1 = shrink_y ? (bottom + dh - 1) / dh : y0 + 1;
    uint3 sum = uint3(0);
    for (uint sy = y0; sy < y1; sy++) {
        uint wy = shrink_y ? min(bottom, (sy + 1) * dh) - max(top, sy * dh) : 1;
        for (uint sx = x0; sx < x1; sx++) {
            uint wx = shrink_x ? min(right, (sx + 1) * dw) - max(left, sx * dw) : 1;
            uint rgb = compact_sample_rgb(cells, detail, frame, sx, sy);
            sum += ((uint3(rgb) >> uint3(16, 8, 0)) & 255) * (wx * wy);
        }
    }
    uint total = (shrink_x ? sw : 1) * (shrink_y ? sh : 1);
    uint3 result = (sum + total / 2) / total;
    return float4(float3(result) / 255.0, 1.0);
}
