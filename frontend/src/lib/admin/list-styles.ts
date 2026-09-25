// One-off atomic styles for the admin Locations list page
// (routes/admin/+page.svelte). Panda cannot extract css() from .svelte, so the
// class constants live here; recipes (button/card/…) stay inline in the page.
import { css } from 'styled-system/css';

// ----- toolbar: search, sort, the line legend -----
export const toolbar = css({
	display: 'flex',
	flexDirection: 'column',
	gap: '12px',
	md: { flexDirection: 'row', alignItems: 'center' }
});
export const searchField = css({ flex: '1', minWidth: '0' });
export const searchWrap = css({
	position: 'relative',
	'& > svg': {
		position: 'absolute',
		left: '12px',
		top: '50%',
		transform: 'translateY(-50%)',
		width: '18px',
		height: '18px',
		color: 'ink-faint',
		pointerEvents: 'none'
	}
});
export const searchInput = css({ paddingInlineStart: '40px' });
export const sortField = css({
	display: 'flex',
	alignItems: 'center',
	gap: '10px',
	'& > label': { whiteSpace: 'nowrap', color: 'ink-muted' },
	'& > div': { width: '160px' }
});
export const legend = css({
	display: 'flex',
	flexWrap: 'wrap',
	gap: '6px 18px',
	textStyle: 'caption',
	fontWeight: 600,
	color: 'ink-muted'
});
export const legendItem = css({ display: 'inline-flex', alignItems: 'center', gap: '8px' });
export const reorderNote = css({ textStyle: 'body-sm', color: 'ink-muted', marginBottom: '12px' });

// ----- the line index -----
export const list = css({
	position: 'relative',
	listStyle: 'none',
	background: 'panel',
	borderWidth: '1px',
	borderStyle: 'solid',
	borderColor: 'rule'
});
// Fixed tracks so every row's cells line up under the head row. The first
// track widens when rows carry a reorder handle (see `reorderable`); the
// methods track fits its longest label ("8 methods offered", the cap) tightly.
const columns = 'var(--mark-track, 56px) minmax(0, 1fr) 124px 96px 128px 292px';
export const headRow = css({
	display: 'none',
	lg: {
		display: 'grid',
		gridTemplateColumns: columns,
		gap: '16px',
		padding: '12px 20px',
		textStyle: 'key',
		color: 'ink-muted',
		borderBottomWidth: '1px',
		borderBottomStyle: 'solid',
		borderBottomColor: 'rule'
	}
});
export const row = css({
	display: 'grid',
	gridTemplateColumns: 'var(--mark-track, 56px) minmax(0, 1fr)',
	gap: '12px 16px',
	alignItems: 'center',
	padding: '18px 16px',
	transitionProperty: 'background',
	transitionDuration: '120ms',
	'& + &': { borderTopWidth: '1px', borderTopStyle: 'solid', borderTopColor: 'rule' },
	_hover: { background: 'color-mix(in srgb, {colors.sunk} 45%, transparent)' },
	lg: { gridTemplateColumns: columns, padding: '16px 20px' }
});
/** Wraps the head row and list while rows can be dragged into public order. */
export const reorderable = css({ '--mark-track': '84px' });
/** The row being dragged floats above the list: ink-edged so it reads in both
 *  themes (a shadow alone vanishes on the night map). */
export const rowDragging = css({
	position: 'relative',
	zIndex: 1,
	background: 'panel',
	boxShadow: 'popup',
	outline: '1px solid {colors.ink}',
	outlineOffset: '-1px',
	_hover: { background: 'panel' }
});
/** While a drag is live the whole list shows the grabbing hand. */
export const listDragging = css({ cursor: 'grabbing', userSelect: 'none', '& *': { cursor: 'grabbing' } });
/** Grip before the roundel: drag it, or focus it and use the arrow keys. */
export const dragHandle = css({
	display: 'inline-flex',
	alignItems: 'center',
	justifyContent: 'center',
	width: '24px',
	height: '40px',
	marginInlineEnd: '4px',
	flexShrink: 0,
	borderRadius: 'sm',
	color: 'ink-faint',
	cursor: 'grab',
	touchAction: 'none',
	transitionProperty: 'color, background',
	transitionDuration: '120ms',
	_hover: { color: 'ink', background: 'sunk' },
	_focusVisible: { outline: '2px solid {colors.ink}', outlineOffset: '1px', color: 'ink' },
	'&[data-dragging]': { cursor: 'grabbing', color: 'ink', background: 'sunk' },
	'& svg': { width: '20px', height: '20px' }
});
/** Roundel plus a short stub of the Location's line, drawn in its state. */
export const lineMark = css({
	display: 'flex',
	alignItems: 'center',
	gridRow: 'span 4',
	alignSelf: 'start',
	lg: { gridRow: 'auto', alignSelf: 'center' }
});
export const roundel = css({
	display: 'inline-flex',
	alignItems: 'center',
	justifyContent: 'center',
	width: '32px',
	height: '32px',
	borderRadius: 'full',
	background: 'var(--line)',
	color: 'var(--line-ink)',
	fontSize: '10px',
	fontWeight: 800,
	letterSpacing: '0.02em',
	flexShrink: 0,
	position: 'relative',
	zIndex: 1
});
export const stub = css({
	width: '22px',
	height: '0',
	marginLeft: '-2px',
	borderTopWidth: '6px',
	borderTopStyle: 'solid',
	borderTopColor: 'var(--line)'
});
export const stubState = {
	online: css({}),
	offline: css({ borderTopStyle: 'dotted', opacity: 0.6 }),
	not_enrolled: css({ borderTopStyle: 'dashed', opacity: 0.6 })
} as const;
export const nameCell = css({ minWidth: '0', gridColumn: '2', lg: { gridColumn: 'auto' } });
export const rowTitle = css({
	textStyle: 'item',
	color: 'ink'
});
export const metaRow = css({
	display: 'flex',
	flexWrap: 'wrap',
	alignItems: 'center',
	gap: '4px 8px',
	marginTop: '2px',
	textStyle: 'body-sm',
	color: 'ink-muted'
});
export const cell = css({
	gridColumn: '2',
	display: 'flex',
	alignItems: 'baseline',
	gap: '8px',
	fontSize: '14px',
	lineHeight: '20px',
	color: 'ink',
	minWidth: '0',
	fontVariantNumeric: 'tabular-nums',
	lg: { gridColumn: 'auto', display: 'block' }
});
/** Per-cell label: visible on narrow rows, read-only for screen readers on wide ones. */
export const cellLabel = css({
	textStyle: 'key',
	color: 'ink-muted',
	minWidth: '84px',
	lg: {
		position: 'absolute',
		width: '1px',
		height: '1px',
		overflow: 'hidden',
		clip: 'rect(0,0,0,0)',
		whiteSpace: 'nowrap'
	}
});
export const quietDelete = css({ color: 'ink-faint', _hover: { color: 'danger' } });
export const actions = css({
	gridColumn: '2',
	display: 'flex',
	alignItems: 'center',
	gap: '6px',
	flexWrap: 'wrap',
	lg: { gridColumn: 'auto', justifyContent: 'flex-end', flexWrap: 'nowrap' }
});

// ----- empty / loading / error states -----
export const stateCard = css({
	display: 'flex',
	flexDirection: 'column',
	alignItems: 'flex-start',
	gap: '16px',
	padding: '40px 24px',
	background: 'panel',
	borderWidth: '1px',
	borderStyle: 'dashed',
	borderColor: 'rule-strong'
});
export const stateText = css({ textStyle: 'body', color: 'ink-muted' });
export const errorCard = css({
	display: 'flex',
	flexWrap: 'wrap',
	alignItems: 'center',
	justifyContent: 'space-between',
	gap: '16px',
	padding: '20px 24px',
	background: 'danger-soft',
	color: 'danger',
	fontWeight: 600
});
export const skeleton = css({
	height: '76px',
	background: 'panel',
	animation: 'fade-in 900ms ease-in-out infinite alternate',
	'& + &': { borderTopWidth: '1px', borderTopStyle: 'solid', borderTopColor: 'rule' }
});
export const skeletonList = css({ borderWidth: '1px', borderStyle: 'solid', borderColor: 'rule' });
export const formStack = css({ display: 'flex', flexDirection: 'column', gap: '18px' });
export const formError = css({ textStyle: 'body-sm', color: 'danger', fontWeight: 600 });
