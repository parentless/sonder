# What Sonder's GPUI can and cannot do

Verdicts: ✅ yes · 🟡 yes with a workaround · ❌ no · ❓ not found, unverified.
Source paths: `GP` = `~/.cargo/registry/src/*/gpui-pre-0.3.7/src`. Sonder pins
`gpui-kit 0.7.0` and a patched `vendor/gpui-pre-windows` (only the debug shader
loading differs; see `vendor/gpui-pre-windows/CADENCE_PATCH.md`).
Checked 2026-11-24 unless a row says otherwise.

## Things that work

| Effect | Verdict | Evidence and notes |
|---|---|---|
| Opacity on any element | ✅ | `GP/styled.rs:746`. Applies to the element's children too. |
| Colours, borders, per-corner radii | ✅ | `rounded_tl` etc. Round the element that paints the background. |
| Box shadows (offset, spread, blur) | ✅ | `GP/style.rs:377` `blur_radius`. Shadows only, not blur of content. |
| Linear gradients as a background | ✅ | `linear_gradient`, `GP/color.rs:865`. |
| Hatch pattern | ✅ | `pattern_slash`, `GP/color.rs:827`. |
| Animations on any style property | ✅ | `with_animation` plus easings in `GP/elements/animation.rs`: linear, quadratic, ease_in_out, ease_out_quint, bounce, pulsating_between. |
| Absolute and relative positioning, flex, grid | ✅ | `grid()` exists at `GP/styled.rs:52`; Sonder is flex-based. |
| Custom drawn shapes | ✅ | `canvas` plus `PathBuilder` (fill, stroke, arcs, beziers). Used in `settings/audio.rs` and `components::page_corner`. |
| SVG rotate and scale | ✅ | `with_transformation`, `GP/elements/svg.rs:66`. SVG only. |
| Image grayscale and fit | ✅ | `GP/elements/img.rs:153`. |
| Blur of an image | 🟡 | Not a GPU effect. Sonder blurs on the CPU at 160 px and stretches it (`blur_artist_image`, `src/app/image_cache.rs`). Good for static images only. |
| Text size, weight, colour, ellipsis, line clamp | ✅ | `GP/styled.rs`. |
| Text underline (solid, wavy), per-range colour and weight | ✅ | `text_decoration_*`; `StyledText::with_highlights` (used for search highlighting). |
| Virtualised long lists | ✅ | `uniform_list`. |
| Overlays and menus above content | ✅ | `deferred`, `anchored`, `occlude`. |
| Window blur or material | 🟡 | `WindowBackgroundAppearance::{Transparent, Blurred, MicaBackdrop, MicaAltBackdrop}` (`GP/platform.rs:2461-2468`). `Blurred` is "not always supported"; Mica is Windows 11 only. Sonder tried window styles in 0.8.66 to 0.8.68 and removed them in 0.8.69, so check with the user first. |

## Things that do not work

| Effect | Verdict | Why and the nearest alternative |
|---|---|---|
| Blur of elements or text, frosted glass behind a panel | ❌ | No filter in the style system; only box-shadow blur and the window material. Alternatives: pre-blurred image, fade plus slight size change for "defocus", window material. |
| Rotate, scale, skew, translate on ordinary elements | ❌ | `Transformation` appears only for SVG and low-level paint (`GP/elements/svg.rs`, `scene.rs`, `window.rs`). Alternatives: animate absolute `left`/`top` for movement, change font size for scale, swap SVGs. |
| Rounded clipping of children | ❌ | `overflow_hidden` clips to the rectangle only. Confirmed 2026-11-24: rounding the page container left square corners because each page paints its own square background. Alternative: overlay cap (`components::page_corner`), or round the child that paints. |
| Text shadow or glow | ❌ | No `text_shadow` in `GP/styled.rs`. Alternative: a duplicate copy of the text behind it, offset, at low opacity. |
| CSS-style filters (brightness, saturate, hue) | ❌ | Only image grayscale. Alternative: pre-process the image on the background executor. |
| Radial, conic or mesh gradients | ❌ | Only linear gradients. Alternative: pre-rendered image (cached, bounded). |
| Springs and physics | ✅ (updated 2026-11-27) | Use `gpui-motion-kit` (`ValueState` plus `Transition::Spring`/`Tween`), now in `main`: sidebar, account menu and lyrics use it (`src/app/motion.rs`). Springs retarget with their velocity; reduced motion snaps. Windows-tested for the sidebar. |
| Letter-spacing or tracking | ❓ | No `letter_spacing` or `tracking` in `GP/styled.rs`. Treat as no until a probe says otherwise. |
| Word-by-word karaoke fill | ❓ | Not built in. Might be possible with `with_highlights` updated per frame; needs a probe and a performance check. |

## Libraries

- **gpui-motion-kit 0.5** (checked 2026-11-25, on `redesign`): builds with Sonder
  after pinning `gpui-kit =0.7.3` (which moves gpui-pre to 0.3.9; the vendored
  `gpui-pre-windows` was rebased to 0.3.9). Gives exit animations
  (`Motion::exit`, `Frame::is_mounted`), springs, keyed presence. Its `width` and
  `height` animation is real layout, not a transform. Windows behaviour untested.

## Cost notes

- Animating width, height, margin or padding re-lays out siblings each frame.
  The original sidebar code avoided it on purpose. The `redesign` branch now
  animates the sidebar width; it is untested on Windows for jank.
- Reduced motion: `cx.reduce_motion()` must snap, not animate.
- Pixel baselines come from macOS 15 CI; local newer macOS shows false
  differences. Judge by where pixels differ.
