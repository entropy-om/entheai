// rain_on_glass.glsl — Ghostty custom-shader for entheai
// Based on ldSBWW by Élie Michel (CC BY 3.0)
// Adapted for low-contrast terminal use, then made TEXT-AWARE: the rain only
// refracts the empty background — glyphs and a small margin around them stay
// crisp (no smoke over the text).

float luma(vec3 c) { return dot(c, vec3(0.299, 0.587, 0.114)); }

void mainImage(out vec4 fragColor, in vec2 fragCoord) {
    vec2 u = fragCoord.xy / iResolution.xy;
    vec2 px = 1.0 / iResolution.xy;  // one pixel in UV

    // Start with the terminal content (text stays exactly as-is here).
    fragColor = texture(iChannel0, u);

    // --- Text-aware clear zone --------------------------------------------
    // Text is brighter than the dark terminal void. Take the MAX luminance over
    // a small ring so the mask DILATES around glyphs: the rain is suppressed on
    // the text AND a ~margin-radius halo next to it. Empty areas stay rainy.
    float m = luma(fragColor.rgb);
    const float R = 4.0;  // clear-margin radius, in pixels
    for (int i = 0; i < 8; i++) {
        float a = float(i) * 0.7853982;  // 8 directions
        m = max(m, luma(texture(iChannel0, u + vec2(cos(a), sin(a)) * R * px).rgb));
    }
    // 1 over text + its margin, 0 in the empty void.
    float clearZone = smoothstep(0.04, 0.14, m);

    // Coarse displacement noise sampled from the terminal.
    vec2 n = texture(iChannel0, u * .1).rg;

    // Three scale passes: r=3 (many small drops), r=2, r=1 (few large drops).
    for (float r = 3.; r > 0.; r--) {
        vec2 x = iResolution.xy * r * .015;
        vec2 p = 6.28 * u * x + (n - .5) * 2.;
        vec2 s = sin(p);
        vec4 d = texture(iChannel0, round(u * x - .25) / x);
        float t = (s.x + s.y) * max(0., 1. - fract(iTime * (d.b + .1) + d.g) * 2.);

        if (d.r < (5. - r) * .08 && t > .5) {
            vec3 v = normalize(-vec3(cos(p), mix(.2, 2., t - .5)));
            vec4 refracted = texture(iChannel0, u - v.xy * 0.08);
            // Refract the void only — fully cleared over text + margin.
            fragColor = mix(fragColor, refracted, 0.3 * (1.0 - clearZone));
        }
    }

    // Subtle darkening in the empty void only (never touches lit text).
    float lum = luma(fragColor.rgb);
    float darkMask = (1.0 - smoothstep(0.03, 0.18, lum)) * (1.0 - clearZone);
    fragColor = mix(fragColor, fragColor * 0.97, darkMask);
}
