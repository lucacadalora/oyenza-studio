// Oyen Poni's Glass theme: the island as liquid glass.
//
// One triangle covers the island's canvas. Each pixel works out how far
// inside the island's outline it is (the outline gfx::notch_segments draws:
// flat top on the screen's edge, concave shoulders, rounded bottom corners),
// then:
//   rim     bends the screen copy inward, like a convex lens: the edge
//           magnifies what's under it (poni_core::glass::rim_offset)
//   middle  shows the copy frosted (two blur levels, mixed by thickness)
//   tint    deepens toward black (dark glass) or white (light glass) only as
//           far as legible text needs (chosen on the CPU)
//   light   runs along the rim, brightest where the edge faces the light
//   shadow  falls under the island, deeper over busy backdrops
// Without a screen copy the body is the look's ground ("smoked glass"),
// see-through so the screen still shows under it, with the same rim light
// and shadow. Output is premultiplied BGRA.

Texture2D sharpTex : register(t0);
Texture2D frostLo : register(t1);
Texture2D frostHi : register(t2);
SamplerState samp : register(s0);

cbuffer Glass : register(b0)
{
    float4 body;      // left, top, right, bottom (pixels)
    float4 shape;     // corner radius, shoulder, pixels per DIP, 1 = sample the screen copy
    float4 lens;      // bevel (px), mag (px), rim power, frost mix (0 thin .. 1 thick)
    float4 tintDark;  // rgb: smoked dark ground; a: tint toward black
    float4 tintLight; // rgb: smoked light ground; a: tint toward white
    float4 light;     // xy: light direction; z: rim strength; w: glow strength
    float4 shadow;    // alpha, reach (px), drop (px), light mix (0 dark .. 1 light glass)
    float4 copy;      // copy width, height (px), saturation, dispersion
    float4 probe;     // first probe pixel x, 1 = draw the probe, lens strength, smoked glass opacity
    float4 origin;    // xy: where the copy's top-left is, in this window's pixels
};

struct VsOut
{
    float4 pos : SV_Position;
};

VsOut vs_main(uint id : SV_VertexID)
{
    VsOut o;
    float2 uv = float2((id << 1) & 2, id & 2);
    o.pos = float4(uv * float2(2.0, -2.0) + float2(-1.0, 1.0), 0.0, 1.0);
    return o;
}

// Distance (px) from p to the island's edge, positive inside. The top is the
// screen's edge, not glass, so it has no rim.
float inside(float2 p)
{
    float x0 = body.x, y0 = body.y, x1 = body.z, y1 = body.w;
    float r = shape.x, s = shape.y;
    if (p.y > y1 - r && p.x < x0 + r)
        return r - length(p - float2(x0 + r, y1 - r));
    if (p.y > y1 - r && p.x > x1 - r)
        return r - length(p - float2(x1 - r, y1 - r));
    float d = y1 - p.y;
    // Beside the body near the top, the edge is the shoulder's concave arc;
    // past the shoulder's tip there is no island at all.
    float dl = p.x - x0;
    float dr = x1 - p.x;
    if (p.y < y0 + s)
    {
        dl = p.x <= x0 - s ? -length(p - float2(x0 - s, y0)) : length(p - float2(x0 - s, y0 + s)) - s;
        dr = p.x >= x1 + s ? -length(p - float2(x1 + s, y0)) : length(p - float2(x1 + s, y0 + s)) - s;
    }
    return min(d, min(dl, dr));
}

// The probe: four pixels of the top row, all the glass colour of the first
// one, nudged by these steps (1/255 each). Too small to see; exact enough that
// if they ever turn up in the screen copy, Windows put the island into its
// own copy (glass.rs `probe_in`).
float3 probeStep(int k)
{
    if (k == 1) return float3(2.0, -1.0, 1.0);
    if (k == 2) return float3(-1.0, 2.0, -1.0);
    if (k == 3) return float3(1.0, 1.0, -2.0);
    return float3(0.0, 0.0, 0.0);
}

float4 ps_main(VsOut i) : SV_Target
{
    float2 p = i.pos.xy;
    int k = -1;
    if (probe.y > 0.5 && p.y < 1.0 && p.x >= probe.x && p.x < probe.x + 4.0)
    {
        k = (int)(p.x - probe.x);
        p.x = probe.x + 0.5;
    }
    float d = inside(p);
    float cov = saturate(d + 0.5);

    // Shadow: the outline dropped a little, fading out over `reach`.
    float ds = inside(p - float2(0.0, shadow.z));
    float fall = 1.0 - saturate(-ds / max(shadow.y, 1.0));
    float sh = shadow.x * (ds >= 0.0 ? 1.0 : fall * fall);
    if (cov <= 0.0)
        return float4(0.0, 0.0, 0.0, sh);

    float2 g = float2(inside(p + float2(1.0, 0.0)) - inside(p - float2(1.0, 0.0)), inside(p + float2(0.0, 1.0)) - inside(p - float2(0.0, 1.0)));
    float gl = length(g);
    float2 nin = gl > 1e-4 ? g / gl : float2(0.0, -1.0);
    float x = saturate(d / max(lens.x, 1.0));
    float m = lens.y * pow(1.0 - x, lens.z) * probe.z;
    float w = pow(1.0 - x, 1.3);
    float mixL = shadow.w;

    float3 col;
    float bodyA = 1.0;
    if (shape.w > 0.5)
    {
        float2 inv = 1.0 / copy.xy;
        float2 o = nin * m;
        float2 q = p - origin.xy;
        float disp = copy.w;
        float3 sharp = float3(
            sharpTex.Sample(samp, (q + o * (1.0 - disp)) * inv).r,
            sharpTex.Sample(samp, (q + o) * inv).g,
            sharpTex.Sample(samp, (q + o * (1.0 + disp)) * inv).b);
        float2 uv = (q + o) * inv;
        float3 frost = lerp(frostLo.Sample(samp, uv).rgb, frostHi.Sample(samp, uv).rgb, lens.w);
        float3 mat = lerp(frost, sharp, w);
        float l = dot(mat, float3(0.2126, 0.7152, 0.0722));
        mat = saturate(l + (mat - l) * copy.z);
        mat = saturate(mat * (1.0 + 0.10 * w));
        float3 dark = mat * (1.0 - tintDark.a);
        float3 lite = lerp(mat, float3(0.985, 0.985, 0.985), tintLight.a);
        col = lerp(dark, lite, mixL);
    }
    else
    {
        // A little brighter toward the top, as light falls on glass.
        col = lerp(tintDark.rgb, tintLight.rgb, mixL);
        float sheen = saturate(1.0 - (p.y - body.y) / max(body.w - body.y, 1.0));
        col = lerp(col, float3(1.0, 1.0, 1.0), 0.07 * sheen);
        bodyA = probe.w;
    }

    // Light along the rim (the lit side, and the inner reflection opposite)
    // and a soft glow just inside the edge.
    float2 nout = -nin;
    float dl = dot(nout, light.xy);
    float facing = pow(max(dl, 0.0), 2.0) + 0.85 * pow(max(-dl, 0.0), 2.0);
    float px = shape.z;
    float rim = saturate(1.0 - (d - 1.0) / (1.7 * px)) * (0.30 + 0.70 * facing) * light.z;
    float glow = exp(-max(d - 1.0, 0.0) / (lerp(4.0, 7.0, lens.w) * px)) * light.w;
    float la = saturate(rim + glow);

    if (k >= 0)
    {
        // Snap to whole 8-bit steps first, so each nudge lands exactly.
        col = col * (1.0 - la) + la;
        col = (round(saturate(col) * 255.0) + probeStep(k)) / 255.0;
        return float4(col, 1.0);
    }
    // The body (see-through when smoked), then the light on top of it.
    float a = (bodyA * (1.0 - la) + la) * cov;
    float3 pm = (col * bodyA * (1.0 - la) + la) * cov;
    return float4(pm, a + sh * (1.0 - a));
}
