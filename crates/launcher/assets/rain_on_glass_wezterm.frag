// rain_on_glass_wezterm.frag — WezTerm port of the entheai rain-on-glass effect.
// The Ghostty original lives in rain_on_glass.glsl (Shadertoy-style mainImage +
// iChannel0); WezTerm's shader contract is different — each cell is drawn as a
// quad with the glyph texture, so this adapts the same text-aware intent: rain
// animates over the empty background and is suppressed where glyph coverage
// (text) is present.
//
// WezTerm-provided uniforms: pixel_coord, text_cell_dim, viewport_dim,
// glyph_scale, time, glyph_texture. Output: `ocolor` (keep the glyph alpha so
// the terminal background still shows through). If the shader fails to compile,
// WezTerm falls back to normal rendering — the window still opens.

#version 150

uniform vec2 pixel_coord;
uniform vec2 text_cell_dim;
uniform vec2 viewport_dim;
uniform vec2 glyph_scale;
uniform float time;
uniform sampler2D glyph_texture;

out vec4 ocolor;

float hash(vec2 p) {
    return fract(sin(dot(p, vec2(127.1, 311.7))) * 43758.5453123);
}

void main() {
    // The current cell's glyph bitmap, mapped by glyph_scale so pixel_coord
    // lands on the glyph texels.
    vec4 glyph = texture(glyph_texture, pixel_coord * glyph_scale);
    float coverage = glyph.a;

    // Text-aware clear zone: rain is suppressed over glyphs + a small halo.
    float clearZone = smoothstep(0.04, 0.14, coverage);

    // Rain: droplets drift down a coarse grid; a per-cell hash scatters which
    // cells rain at any moment.
    vec2 cell = floor(pixel_coord / vec2(text_cell_dim.x * 4.0, 24.0));
    float band = hash(cell + floor(time * 2.0) * 0.137);
    float streak = fract(pixel_coord.y / 24.0 - time * (1.5 + band * 2.0));
    streak = smoothstep(0.0, 0.1, streak) * (1.0 - smoothstep(0.5, 1.0, streak));
    float rain = step(0.93, band) * streak;

    // In the void only: subtle darkening + droplet ripple; text stays crisp.
    float effect = rain * (1.0 - clearZone);
    vec3 color = glyph.rgb * (1.0 - 0.12 * effect);

    ocolor = vec4(color, coverage);
}
