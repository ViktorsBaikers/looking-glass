// One-off atomic styles for the admin Locations list page
// (routes/admin/+page.svelte). Panda cannot extract css() from .svelte, so the
// class constants live here; recipes (button/card/…) stay inline in the page.
import { css } from 'styled-system/css';

// ----- page header + toolbar -----
export const pageHead = css({
	display: 'flex',
	flexDirection: 'column',
	gap: '16px',
	md: { flexDirection: 'row', justifyContent: 'space-between', alignItems: 'center' }
});
export const pageTitle = css({
	textStyle: { base: 'headline-mobile', md: 'headline-lg' },
	color: 'on-surface',
	marginBottom: '8px'
});
export const pageSub = css({ textStyle: 'body-lg', color: 'on-surface-variant' });
export const toolbar = css({
	display: 'flex',
	flexDirection: 'column',
	gap: '16px',
	md: { flexDirection: 'row', alignItems: 'flex-end' },
	background: 'surface-container',
	borderWidth: '1px',
	borderStyle: 'solid',
	borderColor: 'outline-variant',
	borderRadius: 'xl',
	padding: '24px'
});
export const searchField = css({ flex: '1' });
export const fieldLabel = css({ display: 'block', marginBottom: '8px' });
export const searchWrap = css({
	position: 'relative',
	display: 'flex',
	alignItems: 'center',
	'& svg': {
		position: 'absolute',
		left: '12px',
		width: '20px',
		height: '20px',
		color: 'on-surface-variant',
		pointerEvents: 'none'
	}
});
export const searchInput = css({ paddingInlineStart: '40px' });
export const sortField = css({ width: '100%', md: { width: '256px' } });

// ----- cards -----
export const stack = css({ display: 'flex', flexDirection: 'column', gap: '48px' });
export const grid = css({ display: 'grid', gridTemplateColumns: '1fr', md: { gridTemplateColumns: '1fr 1fr' }, gap: '24px' });
export const card = css({
	display: 'flex',
	flexDirection: 'column',
	background: 'surface-container-low',
	borderWidth: '1px',
	borderStyle: 'solid',
	borderColor: 'outline-variant',
	borderRadius: 'xl',
	padding: '24px',
	md: { padding: '48px' },
	minHeight: '300px',
	_hover: { borderColor: 'primary/50' },
	transitionProperty: 'border-color',
	transitionDuration: '300ms'
});
export const cardHead = css({
	display: 'flex',
	justifyContent: 'space-between',
	alignItems: 'flex-start',
	gap: '16px',
	marginBottom: '24px'
});
export const cardTitle = css({ textStyle: 'headline-md', color: 'on-surface', marginBottom: '8px' });
export const metaRow = css({
	display: 'flex',
	alignItems: 'center',
	gap: '8px',
	textStyle: 'body-md',
	color: 'on-surface-variant'
});
export const geoChip = css({
	background: 'surface-container-highest',
	padding: '4px 8px',
	borderRadius: 'sm',
	color: 'on-surface',
	fontWeight: 600
});
export const metrics = css({
	display: 'grid',
	gridTemplateColumns: '1fr 1fr',
	gap: '24px',
	marginBottom: '32px',
	flexGrow: 1
});
export const metricLabel = css({
	textStyle: 'label-md',
	color: 'on-surface-variant',
	marginBottom: '8px'
});
export const metricValue = css({ textStyle: 'body-lg', color: 'on-surface' });
export const cardFoot = css({
	display: 'flex',
	alignItems: 'center',
	gap: '16px',
	marginTop: 'auto',
	paddingTop: '24px',
	borderTopWidth: '1px',
	borderTopStyle: 'solid',
	borderTopColor: 'outline-variant'
});
/** Outline-only look for the Edit/Revoke card actions (design shows a bordered
 *  transparent button; the ghost recipe carries no border). */
export const outlineAction = css({
	borderWidth: '1px',
	borderStyle: 'solid',
	borderColor: 'outline-variant'
});
export const trailingAction = css({ marginLeft: 'auto' });

// ----- empty / loading / error states -----
export const stateCard = css({
	display: 'flex',
	flexDirection: 'column',
	alignItems: 'center',
	gap: '16px',
	textAlign: 'center',
	borderWidth: '1px',
	borderStyle: 'dashed',
	borderColor: 'outline-variant',
	borderRadius: 'xl',
	padding: '48px 16px'
});
export const stateText = css({ textStyle: 'body-md', color: 'on-surface-variant' });
export const errorCard = css({
	display: 'flex',
	flexDirection: 'column',
	alignItems: 'center',
	gap: '16px',
	textAlign: 'center',
	borderWidth: '1px',
	borderStyle: 'solid',
	borderColor: 'error',
	borderRadius: 'xl',
	padding: '48px 16px',
	color: 'error'
});
export const skeleton = css({
	height: '300px',
	borderRadius: 'xl',
	borderWidth: '1px',
	borderStyle: 'solid',
	borderColor: 'outline-variant',
	background: 'surface-container-low',
	opacity: 0.6
});
export const formStack = css({ display: 'flex', flexDirection: 'column', gap: '16px' });
export const formError = css({ textStyle: 'body-sm', color: 'error' });
