---
name: gpui-feasibility
description: Checks whether a visual, animation, layout or design idea can actually be built in Sonder's GPUI. Use automatically whenever the user asks for, shows or discusses a UI change, animation, effect, mockup or design (blur, glass, curves, transitions, shadows, gradients, transforms, clipping, text effects) before making a mockup, plan or code. Says yes and continues, says yes with a workaround, or says no and explains why with the closest alternative.
---

# GPUI feasibility check

Sonder's UI is GPUI (Zed's toolkit), which is fast and light but has few visual
effects. Web-style ideas often cannot be built as drawn. Check first, so the
user never approves a design that cannot ship.

## When to run

Any request that changes how Sonder looks or moves. Skip only when the whole
request uses effects already marked ✅ in `references/known-limits.md`; then
say "Doable in GPUI." in one line and carry on.

## Steps

1. **Split the idea into effects.** List each primitive it needs: blur, rounded
   clipping, transform, gradient, shadow, text effect, animated size, window
   material, and so on.
2. **Look each one up** in `references/known-limits.md`. It holds the verdicts
   and the evidence.
3. **Not listed, or unsure?** Search the source instead of guessing:
   - `~/.cargo/registry/src/*/gpui-pre-0.3.7/src` (core: `styled.rs`, `style.rs`,
     `color.rs`, `elements/`, `platform.rs`)
   - `~/.cargo/registry/src/*/gpui-kit-0.7.0` and `gpui-component-0.7.0`
   - `vendor/gpui-pre-windows` (Windows renderer, DirectX)
   - Sonder's own helpers: `src/app/components.rs`, `src/app/tokens.rs`

   Cite `file:line` for what you find.
4. **Still unsure?** Write the smallest throwaway probe: a temporary scene or
   test in `src/app/test_support`, run it, read the image in
   `target/ui-artifacts`, then discard it with `git checkout`. Never commit it.
5. **Check the costs** (see Gotchas): layout per frame, memory, Mac vs Windows,
   reduced motion.
6. **Give the verdict.** One of:
   - ✅ **Yes.** Say so in one line and continue the task.
   - 🟡 **Yes, with a workaround.** Name the workaround, its effort and its cost.
   - ❌ **No.** Say why in plain words, with the evidence, then offer the
     closest thing that works.

## Reply format

Lead with the verdict per effect, as a short list. Then:

- **Yes:** continue the task. No lecture.
- **Workaround or no:** one line on why, the nearest achievable version, its
  effort (small, medium, large) and its performance cost. Then ask which way
  to go. Do not build or mock up the impossible version quietly.
- **Mockups** may fake effects the app cannot do (HTML can blur anything).
  Label what is faked, and prefer a real Sonder screenshot as the base.
- Say "unverified" when it is. Never state a limit you have not checked.

## Gotchas that decide most answers

- **Animating size or margin re-lays out the page every frame.** The original
  sidebar code reflowed once on purpose. Prefer opacity, colour and absolute
  offsets. Flag any animated width, height, margin or padding, and ask the
  user to test it on Windows.
- **Every page paints its own square background.** Anything that must round or
  overlay at a page edge needs an overlay (`components::page_corner`).
- **Honour reduced motion.** Use `cx.reduce_motion()` and snap instead.
- **Memory is a budget.** No per-frame allocation, no polling. Do image work on
  the background executor and keep caches bounded (see `blur_artist_image`).
- **Mac is not Windows.** Mac renders with Metal, Windows with DirectX
  (`vendor/gpui-pre-windows`). Windows-only code cannot be type-checked on the
  Mac, and the user tests on Windows 11. Mention the gap when an effect depends
  on the platform (Mica needs Windows 11).
- **Visual baselines.** Any pixel change fails `cargo test --test ui_rendering`
  by design. Accept with `scripts/accept-baseline.sh` on the CI platform.
- **Test builds.** Use `gh workflow run windows.yml -R parentless/sonder --ref
  <branch>`. Never release design experiments from `main`.

## Keep the limits file alive

When you learn something new, from the source, a probe, or a failed build, add
a dated row to `references/known-limits.md` in the same turn. A limit found
once should never be rediscovered.
