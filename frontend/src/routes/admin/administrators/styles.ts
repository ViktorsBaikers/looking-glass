// One-off atomic styles for the Administrators page. Panda's extractor reads
// css() from .ts only, never from .svelte templates.
import { css } from 'styled-system/css';

export const pageWrap = css({
	maxWidth: '896px',
	margin: '0 auto',
	display: 'flex',
	flexDirection: 'column',
	gap: '24px'
});

export const pageHeader = css({ display: 'flex', flexDirection: 'column', gap: '4px' });

export const pageTitle = css({
	textStyle: 'headline-mobile',
	color: 'on-surface',
	md: { textStyle: 'headline-lg' }
});

export const pageSub = css({ textStyle: 'body-lg', color: 'on-surface-variant' });

export const sectionDesc = css({ textStyle: 'body-sm', color: 'on-surface-variant' });

export const addRow = css({
	display: 'flex',
	flexDirection: 'column',
	gap: '16px',
	alignItems: 'stretch',
	sm: { flexDirection: 'row', alignItems: 'flex-end' }
});

export const addField = css({ flex: '1' });

export const peersHead = css({
	display: 'flex',
	justifyContent: 'space-between',
	alignItems: 'baseline',
	gap: '8px',
	marginBottom: '16px'
});

export const accountCount = css({ textStyle: 'label-sm', color: 'on-surface-variant' });

export const peerList = css({
	display: 'flex',
	flexDirection: 'column',
	gap: '8px',
	listStyle: 'none'
});

export const peerRow = css({
	display: 'flex',
	flexDirection: 'column',
	gap: '8px',
	padding: '12px',
	borderRadius: 'lg',
	background: 'surface',
	borderWidth: '1px',
	borderStyle: 'solid',
	borderColor: 'transparent',
	_hover: { borderColor: 'outline-variant' },
	sm: { flexDirection: 'row', justifyContent: 'space-between', alignItems: 'center', gap: '16px' }
});

export const peerMain = css({ display: 'flex', flexDirection: 'column', gap: '4px' });

export const peerNameRow = css({ display: 'flex', alignItems: 'center', gap: '8px', flexWrap: 'wrap' });

export const peerName = css({ textStyle: 'body-md', color: 'on-surface', fontWeight: 600 });

export const peerAdded = css({ textStyle: 'label-sm', color: 'on-surface-variant' });

export const peerActions = css({ display: 'flex', alignItems: 'center', gap: '8px' });

export const passwordGrid = css({
	display: 'grid',
	gridTemplateColumns: '1fr',
	gap: '16px',
	md: { gridTemplateColumns: 'repeat(3, 1fr)' }
});

export const passwordActions = css({ marginTop: '16px' });
