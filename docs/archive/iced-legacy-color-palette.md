# Iced Legacy Color Palette (pre-Vizia reference)

Extracted via `git show <commit>:plugins/<plugin>/src/editor.rs` from last iced-era commit per plugin. Use as reference when re-skinning shared-ui / plugin-specific views.

## Shared base (identical across ALL plugins — this is the LX Audiolabs brand, not per-plugin)

| Role | Color (r,g,b[,a]) | Notes |
|---|---|---|
| Panel/window bg | `0.06–0.10, same, same` | darkest greys, panels/footers |
| Border | `0.15–0.3, same, same` | container borders |
| Text primary | `WHITE` / `0.8–0.85` | value readouts, headers |
| Text secondary | `0.55–0.6, same, same` | section labels (e.g. "GONIOMETER") |
| Text dim | `0.4, same, same` | axis labels, disabled |
| Accent (hover/active bg) | `0.25, 0.25, 0.25` | button hover |
| **Accent amber** | `1.0, 0.45, 0.1` | primary brand accent — active/selected buttons, knob/slider fill, active toggle |
| Accent amber (labels) | `1.0, 0.55, 0.15` | section header labels (e.g. "MONO FLOOR", "INFLATE", "COMPRESSOR") |
| Accent amber (values) | `1.0, 0.65, 0.3` | value display text |
| Accent amber (blink/snap) | `1.0, 0.85, 0.3` | SNAP blink state |

This part is already correctly carried into `shared-ui/src/widgets.rs` and `canvas.rs` — knob/slider colors match exactly.

## Per-plugin distinctive colors (data-viz / unique panels — verify these survived migration)

### Lucent
- Masking contributor palette (6 colors, cycled): `(1.0,0.6,0.2)`, `(0.8,0.3,0.3)`, `(0.3,0.8,0.5)`, `(0.4,0.6,1.0)`, `(0.9,0.7,0.3)`, `(0.7,0.4,0.8)`
- Relay/heartbeat teal: `(0.1, 0.9, 0.7)`
- Resonance error red: `(0.95, 0.22, 0.18)`

### Lucent-Relay
- Panel bg slightly blue-tinted: `(0.08, 0.08, 0.10)` (not neutral grey like others)
- Connected: `(0.2, 0.9, 0.3)` / Disconnected: `(0.9, 0.2, 0.2)`

### Equilibrium
- Goniometer dot: correlation-based green/amber/red `(0.0,0.75,0.3)` / `(1.0,0.55,0.1)` / `(1.0,0.25,0.25)`
  - **Fixed 2026-07-05**: shared-ui had drifted to `(0.0,0.85,0.35)` / `(1.0,0.45,0.1)` / `(0.9,0.2,0.2)`, restored exact values in [shared-ui/src/canvas.rs:222-228](../shared-ui/src/canvas.rs)
- Spectrum bar highlight: `rgba(1.0,0.45,0.1,bar_alpha)`
- Solo/Listen active button bg: amber `(1.0,0.45,0.1)` (same as global accent, confirms Listen buttons should look "active/amber" not generic hover-grey)

### Meridian
- Gain-reduction meter: red `(1.0,0.3,0.3)`, peak-hold orange `(1.0,0.6,0.2)`
- EQ freq value tint (cool contrast to amber): `(0.7, 0.85, 1.0)`
- Compressor envelope fill: `rgba(1.0,0.35,0.15,0.18)`, line `(1.0,0.4,0.2)`
- Teal accent (same as Lucent relay heartbeat): `(0.1, 0.9, 0.7)`

### Aurum
- `AMBER` const reused everywhere (= `1.0,0.45,0.1` global accent)
- Tab-active bg (SHAPE/COLOR/LIMIT selection): `(0.25, 0.15, 0.05)` — warm dark amber-tinted, distinct from generic `(0.15,0.15,0.15)` inactive

### Aether
- 1px section separators tinted teal-grey `(0.12, 0.16, 0.16)` / `(0.18, 0.22, 0.22)` instead of neutral dark grey — thin divider lines, not panel backgrounds
- EQ curve line: `rgba(1.0, 0.6, 0.1, 0.85)`

**Verified 2026-07-05:** Aether separator carried over correctly — [editor.rs:592-594](../plugins/aether/src/editor.rs) uses `col(0.12,0.16,0.16,1.0)`, exact match. No regression here.

## Next step (fix phase)

Compare each item above against current `plugins/*/src/editor.rs` + plugin-specific canvas views (`aether_canvas.rs`, Equilibrium `EqSpectrumView`, Meridian `CompressorEnvelopeView`) to find what's missing/generic now.
