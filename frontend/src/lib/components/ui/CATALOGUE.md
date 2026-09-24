# UI Primitive Catalogue

The design-system foundation for the Looking Glass frontend. Build pages **strictly**
from these primitives, the semantic tokens, and the layout patterns below. Styling is
**Panda CSS** (atomic classes via `css()` + recipes); behaviour primitives are **Ark UI**
(`@ark-ui/svelte`); icons are **Material Symbols** compiled to inline SVG at build time
(`~icons/material-symbols/*`). Tailwind / clsx / tailwind-merge / tailwind-variants /
Lucide are gone — do not import them.

- Fonts are self-hosted and loaded globally in `routes/+layout.svelte`:
  **Plus Jakarta Sans Variable** (UI, token `sans`) and **JetBrains Mono Variable**
  (code, token `mono`).
- Dark mode is a `.dark` class on `<html>`, set by the pre-paint script in `app.html`
  and toggled by `$lib/theme.svelte.ts`. **Always use semantic colour tokens** (they
  carry both light and dark values) — never hardcode a hex, or one theme breaks.
- `prefers-reduced-motion` is handled globally (animations/transitions collapse to
  ~0). Don't re-implement it per component.

---

## Styling conventions (read this first)

```ts
import { css, cx } from 'styled-system/css';      // css() -> class string; cx() joins
import { button /* , card, dialog, … */ } from 'styled-system/recipes';
```

- `css({ … })` returns a single class string. Compose/override with `cx(a, b)`
  (later wins). Every component here accepts a `class` prop that is merged last.
- **Values must be static literals.** Panda extracts at build time by scanning source,
  so `css({ color: someRuntimeVar })` produces nothing. For a value that varies at
  runtime, either use a **recipe** (variants are pre-generated) or write **literal
  branches**: `const c = tone === 'danger' ? css({ color: 'error' }) : css({ color: 'primary' })`.
- **One-off `css()` calls must live in a `.ts` module, not inline in `.svelte`.**
  Panda's extractor cannot read `css()` out of Svelte templates, so put one-off
  class strings in `src/lib/styles.ts` (already populated for the shells +
  primitives) and import the exported constant into the component. Recipe calls
  (`button(...)`, `card()`, …) inside `.svelte` ARE fine — recipes are pre-generated
  via `staticCss`, so they don't depend on source extraction.
- Responsive: nest a breakpoint object. Breakpoints are `base`(implicit) `sm:640`
  `md:768` `lg:1024` `xl:1280` `2xl:1536`.

  ```ts
  css({ padding: '16px', md: { padding: '32px' } })
  ```

- Recipes/slot-recipes already exist for every primitive (see each entry). Use the
  component; only reach for a raw recipe when styling a bare element (e.g. a download
  `<a>` via `buttonVariants`).

### Semantic colour tokens

`base` = light scheme, `_dark` = dark scheme; reference by the bare name
(e.g. `background: 'surface-container-low'`).

| Token | Light | Dark | Use |
| --- | --- | --- | --- |
| `background` / `surface` | #f8f9ff | #0b1326 | page canvas / header |
| `surface-container-lowest` | #ffffff | #060e20 | deepest inset, console bg |
| `surface-container-low` | #eff4ff | #131b2e | cards, sidebar |
| `surface-container` | #e8ecf8 | #171f33 | metric/section cards |
| `surface-container-high` | #e2e7f2 | #222a3d | inputs, raised rows |
| `surface-container-highest` | #dce1ec | #2d3449 | popovers, hover fill |
| `surface-variant` | #e1e2ec | #2d3449 | neutral chips, switch track |
| `surface-bright` | #f8f9ff | #31394d | subtle inner borders |
| `surface-dim` | #cbdbf5 | #0b1326 | recessed wells |
| `on-surface` / `on-surface-variant` | #191c20 / #44474e | #dae2fd / #bbcabf | primary / secondary text |
| `outline` / `outline-variant` | #757780 / #c4c6d0 | #86948a / #3c4a42 | strong / hairline borders |
| `primary` / `on-primary` | #10b981 / #fff | #4edea3 / #003824 | accent, active nav, focus |
| `primary-container` / `on-primary-container` | #6ffbbe / #002114 | #10b981 / #00422b | primary button fill/text |
| `primary-fixed` / `primary-fixed-dim` | #6ffbbe / #4edea3 | same | console success text, IP values |
| `secondary-container` / `on-secondary-container` | #dce2f9 / #151b2c | #0b513d / #83c2a9 | active sidebar item |
| `tertiary` | #81429f | #45dfa4 | speed-test upload bar |
| `error` / `on-error` | #ba1a1a / #fff | #ffb4ab / #690005 | destructive, invalid |
| `error-container` / `on-error-container` | #ffdad6 / #410002 | #93000a / #ffdad6 | danger hover fill |
| `warning` | #d97706 | #fbbf24 | PENDING state (not in Stitch palette) |
| `inverse-surface` / `inverse-on-surface` | #2e3036 / #eff4ff | #dae2fd / #283044 | theme-preview cards |

### Other tokens

- Fonts: `sans`, `mono`. Radii: `sm`4 `md`8 (base) `lg`12 `xl`16 `full`.
- Shadows: `glow` (status dot), `glow-md` (primary button), `popup` (menus/dialogs).
- Animations: `spin`, `fade-in`, `content-in` — use `css({ animation: 'spin' })`.
- Text styles (`css({ textStyle: '…' })`): `headline-lg` 48/56·700,
  `headline-md` 32/40·600, `headline-sm` 24/32·600, `headline-mobile` 32/40·700,
  `body-lg` 18/28, `body-md` 16/24 (body default), `body-sm` 14/20,
  `label-md` 14/20·600·+0.05em, `label-sm` 12/16·600·+0.05em,
  `mono-data` 14/22 JetBrains Mono.
- Spacing: use raw px on the design's 4px grid — common steps 4·8·12·16·24·32·48·64·80.

### Layout patterns (from the Stitch exports)

- **Page header**: `<h1>` in `headline-lg` (mobile `headline-mobile`) on `on-surface`
  - a `body-lg` `on-surface-variant` subtitle; actions row on the right at `md`.
- **Section card**: `background: 'surface-container-low'`, `borderWidth:'1px'`
  `borderColor:'outline-variant'`, `borderRadius:'xl'`, `padding:'24px'`. Admin section
  headings use `headline-sm` in `primary`. (This is exactly the `card` recipe / `<Card>`.)
- **Control panel / metric card**: `background:'surface-container'`, same border, radius
  `xl`, padding `24px`. Metrics grid: `grid` `grid-cols-2 md:grid-cols-4` gap `24px`.
- **Data table**: wrap in a `borderRadius:'lg'` `border` `outline-variant` scroller;
  header row `background:'surface-container'`, cells `label-sm` uppercase
  `on-surface-variant`; body rows `borderBottom` `surface-bright`, hover
  `background:'surface-container-high'`; IP/host values in `mono` `primary-fixed-dim`.
- **Content width**: public diagnostics max `1200px`; admin content max `~1024px`
  (`max-w-4xl/5xl`), centred, page padding `16px` → `md:32px`.

### Icons

```svelte
<script lang="ts">
  import Search from '~icons/material-symbols/search';
</script>
<Search aria-hidden="true" />
```

Kebab-case Material Symbols names. Sized via CSS (`& svg { width; height }` on the
parent, or a `class`). Common: `search` `add` `edit` `delete` `close` `check`
`content-copy` `expand-more` `arrow-back` `refresh` `progress-activity` (spinner)
`check-circle` `error` `info` `warning` `location-on` `group` `settings` `logout`
`menu` `light-mode` `dark-mode` `download` `upload` `open-in-new` `link-off` `vpn-key`
`terminal` `speed` `public` `lan` `router` `schedule` `visibility` `visibility-off`.

---

## Primitives

### Button — `$lib/components/ui/button/index.js`

`import { Button, buttonVariants } from '$lib/components/ui/button/index.js'`
Variants `primary|secondary|ghost|danger` (default `primary`); sizes `sm|md|lg|icon`
(default `md`); `loading` shows a spinner + sets `aria-busy` + disables.

```svelte
<Button onclick={save} loading={saving}>Save location</Button>
<Button variant="secondary" size="sm">Edit</Button>
<Button variant="danger" size="icon" aria-label="Delete"><Trash /></Button>
<!-- non-button element styled as a button: -->
<a class={buttonVariants({ variant: 'secondary', size: 'sm' })} href={url} download>Download</a>
```

### Input — `$lib/components/ui/input/index.js`

`import { Input } from '$lib/components/ui/input/index.js'`
`bind:value`, `invalid` (error border + `aria-invalid`), `mono` (JetBrains face for
IPs/ports/CIDR), plus all native `<input>` attrs.

```svelte
<Input id="asn" bind:value={asn} mono invalid={!!asnError} aria-describedby="asn-error" />
```

### Textarea — `$lib/components/ui/textarea.svelte` (default)

Same props as Input (`bind:value`, `invalid`, `mono`) + native `<textarea>` attrs (`rows`).

```svelte
<Textarea bind:value={customBlock} rows={4} />
```

### Label — `$lib/components/ui/label/index.js`

`import { Label } from '$lib/components/ui/label/index.js'` — styled `<label>`
(`label-md`, `on-surface-variant`). Pass `for`.

### Field — `$lib/components/ui/field.svelte` (default)

Vertical label → control → message stack. `label`, `for`, `error` (role=alert, wins)
or `hint`; the control is `children`.

```svelte
<Field label="Geographic label" for="geo" error={geoError}>
  <Input id="geo" bind:value={geo} />
</Field>
```

### Card — `$lib/components/ui/card/index.js`

`import { Card, CardHeader, CardTitle, CardDescription, CardContent, CardFooter } from '$lib/components/ui/card/index.js'`
`Card` is the section-card surface; `CardTitle` renders an `<h2>`. All accept `class`.

```svelte
<Card>
  <CardHeader>
    <CardTitle>Branding</CardTitle>
    <CardDescription>Site identity shown in the header and footer.</CardDescription>
  </CardHeader>
  <CardContent>…fields…</CardContent>
  <CardFooter><Button>Save</Button></CardFooter>
</Card>
```

### Badge / StatusBadge — `$lib/components/ui/badge.svelte`, `status-badge.svelte` (defaults)

`tone: 'neutral'|'success'|'warning'|'danger'` (default `neutral`). `StatusBadge`
prefixes a coloured dot (used for Online/Offline/Not-enrolled, Active/Pending).

```svelte
import StatusBadge from '$lib/components/ui/status-badge.svelte';
<StatusBadge tone="success">Online</StatusBadge>
<StatusBadge tone="danger">Offline</StatusBadge>
<StatusBadge tone="neutral">Not enrolled</StatusBadge>
<Badge tone="warning">Pending</Badge>
```

### Select — `$lib/components/ui/select.svelte` (default)

Ark listbox (typeahead, arrow keys, ARIA). `items: { label, value }[]` (**stable
array** — hoist it or `$derived`, don't inline a fresh literal each render),
`bind:value` (the selected `value` string), `placeholder`, `disabled`, `invalid`,
`name`, `id`, `aria-label`. Pair with an external `<Label for={id}>`.

```svelte
const methods = $derived(offered.map((m) => ({ label: m, value: m })));
<Select id="method" items={methods} bind:value={method} placeholder="Select method" />
```

### Tabs — `$lib/components/ui/tabs.svelte` (default)

Ark Tabs machine → arrow-key nav + `role=tablist/tab` + `aria-selected` for free.
This is a **controlled tablist only**: `tabs: { id, label }[]`, `bind:active` (the id).
Render panels yourself keyed off `active` (so the active tab can live in a URL query).

```svelte
<Tabs tabs={[{ id: 'settings', label: 'Settings' }, { id: 'methods', label: 'Methods' }]} bind:active />
{#if active === 'settings'} …panel… {/if}
```

### Dialog / ConfirmDialog — `$lib/components/ui/dialog.svelte`, `confirm-dialog.svelte` (defaults)

Ark Dialog (focus trap, Escape, inert backdrop, portal). `bind:open`, `title`,
`description?`, `preventClose?`, `onclose?`, `children`.

```svelte
<Dialog bind:open={showCreate} title="Add location" description="Name it; configure next.">
  <form onsubmit={submit}> …fields… <Button type="submit">Create</Button></form>
</Dialog>

<ConfirmDialog bind:open={askDelete} title="Delete location?" message="This cannot be undone."
  confirmLabel="Delete" danger busy={deleting} onconfirm={confirmDelete} />
```

### CheckboxCard — `$lib/components/ui/checkbox-card.svelte` (default)

Ark Checkbox as a whole-card control for the Methods grid. `bind:checked`, `disabled`,
`name`, `value`, `label` (string, rendered mono).

```svelte
<div class={grid}>
  {#each methods as m (m)}
    <CheckboxCard label={m} bind:checked={enabled[m]} name="methods" value={m} />
  {/each}
</div>
```

### Tooltip — `$lib/components/ui/tooltip.svelte` (default)

Ark Tooltip (hover/focus/Escape, portal). `content` (string); `children` is the trigger
(must be focusable — wrap an icon in a `<button>`/`<span tabindex=0>` if needed).

```svelte
<Tooltip content="Average round-trip time across all replies.">
  <span tabindex="0"><Info aria-hidden="true" /></span>
</Tooltip>
```

### Collapsible — `$lib/components/ui/collapsible.svelte` (default)

Ark Collapsible disclosure. `bind:open`, `trigger` (snippet, the always-visible
control), `children` (the body).

```svelte
<Collapsible bind:open={showMore}>
  {#snippet trigger()}<Button variant="ghost" size="sm">More test IPs</Button>{/snippet}
  <ul>…extra IPs…</ul>
</Collapsible>
```

### CopyButton — `$lib/components/ui/copy-button.svelte` (default)

Copy-to-clipboard with a transient "copied" check + polite live region.
`text` (string to copy), `label` (default `'Copy'`, used in the aria-label). Uses
`navigator.clipboard` directly.

```svelte
<CopyButton text={installCommand} label="install command" />
```

### Toast — `$lib/toast.svelte.js` (API) + `$lib/components/ui/toaster.svelte` (renderer)

The `<Toaster>` is mounted once in `routes/+layout.svelte`; pages just call the API.

```ts
import { toast } from '$lib/toast.svelte.js';
toast.success('Settings saved.');
toast.error('Could not save.');
```

### ThemeToggle — `$lib/components/theme-toggle.svelte` (default)

No props. Toggles light/dark, persists to `localStorage['theme']`, renders the
sun/moon icon. Already placed in the public header.

---

## Shells (already built — don't recreate)

- **Public header/footer**: `routes/+layout.svelte`. Header = status dot + site
  title/logo (left), Diagnostics / Administration nav + ThemeToggle (right;
  Administration only when `GET /api/admin/me` succeeds). Footer = Terms link +
  custom content block from public settings. Auth routes (`/login`, `/install`,
  `/activate*`) render bare (no header/footer) for a centred card.
- **Admin shell**: `routes/admin/+layout.svelte`. Fail-closed session gate, persistent
  sidebar at `md+` (heading "Administration / Network Settings"; Locations,
  Administrators, Settings; Log out pinned bottom; **Locations is active on
  `/admin` and `/admin/locations/*`**), and a left **drawer behind a menu button
  below `md`** (Ark Dialog). Admin pages render into the content area only — do not
  add your own header/sidebar.
