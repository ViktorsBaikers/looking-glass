// One-off atomic styles for the Location editor (issue #7). Panda's extractor
// reads css() from .ts only — never inline these in .svelte (see CATALOGUE.md).
import { css } from 'styled-system/css';

// ----- page head: roundel, name, state; the Location's line underneath -----
export const backLink = css({
	display: 'inline-flex',
	alignItems: 'center',
	gap: '6px',
	fontSize: '14px',
	fontWeight: 600,
	color: 'ink-muted',
	textDecoration: 'none',
	marginBottom: '24px',
	_hover: { color: 'ink' },
	_focusVisible: { outline: '2px solid {colors.ink}', outlineOffset: '2px' },
	'& svg': { width: '16px', height: '16px' }
});
export const pageHead = css({
	display: 'flex',
	alignItems: 'center',
	gap: '16px',
	paddingBottom: '24px',
	marginBottom: '32px',
	borderBottomWidth: '6px',
	borderBottomStyle: 'solid',
	borderBottomColor: 'var(--line)'
});
export const headRoundel = css({
	display: 'inline-flex',
	alignItems: 'center',
	justifyContent: 'center',
	width: '44px',
	height: '44px',
	borderRadius: 'full',
	background: 'var(--line)',
	color: 'var(--line-ink)',
	fontSize: '13px',
	fontWeight: 800,
	letterSpacing: '0.02em',
	flexShrink: 0,
	md: { width: '56px', height: '56px', fontSize: '15px' }
});
export const pageTitle = css({ textStyle: 'display-sm', color: 'ink', md: { textStyle: 'display' } });
export const pageSub = css({
	display: 'flex',
	alignItems: 'center',
	flexWrap: 'wrap',
	gap: '6px 12px',
	marginTop: '6px',
	textStyle: 'body-sm',
	color: 'ink-muted'
});

// ----- the tab panel -----
export const panelCard = css({
	background: 'panel',
	borderWidth: '1px',
	borderStyle: 'solid',
	borderColor: 'rule',
	padding: '20px',
	marginTop: '28px',
	md: { padding: '28px' }
});
export const panelHead = css({
	display: 'flex',
	alignItems: 'flex-start',
	justifyContent: 'space-between',
	gap: '16px',
	marginBottom: '20px'
});
export const panelTitle = css({ textStyle: 'title', color: 'ink', marginBottom: '4px' });
export const panelDesc = css({ textStyle: 'body-sm', color: 'ink-muted', maxWidth: '60ch' });

// ----- forms -----
export const formStack = css({
	display: 'grid',
	gridTemplateColumns: '1fr',
	gap: '20px',
	maxWidth: '720px',
	md: { gridTemplateColumns: '1fr 1fr', gap: '20px 24px' },
	// Fields span both columns unless they sit in a formRow2 pair.
	'& > *': { md: { gridColumn: '1 / -1' } }
});
export const formRow2 = css({
	display: 'grid',
	gridTemplateColumns: '1fr',
	md: { gridTemplateColumns: '1fr 1fr' },
	gap: '20px 24px'
});
export const saveRow = css({
	paddingTop: '20px',
	borderTopWidth: '1px',
	borderTopStyle: 'solid',
	borderTopColor: 'rule'
});

// ----- data tables -----
export const tableScroller = css({ overflowX: 'auto' });
export const table = css({ width: '100%', textAlign: 'left', borderCollapse: 'collapse' });
export const th = css({
	padding: '0 12px 10px',
	textStyle: 'caption',
	fontWeight: 700,
	color: 'ink-muted',
	whiteSpace: 'nowrap',
	borderBottomWidth: '1px',
	borderBottomStyle: 'solid',
	borderBottomColor: 'ink',
	'&:first-child': { paddingLeft: '0' }
});
export const thActions = css({ width: '96px', textAlign: 'right' });
export const td = css({
	padding: '12px',
	fontSize: '14px',
	lineHeight: '20px',
	color: 'ink',
	borderBottomWidth: '1px',
	borderBottomStyle: 'solid',
	borderBottomColor: 'rule',
	'&:first-child': { paddingLeft: '0' }
});
export const tdMono = css({ fontFamily: 'mono', fontSize: '13px', fontVariantNumeric: 'tabular-nums' });
export const tdMuted = css({ color: 'ink-muted' });
export const trHover = css({ _hover: { '& > td': { background: 'color-mix(in srgb, {colors.sunk} 45%, transparent)' } } });
export const tdActions = css({ textAlign: 'right', whiteSpace: 'nowrap', paddingRight: '0' });
export const rowAction = css({
	display: 'inline-flex',
	padding: '6px',
	borderRadius: 'sm',
	color: 'ink-muted',
	_hover: { color: 'ink', background: 'sunk' },
	'& svg': { width: '18px', height: '18px' }
});
export const rowActionDanger = css({
	_hover: { color: 'danger', background: 'danger-soft' }
});
export const emptyWell = css({
	borderWidth: '1px',
	borderStyle: 'dashed',
	borderColor: 'rule-strong',
	background: 'paper',
	padding: '32px',
	textAlign: 'center',
	textStyle: 'body-sm',
	color: 'ink-muted'
});

// ----- methods grid -----
export const methodsIntro = css({ textStyle: 'body-sm', color: 'ink-muted', marginBottom: '24px' });
export const familyHead = css({ textStyle: 'label', color: 'ink', marginBottom: '10px' });
export const familyGroup = css({ marginBottom: '24px' });
export const methodsGrid = css({
	display: 'grid',
	gridTemplateColumns: 'repeat(2, minmax(0, 1fr))',
	md: { gridTemplateColumns: 'repeat(4, minmax(0, 1fr))' },
	gap: '8px'
});

// ----- enrollment -----
export const enrollIntro = css({ textStyle: 'body', color: 'ink-muted', marginBottom: '20px', maxWidth: '64ch' });
export const cmdBox = css({
	position: 'relative',
	background: 'sunk',
	padding: '16px',
	paddingRight: '56px',
	fontFamily: 'mono',
	fontSize: '13px',
	lineHeight: '21px',
	color: 'ink',
	overflowX: 'auto',
	whiteSpace: 'pre'
});
export const cmdCopy = css({ position: 'absolute', top: '8px', right: '8px' });
export const enrollRow = css({
	display: 'flex',
	alignItems: 'center',
	justifyContent: 'space-between',
	flexWrap: 'wrap',
	gap: '12px 16px',
	paddingTop: '16px',
	marginTop: '16px',
	borderTopWidth: '1px',
	borderTopStyle: 'solid',
	borderTopColor: 'rule',
	textStyle: 'body-sm'
});
export const countdownText = css({ color: 'ink-muted' });
export const countdownMono = css({ fontFamily: 'mono', fontVariantNumeric: 'tabular-nums', color: 'ink', fontWeight: 600 });
export const expiredText = css({ color: 'danger', fontWeight: 600 });
export const connectedRow = css({
	display: 'flex',
	alignItems: 'center',
	gap: '8px',
	marginTop: '16px',
	textStyle: 'body-sm'
});
export const connectedOk = css({
	color: 'ok',
	fontWeight: 700,
	display: 'flex',
	alignItems: 'center',
	gap: '8px',
	'& svg': { width: '16px', height: '16px' }
});
export const waitingMuted = css({
	color: 'ink-muted',
	display: 'flex',
	alignItems: 'center',
	gap: '8px',
	'& svg': { width: '16px', height: '16px' }
});
export const revokeRow = css({ marginTop: '20px', display: 'flex', justifyContent: 'flex-end' });

// ----- shared states -----
export const loadingRow = css({
	display: 'flex',
	alignItems: 'center',
	gap: '8px',
	padding: '24px 0',
	textStyle: 'body',
	color: 'ink-muted',
	'& svg': { width: '20px', height: '20px' }
});
export const errorText = css({ textStyle: 'body-sm', color: 'danger', fontWeight: 600 });
export const spinIcon = css({ animation: 'spin' });
export const enrollTitle = css({ textStyle: 'title', color: 'ink', marginBottom: '8px' });
export const retryBtn = css({ marginTop: '12px' });
export const methodsSave = css({
	paddingTop: '20px',
	borderTopWidth: '1px',
	borderTopStyle: 'solid',
	borderTopColor: 'rule'
});
