# Pen Design System

Saved design ledger for **Pen** so future sessions (Phases 2–5) hold exact visual
continuity without re-deriving tokens or proportions.

## Direction & Feel
- **Metaphor:** A fine nib on rag paper — calm, monochrome, architectural, zero glass.
- **Focal point:** The code on the writing surface. Chrome steps out of the way.
- **Signature:** A **48 pt left notebook margin** before right-aligned tabular line numbers (`NOTEBOOK_MARGIN = 48.0`), plus `GUTTER_GAP = 20.0` before column 0 of code.

## Palette & Contrast Rules
- **Paper (light default):**
  - Ground `bg`: `#f6f4f0`
  - Primary ink `fg`: `#1a1916` (**16.00:1** on ground)
  - Faint ink `faint`: `#5c5954` (**6.35:1** on ground)
- **Ink (dark default):**
  - Ground `bg`: `#12110f`
  - Primary ink `fg`: `#f4f1ea` (**16.73:1** on ground)
  - Faint ink `faint`: `#a8a49c` (**7.60:1** on ground)
- **Selection:** `fg` at `12%` alpha over `bg` (never blue).
- **Rules / dividers:** `1 px` hairline in `fg` at `20%` alpha.
- **Focus ring:** `1 px` solid `fg`, `1 pt` offset, instant (`0 ms` fade).
- **Syntax ramp:** Five perceptual OKLab lightness steps of `fg` toward `bg` (`[0.00, 0.14, 0.27, 0.40, 0.52]`), each walked back toward `fg` if under **4.5:1** WCAG contrast. Custom `theme.json` `accent` / `syntax` values under 4.5:1 are dropped.

## Depth & Surface Strategy
- **Borders-only, solid surfaces.** Sidebar, toolbar, find bar, status strip, and writing page share the exact same opaque `theme.bg` ground, divided by `1 px` `fg @ 20%` hairlines.
- No frosted toolbars, no `backdrop-filter`, no drop shadows on chrome or overlays.
- **Corner radius:**
  - Writing surface, selection bands, scroll thumb, find bar, status strip: **`0 px`**.
  - Chrome controls (`Open a folder` button, command palette card): **`6 px`**.
  - Nested rows inside the `6 px` palette card (`6 px` inset): **`4 px`** (concentric radius).

## Typography
- **Chrome (`Family::Sans`):** Bundled **IBM Plex Sans** (Regular 400, Medium 500, SemiBold 600), `13 px / 20 pt`, tracking `0`, sentence case everywhere. Never tracked uppercase eyebrows, never emoji icons, never a third font family.
- **Code (`Family::Mono`):** Bundled **IBM Plex Mono** (Regular 400, Medium 500), `13 px`, line-height `1.55` (`20.15 pt`), tracking `0`, exact glyph advance `7.80 pt`. Line numbers enable OpenType `'tnum' 1`.

## Motion Rules
- **Instant (`Transition::none()` + `tx.instant()`):** Caret movement, typing, scrolling, tab/file switching, command palette open/close, find bar open/close, focus rings.
- **Sidebar pane:** Critically damped spring (`Spring::new(0.35, 0.0)` — `0.35 s`, bounce `0`). Under `prefers-reduced-motion` (`cx.is_reduced_motion()`), `SplitView::reduced_motion(true)` replaces translation with an opacity fade.
- **First launch only:** `200 ms` page opacity fade (`Spring::new(0.20, 0.0)`) gated by `<config>/pen/launched`; every subsequent launch is `0 ms` instant.
- **Press scale:** Skipped in Phase 1 because the pinned Rune rev exposes no fine-pointer query on `Cx`/`Env` (per Task 10 spec).

## Key Component Measurements
- **Sidebar (`PaneState`):** `248 pt` default width, resizable `160–480 pt`, collapsed on the empty screen (`hide_sidebar_toggle`), auto-opened when a project folder loads.
- **Empty screen CTA (`pen/open-folder`):** `40 pt` height · `20 pt` horizontal pad · `6 px` radius · `1 px fg@20%` border · `"Open a folder"` (`13 px / 500`), sole control on screen.
- **Find bar (`pen/find-bar`):** `38 pt` height · `48 pt` left pad (aligned to notebook margin) · `1 px` bottom hairline · `13 px` mono input on shared `20 pt` baseline.
- **Command palette (`pen/palette`):** `440 pt` width · `76 pt` top offset · `6 px` radius · `42 pt` query header + `6 pt` padded list of `34 pt` rows (`4 px` inner radius).
- **Caret (`pen/caret`):** `1.5 pt × 16.15 pt` solid `fg` stroke, `.ime_cursor()` when the editor owns focus.
