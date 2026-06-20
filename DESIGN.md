---
# ============================================================================
# DESIGN.md  —  Arctis Sound Manager
# A Tauri v2 desktop app for the SteelSeries Arctis Nova Pro Wireless.
# UI must look and feel like SteelSeries GG / Sonar / Arctis Companion.
#
# Format: google-labs-code/design.md (YAML token front matter + ## prose).
# Token references use {path.to.token}. All values are CSS custom-property ready.
#
# NOTE ON SOURCING: hex values marked (SS-official) come from SteelSeries'
# published style guides. Values marked (ours) are our defaults chosen to match
# the SteelSeries look — they are NOT documented SteelSeries facts. See sources
# at the bottom of this file.
# ============================================================================

meta:
  name: Arctis Sound Manager
  platform: Tauri v2 (web frontend — HTML/CSS, framework-agnostic)
  theme: dark-only (v1)

colors:
  # --- Brand / accent (SS-official: styleguide.steelseries.io = #fc4c02; ----
  # --- software UX guide = #ff5200. We adopt #FF5200 as the interactive ------
  # --- accent and keep #FC4C02 as the deeper brand/press tone.) --------------
  accent:                "#FF5200"   # primary orange — interactive accent (ours: from SS software guide)
  accentBrand:           "#FC4C02"   # SteelSeries brand orange (SS-official)
  accentPress:           "#CD3400"   # darker orange for :active / gradient end (SS-official software guide)
  accentHover:           "#FF6A24"   # lighter orange for :hover (ours)
  accentSoft:            "rgba(255, 82, 0, 0.14)"  # 14% accent wash for selected rows / track fills (ours)
  accentBorder:          "rgba(255, 82, 0, 0.45)"  # accent at low alpha for focus rings/outlines (ours)

  # --- Surfaces (dark, near-black layered) ---------------------------------
  bgRoot:                "#0F0D0E"   # app background / window chrome (SS-official: $off-black)
  bgBase:                "#161415"   # base content area (ours)
  surface1:              "#1C1C1C"   # primary card / panel (SS-official software: "almost black")
  surface2:              "#212121"   # raised card / popover (SS-official: $darkest-grey)
  surface3:              "#262626"   # highest surface / dropdown menu (SS-official: $darker-grey)
  surfaceInput:          "#2D2D2D"   # input/control field fill (SS-official software guide)
  surfaceInputAlt:       "#404040"   # secondary-button / track base (SS-official software gradient start)

  # --- Hairlines & dividers -------------------------------------------------
  border:                "rgba(255, 255, 255, 0.08)"  # default hairline (ours)
  borderStrong:          "rgba(255, 255, 255, 0.14)"  # emphasized divider (ours)
  borderInset:           "rgba(255, 255, 255, 0.10)"  # inset top-light on raised modals (SS-official software guide)

  # --- Text (light-on-dark) -------------------------------------------------
  textPrimary:           "#E6E6E6"   # primary copy / headings (SS-official: $off-white)
  textBright:            "#FFFFFF"   # text on accent fills, selected items (SS-official)
  textSecondary:         "#A5A7AA"   # default body / labels (SS-official: $grey)
  textTertiary:          "#7A7C80"   # captions, units, hints (ours: between grey and dark-grey)
  textDisabled:          "#5A5A5A"   # disabled (ours)

  # --- Semantic / status ----------------------------------------------------
  selected:              "#0091D1"   # selected-item blue (SS-official software guide)
  selectedBrand:         "#007EC8"   # brand blue (SS-official styleguide)
  success:               "#41A930"   # ok / connected / charging (SS-official: $brand-green)
  warning:               "#FFBE00"   # low battery / caution (SS-official: $brand-yellow)
  danger:                "#E5484D"   # error / disconnect / destructive (ours: SteelSeries publishes no error red)
  muted:                 "#A5A7AA"   # "muted channel" inactive state == textSecondary (ours)

  # --- Data-viz / EQ band identity colors (10 bands) ------------------------
  # Derived from SteelSeries' documented "highlight colors" for categorization.
  band1:                 "#FF5200"   # accent (ours)
  band2:                 "#0091D1"   # selected blue (SS-official)
  band3:                 "#41A930"   # green (SS-official)
  band4:                 "#754BD3"   # purple (SS-official)
  band5:                 "#FFBE00"   # yellow (SS-official)
  band6:                 "#2A7199"   # light blue (SS-official highlight)
  band7:                 "#B24736"   # clay (SS-official highlight)
  band8:                 "#356E74"   # aqua (SS-official highlight)
  band9:                 "#6F3969"   # plum (SS-official highlight)
  band10:                "#50648C"   # slate (SS-official highlight)

  # --- Gradients (SS-official software guide) -------------------------------
  gradientPrimary:       "linear-gradient(180deg, #FF5200 0%, #CD3400 100%)"   # primary action button
  gradientSecondary:     "linear-gradient(180deg, #404040 0%, #2D2D2D 100%)"   # secondary action button

typography:
  # SteelSeries ships proprietary faces (Verdana, Helvetica Neue Condensed,
  # Frucade). We substitute an open system stack with the same intent:
  # a clean UI sans for body, a condensed/heavy treatment for headings, and a
  # tabular mono for numeric readouts (dB, Hz, %, battery). (ours)
  fontFamilyUI:          "'Inter', 'Segoe UI', system-ui, -apple-system, sans-serif"
  fontFamilyDisplay:     "'Saira Condensed', 'Inter', system-ui, sans-serif"  # caps headings / accent
  fontFamilyMono:        "'JetBrains Mono', 'SF Mono', ui-monospace, monospace" # numeric readouts (tabular)

  display:   { fontFamily: "{typography.fontFamilyDisplay}", fontSize: "26px", fontWeight: 700, lineHeight: "1.1",  letterSpacing: "0.04em", textTransform: "uppercase" } # H1/page title
  h2:        { fontFamily: "{typography.fontFamilyDisplay}", fontSize: "18px", fontWeight: 700, lineHeight: "1.2",  letterSpacing: "0.03em", textTransform: "uppercase" } # section header
  h3:        { fontFamily: "{typography.fontFamilyUI}",      fontSize: "14px", fontWeight: 700, lineHeight: "1.3",  letterSpacing: "0.01em" } # card title
  bodyLg:    { fontFamily: "{typography.fontFamilyUI}",      fontSize: "14px", fontWeight: 400, lineHeight: "1.45" }
  body:      { fontFamily: "{typography.fontFamilyUI}",      fontSize: "13px", fontWeight: 400, lineHeight: "1.45" } # default
  label:     { fontFamily: "{typography.fontFamilyUI}",      fontSize: "12px", fontWeight: 600, lineHeight: "1.3",  letterSpacing: "0.02em" } # control labels (often UPPERCASE)
  caption:   { fontFamily: "{typography.fontFamilyUI}",      fontSize: "11px", fontWeight: 400, lineHeight: "1.3" } # hints, units
  micro:     { fontFamily: "{typography.fontFamilyUI}",      fontSize: "10px", fontWeight: 600, lineHeight: "1.2",  letterSpacing: "0.06em", textTransform: "uppercase" } # tab/nav labels, badges
  button:    { fontFamily: "{typography.fontFamilyUI}",      fontSize: "12px", fontWeight: 700, lineHeight: "1",    letterSpacing: "0.05em", textTransform: "uppercase" }
  readout:   { fontFamily: "{typography.fontFamilyMono}",    fontSize: "13px", fontWeight: 500, lineHeight: "1",    fontFeature: "'tnum' 1" } # dB / Hz / % numbers

layout:
  # 4px base grid (ours).
  space0:   "0px"
  space1:   "4px"
  space2:   "8px"
  space3:   "12px"
  space4:   "16px"
  space5:   "20px"
  space6:   "24px"
  space8:   "32px"
  space10:  "40px"
  space12:  "48px"
  space16:  "64px"

  # Sizing
  navWidth:        "72px"    # left icon rail (ours; collapsible)
  navWidthExpanded:"220px"
  topbarHeight:    "56px"
  channelStripW:   "120px"   # mixer channel strip width (ours)
  channelStripWMin:"96px"
  controlH:        "36px"    # default control/row height (ours; SS guide used ~22-24px, we relax for desktop)
  controlHsm:      "28px"
  fieldH:          "36px"    # input / dropdown height
  tabH:            "40px"    # SS guide: ~33px; relaxed (ours)
  iconBtn:         "32px"
  pagePadding:     "24px"
  contentMaxW:     "1280px"
  windowMinW:      "920px"   # min app window (ours; SS config window was 890px)
  windowMinH:      "620px"

elevation:
  # SteelSeries software uses an inset top-light + downward drop shadow on
  # raised surfaces (SS-official software guide). We layer that as our system.
  e0:  "none"                                                        # flat on bg
  e1:  "0 1px 2px rgba(0,0,0,0.4)"                                   # cards
  e2:  "0 4px 8px rgba(0,0,0,0.45)"                                  # popovers / dropdowns
  e3:  "inset 0 0 1px 1px rgba(255,255,255,0.10), 0 6px 8px rgba(0,0,0,0.50)" # modals (SS-official)
  glowAccent: "0 0 0 1px {colors.accentBorder}, 0 0 12px rgba(255,82,0,0.35)" # active band dot / live meter peak

shapes:
  radiusXs:  "3px"
  radiusSm:  "4px"   # inputs, buttons, small controls (ours — SS look is low-radius/squared)
  radiusMd:  "6px"   # cards, panels
  radiusLg:  "10px"  # large containers / modals
  radiusPill:"999px" # slider thumbs, toggles, badges
  borderWidth: "1px"

motion:
  durInstant: "80ms"
  durFast:    "120ms"   # hovers, toggles, button states
  durBase:    "180ms"   # most transitions, tab/panel swaps
  durSlow:    "280ms"   # page transitions, EQ curve morph
  easeStandard: "cubic-bezier(0.2, 0, 0, 1)"     # enter/standard
  easeOut:      "cubic-bezier(0.16, 1, 0.3, 1)"  # decelerate (overlays in)
  easeIn:       "cubic-bezier(0.4, 0, 1, 1)"     # accelerate (overlays out)

components:
  buttonPrimary:
    background: "{colors.gradientPrimary}"
    textColor: "{colors.textBright}"
    typography: "{typography.button}"
    rounded: "{shapes.radiusSm}"
    height: "{layout.controlH}"
    padding: "0 {layout.space4}"
  buttonSecondary:
    background: "{colors.gradientSecondary}"
    textColor: "{colors.textPrimary}"
    border: "1px solid {colors.border}"
    typography: "{typography.button}"
    rounded: "{shapes.radiusSm}"
    height: "{layout.controlH}"
  card:
    background: "{colors.surface1}"
    border: "1px solid {colors.border}"
    rounded: "{shapes.radiusMd}"
    padding: "{layout.space5}"
    shadow: "{elevation.e1}"
  channelStrip:
    background: "{colors.surface1}"
    width: "{layout.channelStripW}"
    rounded: "{shapes.radiusMd}"
    padding: "{layout.space3}"
  slider:
    trackColor: "{colors.surfaceInput}"
    fillColor: "{colors.accent}"
    thumbColor: "{colors.textBright}"
    thumbSize: "16px"
  toggle:
    offBg: "{colors.surfaceInput}"
    onBg: "{colors.accent}"
    knobColor: "{colors.textBright}"
  dropdown:
    background: "{colors.surfaceInput}"
    menuBackground: "{colors.surface3}"
    textColor: "{colors.textPrimary}"
    height: "{layout.fieldH}"
    rounded: "{shapes.radiusSm}"
  tab:
    textColor: "{colors.textSecondary}"
    activeTextColor: "{colors.textBright}"
    activeIndicator: "{colors.accent}"
    height: "{layout.tabH}"
---

## Overview

**North star.** Arctis Sound Manager is a *device console*, not a media app. It should feel like a precision piece of audio hardware rendered in software — the same family as **SteelSeries GG / Sonar / Arctis Companion**: dark, near-black, dense-but-legible, with a single decisive **orange** accent that means "live / active / you-can-touch-this." Everything else is grayscale. Color is information, not decoration.

**Three principles, in priority order:**

1. **Orange is a scalpel, not a paintbrush.** The SteelSeries accent (`{colors.accent}`) marks exactly one thing per context: the live value, the selected item, the focused control, the primary action. If two oranges fight for attention on a screen, one of them is wrong. Surfaces, text, borders, and chrome are all neutral grayscale.
2. **Direct manipulation over dialogs.** This is a mixer and an EQ — users *drag*. Sliders, the EQ curve, and band dots respond immediately and continuously, with the numeric readout (`{typography.readout}`) updating in real time. Prefer in-place editing to modal forms.
3. **Hardware honesty.** Reflect real device state truthfully and fast: battery, ANC mode, charging, connection. When the device says something changed (e.g. ANC toggled on the headset), the UI mirrors it. Never show a control as available when the hardware can't honor it — disable and explain.

**Aesthetic anchors (grounded in real SteelSeries software):** layered near-black surfaces (`{colors.bgRoot}` → `{colors.surface3}`), low border-radius / mostly-squared geometry, uppercase condensed headings, thin white hairlines, an inset-top-light + drop-shadow elevation model, and the orange→deep-orange gradient on primary actions. Density is deliberately tight (SteelSeries' own software is compact), but we relax control heights modestly for a desktop Tauri window and pointer/keyboard use.

## Colors

The palette is **dark-only** for v1. Build it as CSS custom properties (`--ss-accent`, `--ss-surface-1`, …) mapped 1:1 from the `colors` tokens above.

**Surface ladder (lowest → highest).** Stack surfaces by elevation, never by hue:
`{colors.bgRoot}` (window) → `{colors.bgBase}` (content) → `{colors.surface1}` (cards/strips) → `{colors.surface2}` (raised cards) → `{colors.surface3}` (dropdowns/popovers). Inputs sit on `{colors.surfaceInput}`. Keep adjacent surfaces ~1 step apart; if you need separation, prefer a hairline (`{colors.border}`) over a bigger color jump.

**Text on dark.** `{colors.textPrimary}` for headings/important copy, `{colors.textSecondary}` for the bulk of labels and body, `{colors.textTertiary}` for units/hints. `{colors.textBright}` (pure white) is reserved for text sitting *on* the orange accent or on selected/blue fills. All four neutral text tokens clear WCAG AA on our surfaces (see Do's & Don'ts).

**Accent usage rules (strict):**
- Use `{colors.accent}` for: the filled portion of an active slider/track, the focused/dragged EQ band dot, the active tab indicator, the selected profile, the live level-meter fill, and the single primary button on a screen.
- Use the **gradient** `{colors.gradientPrimary}` only for primary action buttons.
- Do **not** use orange for: large background areas, body text, icons at rest, card borders, or more than one "primary" emphasis in the same view.
- `{colors.accentSoft}` (14% wash) is the only acceptable "large orange area" — for a selected row background or a slider track fill behind the bright fill.

**Semantic colors** are for *state*, not styling: `{colors.success}` (connected/charging/levels-ok), `{colors.warning}` (low battery/clipping caution), `{colors.danger}` (disconnected/error/destructive), `{colors.selected}` (blue) for multi-select or "currently selected device/input" where orange would over-emphasize. These mirror SteelSeries' published brand greens/yellows/blues.

**Band colors** (`{colors.band1}`–`{colors.band10}`) give each parametric EQ band a stable identity so its dot, its row in the band list, and its Q-region shading all match. The active/selected band additionally gets the accent treatment on top of its identity color.

## Typography

SteelSeries ships proprietary faces; we substitute an open stack with the same *intent* (`{typography.fontFamilyUI}` for everything readable, `{typography.fontFamilyDisplay}` condensed-uppercase for headings and accents, `{typography.fontFamilyMono}` tabular for numbers). Bundle the fonts with the app (Tauri offline) rather than relying on the network.

**Scale & rules:**
- Page titles use `display` (uppercase, condensed, tracked-out) — this is the strongest SteelSeries signature.
- Section headers use `h2` (uppercase condensed); card titles use `h3` (sentence/Title case, regular sans bold).
- Body is `body` (13px); labels are `label` (12px semibold, frequently UPPERCASE for control captions like "GAME", "SIDETONE", "ANC").
- **Every numeric readout** — dB, Hz, %, ms, battery % — uses `readout` (mono, tabular figures) so digits don't jitter while dragging. This is non-negotiable for the mixer and EQ.
- Tab/nav labels and small badges use `micro` (10px uppercase, tracked).

Avoid more than 3 distinct sizes in one component. Let weight and case (uppercase vs sentence) carry hierarchy, the way SteelSeries does, rather than many sizes.

## Layout

**App shell.** A persistent **left icon rail** (`{layout.navWidth}`, collapsible to `{layout.navWidthExpanded}`) holds top-level destinations: Mixer, EQ, Spatial, Mic, Device. A **top bar** (`{layout.topbarHeight}`) holds the app/device name, connection + battery status (right-aligned), and the **Profiles dropdown** (right side, near status). Content fills the remainder, capped at `{layout.contentMaxW}` and centered on very wide windows. Minimum window `{layout.windowMinW}` × `{layout.windowMinH}`.

**Grid & spacing.** Everything snaps to the 4px grid (`space1`–`space16`). Page padding `{layout.pagePadding}`. Cards separated by `{layout.space4}`. Within a card, group related controls with `{layout.space3}` and separate groups with `{layout.space5}`. Density is tight but never cramped — give draggable targets room (see Accessibility for min hit sizes).

**Responsiveness.** Single-window desktop, but resilient: the mixer's channel strips lay out in a horizontal row that can scroll horizontally if the window is narrow rather than shrinking below `{layout.channelStripWMin}`. The EQ canvas is fluid-width with a fixed minimum height. Collapse the nav rail to icons-only under ~1040px.

## Elevation & Depth

Depth is communicated by the **surface ladder + a SteelSeries-style inset/drop shadow**, not by heavy blur. Cards rest at `{elevation.e1}`. Popovers and dropdown menus float at `{elevation.e2}`. Modals/dialogs use the signature `{elevation.e3}` (inset top hairline of light + a soft downward shadow) so they read as a panel lifted off the app.

Reserve `{elevation.glowAccent}` for genuinely *live* elements only: the currently dragged EQ band dot and a level meter hitting peak. The glow is the orange accent's "energy" expression — used sparingly it makes the UI feel responsive and physical; overused it looks like a toy.

## Shapes

SteelSeries software reads **squared and technical** — keep radii small. Inputs/buttons/small controls use `{shapes.radiusSm}`; cards/panels `{shapes.radiusMd}`; large containers/modals `{shapes.radiusLg}`. Only inherently round things are pill/circular: slider thumbs, toggle knobs, EQ band dots, and badges (`{shapes.radiusPill}`). Borders are a single `{shapes.borderWidth}` hairline. Avoid soft, friendly, fully-rounded "consumer app" shapes — this is gear.

## Components

> Specs below are implementation-ready. Where a property isn't listed, inherit from the token tables. All states must have hover / active / focus-visible / disabled treatments.

### Left nav rail
Vertical stack of icon buttons (`{layout.iconBtn}`), one per page. Active item: orange `{colors.accent}` icon + a 3px orange left-edge indicator + subtle `{colors.accentSoft}` background. Inactive: `{colors.textSecondary}` icon, → `{colors.textBright}` on hover. Labels appear (uppercase `micro`) when expanded. Tooltip on hover when collapsed.

### Top bar
Left: device name (`h3`) + a connection dot (`{colors.success}` connected / `{colors.danger}` disconnected). Right: **battery indicator** (see below), then the **Profiles dropdown**. Background `{colors.bgRoot}`, bottom hairline `{colors.border}`.

### Profiles dropdown / selector
A compact dropdown button (`{layout.fieldH}`, `{colors.surfaceInput}` fill, caret icon) showing the active profile name. Open → menu on `{colors.surface3}` at `{elevation.e2}`: list of profiles with the active one marked by an orange check + `{colors.accentSoft}` row. Footer actions: "Save", "Save as new…", "Rename", "Delete". A small orange dot next to the name indicates unsaved changes. Keyboard: Up/Down to move, Enter to select, Esc to close.

### Channel strip (Mixer)
Vertical card (`{layout.channelStripW}`, `{colors.surface1}`, `{shapes.radiusMd}`). Top → bottom:
1. **Channel label** (`label`, uppercase) + small channel icon — Game / Chat / Media / Aux / Mic. Master strip is visually distinct (slightly wider, accent-tinted top border).
2. **Vertical volume slider** (the strip's hero; see Volume slider) with a live **level meter** rail beside it.
3. **Numeric dB readout** (`readout`) below the slider, editable on click.
4. **Mute button** (see below), and where applicable a **route/secondary toggle**.
Strips sit in a horizontal row; the **Master** strip is anchored at the right end after a divider. Each non-master strip's accent fill uses the global `{colors.accent}` (channel identity is shown by icon/label, not by recoloring the fill — keep one accent).

### Volume slider
- Vertical in channel strips, horizontal elsewhere.
- Track: `{colors.surfaceInput}`, `{shapes.radiusPill}`, ~4px thick. Fill (from min to thumb): `{colors.accent}`.
- Thumb: `{shapes.radiusPill}` `{colors.textBright}` circle, 16px, with `{elevation.e1}`; grows ~10% and gains a faint `{colors.accentBorder}` ring on hover/drag.
- Live **level meter** runs parallel as a thin segmented/gradient rail: green `{colors.success}` → yellow `{colors.warning}` → orange/red near peak; peak hold shows briefly.
- Readout updates continuously while dragging; `durInstant` for thumb position, no easing during active drag (1:1 with pointer).
- Double-click resets to 0 dB / default. Scroll-wheel nudges by a fine step; Shift = coarse.

### Mute button
Square-ish toggle (`{layout.controlHsm}`) with a speaker/mic icon. **Active (muted):** `{colors.danger}` icon + `{colors.danger}`-tinted background + the channel slider fill desaturates to `{colors.muted}`. **Unmuted:** `{colors.textSecondary}` icon. Solo (if present) uses `{colors.warning}`. State change animates over `durFast`.

### Parametric EQ canvas + band dots
The centerpiece. A wide dark plotting surface on `{colors.surface2}` inside a card.
- **Grid:** faint `{colors.border}` gridlines; X axis = frequency (log scale, 20 Hz–20 kHz, labeled `caption`), Y axis = gain (−12…+12 dB or similar, labeled). 0 dB line slightly brighter.
- **Curve:** the summed EQ response drawn as a smooth 2–3px line in `{colors.accent}`, with a soft `{colors.accentSoft}` fill between curve and the 0 dB line. Curve morphs over `durSlow` with `easeOut` when a preset loads; tracks 1:1 (no easing) during a drag.
- **Band dots:** up to **10** draggable dots, each colored by its `{colors.band1..10}` identity. Dragging X = frequency, Y = gain; a modifier (or scroll on the dot) changes **Q** (visualized as the width of a faint shaded bell under that band in its band color). The **active/dragged dot** enlarges, gets `{elevation.glowAccent}`, and shows a floating readout chip (mono: freq / gain / Q).
- **Add/remove bands:** double-click empty canvas adds a band at that freq/gain (up to 10); double-click a dot (or a delete affordance) removes it. Reflect SteelSeries' documented "add/delete up to 10 bands" behavior.
- **Band list (optional companion):** a compact row list beside/below the canvas, each row tinted by band color, with freq/gain/Q numeric fields and an enable toggle. Selecting a row selects its dot and vice-versa.
- Keyboard: Tab between dots; arrows nudge gain/freq; Shift = coarse; Delete removes the focused band.

### Toggle (switch)
Pill track `{layout.controlHsm}`-tall. Off: `{colors.surfaceInput}` track, knob `{colors.textBright}`. On: `{colors.accent}` track, knob white. Knob slides over `durFast` with `easeStandard`. Used for ANC on/off-style binary controls; for multi-state (ANC: Off / On / Transparency) use a **segmented control** instead.

### Segmented control
Row of 2–4 options in a `{colors.surfaceInput}` housing (`{shapes.radiusSm}`). Selected segment: `{colors.accent}` fill (or `{colors.accentSoft}` + orange text for a lighter touch) with `{colors.textBright}` label; others `{colors.textSecondary}`. Selection indicator slides over `durBase`. Ideal for ANC modes, spatial presets, EQ preset categories.

### Dropdown / select
`{layout.fieldH}`, `{colors.surfaceInput}` fill, hairline border, caret icon, `{colors.textPrimary}` value. Menu on `{colors.surface3}` at `{elevation.e2}`; hovered option `{colors.surface2}`; selected option orange check + `{colors.accentSoft}`. Full keyboard support.

### Tabs / page nav (sub-nav)
Used inside a page (e.g. per-channel EQ vs Spatial). Tab labels `micro`/`label`, `{colors.textSecondary}`; active tab `{colors.textBright}` with a 2px `{colors.accent}` underline indicator that slides over `durBase`. Height `{layout.tabH}`.

### Card / panel
`{colors.surface1}`, `{shapes.radiusMd}`, hairline border, `{elevation.e1}`, padding `{layout.space5}`. Optional header row: `h3` title left, actions right, hairline divider beneath.

### Battery & status indicators
- **Battery:** horizontal battery glyph + `readout` percentage. Fill color by level: `{colors.success}` >40%, `{colors.warning}` 15–40%, `{colors.danger}` <15%. **Charging:** a small bolt + a gentle pulse animation on the fill (respect reduced-motion). Show "—" / dimmed when device disconnected.
- **Connection:** dot + label ("Connected" `{colors.success}` / "Disconnected" `{colors.danger}` / "Connecting…" `{colors.warning}` with a slow pulse).
- **Status pills/badges:** `{shapes.radiusPill}`, `micro` uppercase, semantic-colored text on a low-alpha tint of the same color (e.g. ANC "ON" pill).

### Device controls (Device page)
Grouped in cards: Battery, ANC (segmented: Off / On / Transparency, plus an intensity slider when On), Sidetone (slider + on/off), Mic volume/gain (slider), Auto-off / power, firmware/info. Each control: `label` caption left, control right, optional `caption` hint beneath. Disabled controls (unsupported by current firmware/connection) dim to `{colors.textDisabled}` with a tooltip explaining why.

## Layout — Page structures

**Mixer (home).** Top bar + nav. Main area: a horizontal row of **channel strips** — Game, Chat, Media, Aux, Mic — followed by a divider and the **Master** strip at the right. Above the strips, a slim header with the active profile name (echoing the Profiles dropdown) per SteelSeries' "preset name in the mixer" behavior. Clicking a channel's label/EQ affordance deep-links to that channel's EQ page.

**Per-channel EQ / Spatial.** Page title = channel name. Sub-tabs: **EQ** | **Spatial**. EQ tab: the parametric **EQ canvas** as hero (full width), preset selector (segmented or dropdown) above it, optional **band list** below, and a "Reset / Save to profile" action row. Spatial tab: spatial-audio enable toggle, a preset/segmented selector, and any directional/room controls as sliders within cards.

**Mic (tuning).** Cards for: input level meter + gain slider, sidetone slider, noise-suppression / noise-gate controls (toggles + sliders), a mic EQ (smaller reuse of the EQ canvas) if applicable, and a "test mic" affordance with a live meter. Keep the live input meter prominent.

**Device.** Stacked cards as in **Device controls** above: Battery (hero), ANC, Sidetone, Mic, Power/Auto-off, Firmware & info. Connection/battery also mirrored in the top bar.

**Profiles.** Primarily the top-bar dropdown. A fuller management view (optional) lists profiles as cards (name, last-used, scope) with create/duplicate/rename/delete and import/export; the active profile gets the orange-accented selected treatment.

## Interaction states & feedback

- **Every interactive element** defines: default, hover (lighten/elevate, `durFast`), active/pressed (darken or accent-press `{colors.accentPress}`), focus-visible (see Accessibility), disabled (`{colors.textDisabled}`, no shadow, `cursor: not-allowed`), and where relevant selected/active (orange).
- **Dragging** (sliders, EQ dots) is 1:1 with no easing; show the live readout chip and, for EQ, the morphing curve. On release, the value "settles" (no animation needed unless snapping to a step).
- **Optimistic + truthful device sync:** apply changes immediately and optimistically; if the device rejects/fails, revert with a brief inline error (`{colors.danger}`) and a toast. If the device changes state externally, animate the control to the new value over `durBase`.
- **Toasts/notifications:** bottom or top-right, `{colors.surface3}` at `{elevation.e2}`, semantic accent stripe, auto-dismiss, with the orange used only for action links.
- **Empty/disconnected:** when no device is connected, show a clear empty state (icon + "No Arctis Nova Pro detected" + retry), and dim device-dependent controls rather than hiding the layout.

## Accessibility

- **Contrast (WCAG AA, 4.5:1 body / 3:1 large & UI):** `{colors.textPrimary}` (#E6E6E6) and `{colors.textSecondary}` (#A5A7AA) both pass on `{colors.surface1}`/`{colors.surface2}`. `{colors.textTertiary}` is for non-essential hints only. **Do not** put `{colors.accent}` text on dark for body copy (orange-on-near-black is borderline) — accent is for fills, indicators, and large/bold elements; use `{colors.textBright}` on orange fills (passes). Verify every text/bg pair with the design.md `lint` contrast check.
- **Focus-visible:** a 2px `{colors.accent}` outline with a 2px offset (or the `{colors.accentBorder}` ring on round controls). Never remove focus rings; never rely on color alone — pair selected/active states with an icon, underline, or check so color-blind users aren't excluded (don't distinguish channels/states by hue alone).
- **Keyboard:** full operability — Tab order follows visual order; sliders and EQ dots are focusable and arrow-adjustable (Shift = coarse, Home/End = min/max, Delete removes a focused band); dropdowns/menus support Up/Down/Enter/Esc; the nav rail is reachable and labeled.
- **Hit targets:** minimum 28px interactive size; draggable EQ dots have ≥32px effective hit radius even if drawn smaller.
- **Screen readers:** label every control (channel name + value, e.g. "Game volume, −6 dB"); announce live device state changes politely (battery, connection) via an aria-live region; meters and the EQ canvas expose text alternatives / numeric values.
- **Reduced motion:** honor `prefers-reduced-motion` — disable pulses/glows/curve-morph easing and fall back to instant state changes; functional feedback (value updates) must not depend on animation.

## Do's and Don'ts

**Do**
- Keep the UI grayscale + one orange. Let surfaces, hairlines, and weight do the structural work.
- Use mono tabular figures for *all* numeric readouts so digits don't shift while dragging.
- Make sliders and EQ dots feel physical: immediate 1:1 drag, live readout, a subtle settle.
- Mirror real hardware state truthfully and quickly; disable + explain controls the device can't honor.
- Reserve the accent glow for genuinely live elements (dragged band dot, peaking meter).
- Run `npx @google/design.md lint DESIGN.md` and fix contrast/reference/order violations before shipping UI.

**Don't**
- Don't use orange as a background wash, body-text color, or on more than one "primary" emphasis per view.
- Don't distinguish channels or states by color alone — pair with icon/label/shape (color-blind safety).
- Don't introduce large border-radii or soft "consumer" styling — keep it squared and technical.
- Don't add a second accent hue; semantic colors (green/yellow/red/blue) are for *state*, not theming.
- Don't animate values during an active drag, and don't let any animation be the *only* feedback.
- Don't put light-theme assumptions anywhere; v1 is dark-only, but keep tokens themeable for later.

---

## Sources & provenance

**Format / methodology**
- google-labs-code/design.md — README & spec: https://github.com/google-labs-code/design.md
  - Raw README: https://raw.githubusercontent.com/google-labs-code/design.md/main/README.md
  - (Canonical section order, YAML-token-front-matter + prose structure, `{token.path}` references, `lint`/`diff`/`export` CLI, WCAG contrast linting.)

**SteelSeries visual language (real, cited)**
- SteelSeries brand styleguide (official hex tokens): https://styleguide.steelseries.io/colors/
  - $orange `#fc4c02`, $off-black `#0f0d0e`, $off-white `#e6e6e6`, $grey `#a5a7aa`, $darkest-grey `#212121`, $darker-grey `#262626`, $dark-grey `#313131`, brand blue `#007ec8`, yellow `#ffbe00`, green `#41a930`, purple `#754bd3`.
- SteelSeries software UX/style guide (official): https://techblog.steelseries.com/ux-guide/index.html
  - Software orange `#ff5200` ("positive actions"), surfaces `#1c1c1c`/`#2d2d2d`/`#4d4d4d`, input gray `#2d2d2d`, selected blue `#0091d1`, primary gradient `#ff5200→#cd3400`, secondary gradient `#404040→#2d2d2d`, modal shadow `inset 0 0 1px 1px rgba(255,255,255,.1), 0 6px 8px rgba(0,0,0,.5)`, uppercase condensed headings, highlight colors for categorization (used for our EQ band palette).
- SteelSeries Sonar (official product page): https://steelseries.com/gg/sonar
- Sonar parametric EQ — add/delete up to 10 bands (official support): https://support.steelseries.com/hc/en-us/articles/5570622280077-How-do-I-add-or-delete-a-band-to-Sonar-Parametric-EQ
- Sonar review describing dark UI, mixer channel strips, draggable EQ curve with up to 10 points, Mixer/Game/Chat/Microphone tabs: https://www.streamtechreviews.com/blog/sonar
- Sonar review (mixer/parametric EQ overview): https://www.tech2geek.net/steelseries-sonar-review-the-easiest-free-audio-mixer-for-windows/
- Arctis Nova Pro Wireless review (base-station OLED, ANC modes incl. Transparency, sidetone, battery, GG/Engine/Sonar integration): https://www.techpowerup.com/review/steelseries-arctis-nova-pro-wireless/3.html

**Provenance note.** All `#XXXXXX (SS-official)` values come from the two SteelSeries style guides above. Values marked **(ours)** — extended surface ladder, alpha hairlines, spacing/sizing/radius/motion scales, `danger` red, accent hover/soft tints, substitute font stack, and component micro-specs — are our defaults chosen to match the SteelSeries look and are NOT documented SteelSeries facts.

## Open questions for the owner
1. **Accent value:** we standardized on `#FF5200` (software guide) for interactive UI and kept `#FC4C02` (brand styleguide) as the press/brand tone. OK, or prefer one canonical orange?
2. **Fonts:** SteelSeries uses Verdana / Helvetica Neue Condensed / Frucade (proprietary). We propose Inter + Saira Condensed + JetBrains Mono (bundled). Want a closer match (licensed Helvetica) or a different open trio?
3. **Channel set:** confirm the exact strips for Nova Pro Wireless (Game / Chat / Media / Aux / Mic → Master) and whether Aux/streaming/secondary-output strips apply to this device.
4. **Light theme:** v1 is dark-only. Do you want tokens authored for an eventual light theme now, or defer?
5. **EQ Q-editing gesture:** preferred interaction for Q on a band dot — scroll-on-dot, modifier-drag, or an explicit numeric field in the band list (or all three)?
6. **Spatial controls scope:** how deep do spatial-audio controls go (simple on/off + preset, vs. directional/room parameters)? Affects the Spatial page layout.
