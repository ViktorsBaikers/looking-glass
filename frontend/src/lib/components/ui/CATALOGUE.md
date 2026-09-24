# UI Primitive Catalogue

The design-system foundation for the Looking Glass frontend. Build pages **strictly**
from these primitives, the semantic tokens, and the layout patterns below. Styling is
**Panda CSS** (atomic classes via `css()` + recipes); behaviour primitives are **Ark UI**
(`@ark-ui/svelte`); icons are **Material Symbols** compiled to inline SVG at build time
(`~icons/material-symbols/*`). Tailwind / clsx / tailwind-merge / tailwind-variants /
Lucide are gone — do not import them.

- Fonts are self-hosted and loaded globally in `routes/+layout.svelte`:
  **Overpass Variable** (UI, token `sans`) and **Overpass Mono Variable** (tool
  output and data only, token `mono`).
- Dark mode is a `.dark` class on `<html>`, set by the pre-paint script in `app.html`
  and toggled by `$lib/theme.svelte.ts`. Every semantic token is a `light-dark()`
  value, so any subtree can flip scheme with a `dark` / `light` class (the settings
  previews do). **Always use semantic colour tokens**, never a hex.
- The visual world, "Backbone map", is recorded in `/DESIGN.md`.
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
  branches**: `const c = tone === 'danger' ? css({ color: 'danger' }) : css({ color: 'ink' })`.
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

Reference by bare name (e.g. `background: 'panel'`).

| Token | Light | Dark | Use |
| --- | --- | --- | --- |
| `paper` | #f3f3f0 | #111214 | page ground |
| `panel` | #ffffff | #18191c | raised surfaces, cards, inputs |
| `sunk` | #ebebe7 | #0b0c0d | insets: console, code blocks, hover fill |
| `ink` / `ink-hover` | #16171a / #33353b | #ecece6 / #ffffff | text, primary button fill, focus ring |
| `ink-muted` | #595c63 | #a2a5ab | secondary text |
| `ink-faint` | #8b8e94 | #6c6f76 | placeholders, quiet icons (non-body text) |
| `on-ink` | #ffffff | #111214 | text on ink fills |
| `rule` / `rule-strong` | #dcdcd6 / #a6a8a3 | #2a2c30 / #4a4d53 | hairlines / control borders |
| `ok` / `ok-soft` | #1d7a4a / #e3f1e8 | #52c98b / #15291e | Online, Active, success |
| `warn` / `warn-soft` | #9a5200 / #f8ecd9 | #f5b453 / #2e2312 | Pending, lossy hop |
| `danger` / `danger-soft` | #b3261e / #f9e3e1 | #ff8a80 / #331a19 | destructive, invalid, Offline |

**Location lines.** Colour belongs to Location data only. An element that draws a
Location carries `data-line` + `style={lineStyle(location.id)}` (`$lib/lines.ts`);
inside it `var(--line)` / `var(--line-ink)` resolve per theme. Roundel text is
`lineCode(location.name)`.

### Other tokens

- Radii: `none` 0 (panels, cards, dialogs), `sm` 2 (controls), `md` 3, `full`.
- Shadow: `popup` (menus/dialogs/toasts) only.
- Animations: `spin`, `fade-in`, `content-in`, `station-in`, `here-pulse`.
- Text styles: `display` 40/44·800, `display-sm` 28/32·800, `title` 20/26·700,
  `body` 16/24, `body-sm` 14/20, `label` 13/16·600, `caption` 12/16·500,
  `code` mono 13/20 tabular, `numeral` 28/32·700 tabular.
- Spacing: 4px grid — 4·8·12·16·20·24·32·40·48.

### Layout patterns

- **Admin page**: `adminPage` wrapper, `adminPageHead` (h1 `adminTitle` + `adminLede`
  left, one primary action right, 3px ink rule under), then sections. All in `$lib/styles.ts`.
- **Status**: `StatusBadge` draws a 16×4 line segment. `success` solid, `danger`
  dotted, `warning` solid warn, `neutral` dashed. Size `lg` for Location status.
- **Lists**: ruled rows inside one square `panel` (hairline `rule` between rows);
  never a grid of cards.
- **Sequential sections**: `Tabs` variant `stops` (stops on a line). Location
  choice: `Tabs` variant `routes` with `{ code, line }` per item.
- **Content width**: public `1280px`; admin content `1040px`.

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
`bind:value`, `invalid` (error border + `aria-invalid`), `mono` (Overpass Mono for
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
prefixes a coloured dot (used for Online/Offline/Not-enrolled, Active/Pending) and
takes `size: 'sm'|'lg'` (default `sm`, uppercase tag; `lg` is the sentence-case
Locations card pill).

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

- **Public header/footer**: `routes/+layout.svelte`. Header = operator logo or
  interchange-ring mark + site title (left), Diagnostics / Administration nav + ThemeToggle (right;
  Administration only when `GET /api/admin/me` succeeds). Footer = Terms link +
  custom content block from public settings (hidden when both are empty). Auth
  routes (`/login`, `/install`, `/activate*`) render bare: a terminus panel.
- **Admin shell**: `routes/admin/+layout.svelte`. Fail-closed session gate, persistent
  sidebar at `md+` (sections as stations on a vertical line: Locations,
  Administrators, Settings; Log out pinned bottom; **Locations is active on
  `/admin` and `/admin/locations/*`**), and a left **drawer behind a menu button
  below `md`** (Ark Dialog). Admin pages render into the content area only — do not
  add your own header/sidebar.
