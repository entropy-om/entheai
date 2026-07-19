// rain_on_glass.glsl — Ghostty custom-shader for entheai
// Based on ldSBWW by Élie Michel (CC BY 3.0)
// Adapted for low-contrast terminal use (refraction 0.3->0.08)

void mainImage(out vec4 fragColor, in vec2 fragCoord) {
    vec2 u = fragCoord.xy / iResolution.xy;

    // Sample terminal texture at coarse scale as displacement noise
    vec2 n = texture(iChannel0, u * .1).rg;

    // Start with terminal content (text remains as-is in non-drop pixels)
    fragColor = texture(iChannel0, u);

    // Three scale passes: r=3 (many small drops), r=2, r=1 (few large drops)
    for (float r = 3.; r > 0.; r--) {
        vec2 x = iResolution.xy * r * .015;  // grid dimensions
        vec2 p = 6.28 * u * x + (n - .5) * 2.;  // UV modulation
        vec2 s = sin(p);
        // Quantized noise lookup — consistent properties per grid cell
        vec4 d = texture(iChannel0, round(u * x - .25) / x);
        // Drop lifecycle: sine magnitude x fade envelope
        float t = (s.x + s.y) * max(0., 1. - fract(iTime * (d.b + .1) + d.g) * 2.);

        if (d.r < (5. - r) * .08 && t > .5) {
            // Refraction normal: cos gives lateral curvature; z gives depth
            vec3 v = normalize(-vec3(cos(p), mix(.2, 2., t - .5)));
            // Refraction 0.08 (reduced from 0.3) preserves text legibility
            vec4 refracted = texture(iChannel0, u - v.xy * 0.08);
            // Blend: 70% original terminal, 30% refracted
            fragColor = mix(fragColor, refracted, 0.3);
        }
    }

    // Luminance-based masking: subtle darkening in dark/empty areas only
    float lum = dot(fragColor.rgb, vec3(0.299, 0.587, 0.114));
    float darkMask = 1.0 - smoothstep(0.03, 0.18, lum);
    fragColor = mix(fragColor, fragColor * 0.97, darkMask);
}
