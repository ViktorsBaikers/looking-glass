// One-off atomic styles for the Location editor (issue #7). Panda's extractor
// reads css() from .ts only — never inline these in .svelte (see CATALOGUE.md).
import { css } from 'styled-system/css';

// ----- page head -----
export const backLink = css({
	display: 'inline-flex',
	alignItems: 'center',
	gap: '4px',
	textStyle: 'label-sm',
	color: 'on-surface-variant',
	marginBottom: '16px',
	_hover: { color: 'primary' },
	'& svg': { width: '16px', height: '16px' }
});
export const pageHead = css({ marginBottom: '24px' });
export const pageTitle = css({
	textStyle: 'headline-mobile',
	md: { textStyle: 'headline-lg' },
	color: 'on-surface',
	marginBottom: '8px'
});
export const pageSub = css({ textStyle: 'body-lg', color: 'on-surface-variant' });

// ----- tab panel card (design wraps panel content in a section card) -----
export const panelCard = css({
	background: 'surface-container-low',
	borderWidth: '1px',
	borderStyle: 'solid',
	borderColor: 'outline-variant',
	borderRadius: 'xl',
	padding: '24px',
	marginTop: '24px'
});
export const panelHead = css({
	display: 'flex',
	alignItems: 'center',
	justifyContent: 'space-between',
	gap: '16px',
	marginBottom: '24px'
});
export const panelTitle = css({ textStyle: 'headline-sm', color: 'on-surface', marginBottom: '4px' });
export const panelDesc = css({ textStyle: 'body-sm', color: 'on-surface-variant' });

// ----- forms -----
export const formStack = css({
	display: 'flex',
	flexDirection: 'column',
	gap: '16px',
	maxWidth: '560px'
});
export const formRow2 = css({ display: 'grid', gridTemplateColumns: '1fr', md: { gridTemplateColumns: '1fr 1fr' }, gap: '16px' });
export const saveRow = css({ paddingTop: '8px' });

// ----- data tables (CATALOGUE data-table pattern) -----
export const tableScroller = css({
	overflowX: 'auto',
	borderWidth: '1px',
	borderStyle: 'solid',
	borderColor: 'outline-variant',
	borderRadius: 'lg'
});
export const table = css({ width: '100%', textAlign: 'left', borderCollapse: 'collapse' });
export const th = css({
	padding: '12px',
	textStyle: 'label-sm',
	textTransform: 'uppercase',
	letterSpacing: '0.05em',
	color: 'on-surface-variant',
	background: 'surface-container',
	borderBottomWidth: '1px',
	borderBottomStyle: 'solid',
	borderBottomColor: 'outline-variant'
});
export const thActions = css({ width: '96px', textAlign: 'center' });
export const td = css({
	padding: '12px',
	textStyle: 'body-sm',
	color: 'on-surface',
	borderBottomWidth: '1px',
	borderBottomStyle: 'solid',
	borderBottomColor: 'surface-bright'
});
export const tdMono = css({ fontFamily: 'mono', color: 'primary-fixed-dim' });
export const tdMuted = css({ color: 'on-surface-variant' });
export const trHover = css({ _hover: { background: 'surface-container-high' } });
export const tdActions = css({ textAlign: 'center', whiteSpace: 'nowrap' });
export const rowAction = css({
	padding: '4px',
	borderRadius: 'md',
	color: 'on-surface-variant',
	_hover: { color: 'primary', background: 'surface-container' },
	'& svg': { width: '18px', height: '18px' }
});
export const rowActionDanger = css({
	_hover: { color: 'error', background: 'surface-container' }
});
export const emptyWell = css({
	borderWidth: '1px',
	borderStyle: 'dashed',
	borderColor: 'outline-variant',
	borderRadius: 'lg',
	background: 'surface-container',
	padding: '32px',
	textAlign: 'center',
	textStyle: 'body-sm',
	color: 'on-surface-variant'
});

// ----- methods grid -----
export const methodsIntro = css({ textStyle: 'body-sm', color: 'on-surface-variant', marginBottom: '16px' });
export const familyHead = css({
	textStyle: 'label-md',
	textTransform: 'uppercase',
	letterSpacing: '0.05em',
	color: 'on-surface-variant',
	marginBottom: '8px'
});
export const familyGroup = css({ marginBottom: '24px' });
export const methodsGrid = css({
	display: 'grid',
	gridTemplateColumns: 'repeat(2, minmax(0, 1fr))',
	md: { gridTemplateColumns: 'repeat(4, minmax(0, 1fr))' },
	gap: '8px'
});

// ----- enrollment -----
export const enrollIntro = css({ textStyle: 'body-md', color: 'on-surface-variant', marginBottom: '16px' });
export const cmdBox = css({
	position: 'relative',
	background: 'surface-container-lowest',
	borderWidth: '1px',
	borderStyle: 'solid',
	borderColor: 'outline-variant',
	borderRadius: 'lg',
	padding: '16px',
	paddingRight: '48px',
	fontFamily: 'mono',
	fontSize: '14px',
	lineHeight: '22px',
	color: 'primary-fixed-dim',
	overflowX: 'auto',
	whiteSpace: 'pre'
});
export const cmdCopy = css({ position: 'absolute', top: '8px', right: '8px' });
export const enrollRow = css({
	display: 'flex',
	alignItems: 'center',
	justifyContent: 'space-between',
	gap: '16px',
	borderWidth: '1px',
	borderStyle: 'solid',
	borderColor: 'outline-variant',
	borderRadius: 'md',
	padding: '8px 12px',
	marginTop: '16px',
	textStyle: 'body-sm'
});
export const countdownText = css({ color: 'on-surface-variant' });
export const countdownMono = css({ fontFamily: 'mono', fontVariantNumeric: 'tabular-nums' });
export const expiredText = css({ color: 'error' });
export const connectedRow = css({
	display: 'flex',
	alignItems: 'center',
	gap: '8px',
	marginTop: '16px',
	textStyle: 'body-sm'
});
export const connectedOk = css({ color: 'primary', display: 'flex', alignItems: 'center', gap: '8px', '& svg': { width: '16px', height: '16px' } });
export const waitingMuted = css({
	color: 'on-surface-variant',
	display: 'flex',
	alignItems: 'center',
	gap: '8px',
	'& svg': { width: '16px', height: '16px' }
});
export const revokeRow = css({ marginTop: '16px', display: 'flex', justifyContent: 'flex-end' });

// ----- shared states -----
export const loadingRow = css({
	display: 'flex',
	alignItems: 'center',
	gap: '8px',
	padding: '24px 0',
	textStyle: 'body-md',
	color: 'on-surface-variant',
	'& svg': { width: '20px', height: '20px' }
});
export const errorText = css({ textStyle: 'body-sm', color: 'error' });
export const spinIcon = css({ animation: 'spin' });
export const enrollTitle = css({ textStyle: 'headline-sm', color: 'on-surface', marginBottom: '8px' });
export const retryBtn = css({ marginTop: '8px' });
export const methodsSave = css({
	paddingTop: '16px',
	borderTopWidth: '1px',
	borderTopStyle: 'solid',
	borderTopColor: 'outline-variant'
});
