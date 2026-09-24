---
name: Looking Glass
description: Self-hosted network diagnostics drawn as a backbone map.
colors:
  map-paper: "#f3f3f0"
  map-panel: "#ffffff"
  map-sunk: "#ebebe7"
  ink: "#16171a"
  ink-hover: "#33353b"
  ink-muted: "#595c63"
  ink-faint: "#8b8e94"
  rule: "#dcdcd6"
  rule-strong: "#a6a8a3"
  signal-ok: "#1d7a4a"
  signal-warn: "#9a5200"
  signal-danger: "#b3261e"
  night-paper: "#111214"
  night-panel: "#18191c"
  night-sunk: "#0b0c0d"
  night-ink: "#ecece6"
  night-ink-muted: "#a2a5ab"
  night-rule: "#2a2c30"
  line-red: "#c8102e"
  line-blue: "#0b5cad"
  line-green: "#00783a"
  line-ochre: "#8a6500"
  line-violet: "#6b3fa0"
  line-teal: "#00747a"
  line-orange: "#b34a0c"
  line-magenta: "#b0006d"
  line-brown: "#7a4a1e"
typography:
  display:
    fontFamily: "Overpass Variable, Overpass, system-ui, sans-serif"
    fontSize: "40px"
    fontWeight: 800
    lineHeight: "44px"
    letterSpacing: "-0.03em"
  display-sm:
    fontFamily: "Overpass Variable, Overpass, system-ui, sans-serif"
    fontSize: "28px"
    fontWeight: 800
    lineHeight: "32px"
    letterSpacing: "-0.025em"
  title:
    fontFamily: "Overpass Variable, Overpass, system-ui, sans-serif"
    fontSize: "20px"
    fontWeight: 700
    lineHeight: "26px"
    letterSpacing: "-0.015em"
  body:
    fontFamily: "Overpass Variable, Overpass, system-ui, sans-serif"
    fontSize: "16px"
    fontWeight: 400
    lineHeight: "24px"
  body-sm:
    fontFamily: "Overpass Variable, Overpass, system-ui, sans-serif"
    fontSize: "14px"
    fontWeight: 400
    lineHeight: "20px"
  label:
    fontFamily: "Overpass Variable, Overpass, system-ui, sans-serif"
    fontSize: "13px"
    fontWeight: 600
    lineHeight: "16px"
  caption:
    fontFamily: "Overpass Variable, Overpass, system-ui, sans-serif"
    fontSize: "12px"
    fontWeight: 500
    lineHeight: "16px"
  code:
    fontFamily: "Overpass Mono Variable, Overpass Mono, ui-monospace, monospace"
    fontSize: "13px"
    fontWeight: 400
    lineHeight: "20px"
    fontFeature: "tnum"
  numeral:
    fontFamily: "Overpass Variable, Overpass, system-ui, sans-serif"
    fontSize: "28px"
    fontWeight: 700
    lineHeight: "32px"
    letterSpacing: "-0.02em"
    fontFeature: "tnum"
rounded:
  none: "0"
  sm: "2px"
  md: "3px"
  full: "9999px"
spacing:
  xs: "4px"
  sm: "8px"
  md: "16px"
  lg: "24px"
  xl: "32px"
  2xl: "48px"
components:
  button-primary:
    backgroundColor: "{colors.ink}"
    textColor: "{colors.map-panel}"
    rounded: "{rounded.sm}"
    padding: "0 16px"
    height: "40px"
  button-primary-hover:
    backgroundColor: "{colors.ink-hover}"
  button-secondary:
    backgroundColor: "{colors.map-panel}"
    textColor: "{colors.ink}"
    rounded: "{rounded.sm}"
    padding: "0 16px"
    height: "40px"
  button-ghost:
    backgroundColor: "transparent"
    textColor: "{colors.ink-muted}"
    rounded: "{rounded.sm}"
  button-danger:
    backgroundColor: "transparent"
    textColor: "{colors.signal-danger}"
    rounded: "{rounded.sm}"
  input:
    backgroundColor: "{colors.map-panel}"
    textColor: "{colors.ink}"
    rounded: "{rounded.sm}"
    padding: "8px 12px"
    height: "40px"
  card:
    backgroundColor: "{colors.map-panel}"
    textColor: "{colors.ink}"
    rounded: "{rounded.none}"
    padding: "24px"
  console:
    backgroundColor: "{colors.map-sunk}"
    textColor: "{colors.ink}"
    typography: "{typography.code}"
    rounded: "{rounded.none}"
  roundel:
    textColor: "#ffffff"
    typography: "{typography.caption}"
    rounded: "{rounded.full}"
    size: "30px"
---

# Design System: Looking Glass

## Overview

**Creative North Star: "The Backbone Map"**

Carriers and exchanges draw their networks as transit diagrams. Looking Glass takes that grammar literally. Every Location is a line with its own colour and a roundel code (FRA, VIE, LON). A route is a sequence of stations on that line. The status of a Location is how its line is drawn: solid, dotted, or dashed. The interface is the printed map the network team keeps on the wall. It is quiet paper and ink, and colour appears only where there is network data.

The system is an instrument, not a showpiece. The visitor comes to operate: pick a Location, run a Method, read the raw output. So chrome stays neutral, dense, and square, and the operator's own logo and title sit on it without competing. Warmth comes from the line colours and the heavy Overpass headings, not from gradients or glow. Two looks are explicitly rejected: terminal cosplay (neon on black, CRT, traffic-light window dots) and SaaS gloss (glass, gradients, soft rounded card stacks).

**Key Characteristics:**

- Neutral map paper (light) or charcoal night map (dark). Ink is the only chrome colour.
- Location lines are the only saturated colour. Status signals (ok/warn/danger) are the only other hues.
- Square panels, 1px hairlines, 2px control radius, round roundels and stations.
- Overpass for everything a person reads. Overpass Mono only for tool output and data.
- One signature component: the route view, where hops are stations on the run's line.

## Colors

A restrained neutral ground carrying a full palette of transit-line colours that belong to data, never to chrome.

### Primary

- **Map Ink** (#16171a / night #ecece6): all text, primary button fill, focus rings, active navigation, the page-head rule. It is the voice of the interface.

### Secondary: Location lines

- **Line Red** (#c8102e / night #ff6b6b), **Line Blue** (#0b5cad / #5ea8ff), **Line Green** (#00783a / #45cf85), **Line Ochre** (#8a6500 / #f5c842), **Line Violet** (#6b3fa0 / #b48cff), **Line Teal** (#00747a / #35c6cb), **Line Orange** (#b34a0c / #ff9147), **Line Magenta** (#b0006d / #ff66b8), **Line Brown** (#7a4a1e / #d19a5f).
- Each Location gets one line colour, chosen by hashing its id (`lib/lines.ts`), so it keeps that colour on every surface.
- Light values carry white roundel text at ≥4.5:1. Night values carry ink roundel text.

### Tertiary: Signals

- **Signal Green** (#1d7a4a / #52c98b): Online, Active, run completed.
- **Signal Amber** (#9a5200 / #f5b453): Pending, lossy hop.
- **Signal Red** (#b3261e / #ff8a80): Offline, destructive actions, invalid input, failed runs.
- Each signal has a `-soft` tint for small filled tags and danger hover.

### Neutral

- **Map Paper** (#f3f3f0 / night #111214): page ground.
- **Panel White** (#ffffff / #18191c): raised surfaces, inputs, cards.
- **Sunk Grey** (#ebebe7 / #0b0c0d): insets, meaning the console, code blocks, command rows, and hover fills.
- **Muted Ink** (#595c63 / #a2a5ab): secondary text.
- **Faint Ink** (#8b8e94 / #6c6f76): placeholders and quiet icons. It is never body text.
- **Rule** (#dcdcd6 / #2a2c30): hairlines.
- **Strong Rule** (#a6a8a3 / #4a4d53): control borders.

### Named Rules

**The Line Owns the Colour Rule.** Saturated colour appears only on Location data: roundels, route lines, stations, the selected Location's band, and the editor's head rule. Buttons, links, navigation and focus are ink.

**The light-dark() Rule.** Every semantic token is one `light-dark()` value, so a subtree flips theme with `color-scheme` alone. `.dark` / `.light` classes do this (the settings previews render both themes side by side).

## Typography

**Display Font:** Overpass Variable (with Overpass, system-ui)
**Body Font:** Overpass Variable
**Mono Font:** Overpass Mono Variable (with ui-monospace)

**Character:** Overpass descends from Highway Gothic, the wayfinding face of road signage. Heavy weights read like station names, and the matching mono keeps raw tool output in the same family.

### Hierarchy

- **Display** (800, 40px/44px, -0.03em): page h1 at md+. It becomes **Display-sm** (800, 28px/32px) on small screens and in the auth terminus.
- **Title** (700, 20px/26px): section h2 (Output, Speed Tests, card titles, dialog titles).
- **Body** (400, 16px/24px): prose and ledes, capped near 60ch.
- **Body-sm** (400, 14px/20px): descriptions, notes, table text.
- **Label** (600, 13px/16px, sentence case): field labels, metric labels.
- **Caption** (500–700, 12px/16px): fact labels, column heads, legends, tags.
- **Code** (Overpass Mono 400, 13px/20px, tabular): console output, IPs, commands, tokens.
- **Numeral** (700, 28px/32px, tabular): run metrics. Speed readouts go to 40px/800.

### Named Rules

**The Mono Means Data Rule.** Mono appears only on tool output, addresses, commands, ports, ASNs and tokens, never as a "technical" costume for labels.

**The Sentence Case Rule.** No uppercase tracked labels, no eyebrows above headings.

## Layout

- Public page: max 1280px, 32px gutters (16px mobile), 48px section rhythm.
  - Order: title, a compact `routes` tab row of Location roundels, the selected Location's band (6px line along the top, command row, station facts grid), Output, run metrics, Speed Tests.
- Admin: 220px sidebar at md+, a menu drawer below md, content max 1040px.
  - Each page opens with `adminPageHead`: h1 and lede left, the single primary action right, a 3px ink rule beneath.
- Lists are ruled rows inside one panel with fixed column tracks, so every row aligns under a caption head row. Below lg, rows stack and each cell shows its own caption.
- Station facts use `repeat(auto-fill, minmax(140px, 1fr))`.
- Spacing sits on a 4px grid: 4, 8, 12, 16, 20, 24, 32, 40, 48.

## Elevation & Depth

The map is flat. Depth comes from tone: paper, then panel, then sunk inset. Hairline rules separate. Only floating layers (select menus, tooltips, dialogs, toasts, the mobile drawer) cast a shadow.

### Shadow Vocabulary

- **Popup** (`box-shadow: 0 1px 2px rgba(10,11,13,0.08), 0 12px 32px -8px rgba(10,11,13,0.28)`): anything that floats above the page.

### Named Rules

**The Flat Map Rule.** At-rest surfaces never cast a shadow. Glow is never used.

## Shapes

- Panels, cards, dialogs, the console and tables are square (0).
- Controls are barely softened (2px).
- Round: roundels, stations, the interchange-ring brand mark, and the theme toggle.
- Lines are thick and straight. Route lines are 6px, the editor head rule 6px, the Location band edge 6px, status segments 16×4px.
- Dialogs carry a 4px ink top edge.

## Components

### Buttons

- **Shape:** 2px radius. Heights: sm 32, md 40, lg 48, icon 36. Weight 700, 14px.
- **Primary:** solid ink with panel-white text. Use one per view, for the main action.
- **Secondary:** panel fill, 1px strong-rule border. The border goes ink on hover.
- **Ghost:** muted text, sunk fill on hover. **Danger:** red text, danger-soft fill on hover.
- **Focus:** 2px ink outline, 2px offset. Disabled buttons sit at 45% opacity.

### Status signal (StatusBadge)

- A 16×4px line segment before the text: solid green = Online/Active, solid amber = Pending, dotted red = Offline, dashed grey = Not enrolled.
- `lg` shows plain ink text for Location status. `sm` is a small tinted tag for roster states.

### Cards / Containers

- **Corner Style:** square. **Background:** panel. **Border:** 1px rule. **Padding:** 24px (28px in the editor panel). No nesting.

### Inputs / Fields

- **Style:** panel fill, 1px strong-rule border, 2px radius, 40px tall, 15px text. Labels are 13px/600 ink, above the control.
- **Focus:** the border goes ink plus a 1px inset ink ring. **Error:** danger border and ring, with danger text below (role=alert). **Disabled:** sunk fill.

### Navigation

- Header: 56px, with the operator logo or the interchange-ring mark and the site title. Nav links carry a 3px ink underline when active.
- Admin sidebar: sections drawn as stations on a vertical 3px line. The current section is a filled ink station.
- Location editor tabs (`stops`): stops on a horizontal line, the active stop filled.
- Public Location tabs (`routes`): roundel + name + ASN. The active tab is underlined 4px in its own line colour.

### The Route View (signature)

The mtr and traceroute output renders as a real table. The first column draws the run Location's line (6px) through each row, with a station at every hop:

- hollow ink ring per hop;
- larger terminus ring on the final hop;
- amber ring on a lossy hop;
- dotted line and a faint ring for a hop that answered no probe;
- a pulsing halo on the newest hop while traceroute still streams.

Rows arrive with a short station-in motion (mtr reveals them staggered at 60ms). A Route | Raw switch keeps the tool's own text one click away, and Copy output always copies the raw lines.

### Auth terminus

Login, install and activate place one square panel on the left third of the viewport. A 6px ink line runs in from the left edge and ends in an interchange ring on the panel's edge, level with the heading.

## Do's and Don'ts

### Do

- **Do** put `data-line` + `lineStyle(id)` on any element that shows a Location, and lead it with its roundel (`lineCode(name)`).
- **Do** use ruled rows inside one panel for collections, with fixed column tracks.
- **Do** keep raw tool output intact and copyable. Visualise it only alongside a Raw view.
- **Do** use `light-dark()` semantic tokens only. Flip a subtree with `.dark` / `.light`.
- **Do** keep one primary (ink) action per view.

### Don't

- **Don't** use neon-on-black, CRT effects, glow, or macOS traffic-light window dots (rejected as terminal cosplay).
- **Don't** use gradients, glass, or soft rounded card grids (rejected as SaaS gloss).
- **Don't** colour chrome (buttons, links, headings, focus) with a line or signal colour.
- **Don't** add uppercase tracked labels, eyebrows over headings, or monospace labels.
- **Don't** cast shadows from at-rest surfaces.
