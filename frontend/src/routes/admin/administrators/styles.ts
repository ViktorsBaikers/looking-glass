// One-off atomic styles for the Administrators page. Panda's extractor reads
// css() from .ts only, never from .svelte templates. The page frame reuses the
// shared admin scaffolding in $lib/styles.
import { css } from 'styled-system/css';

export {
	adminPage as pageWrap,
	adminPageHead as pageHeader,
	adminTitle as pageTitle,
	adminLede as pageSub
} from '$lib/styles.js';

export const sectionDesc = css({ textStyle: 'body-sm', color: 'ink-muted' });

export const addRow = css({
	display: 'flex',
	flexDirection: 'column',
	gap: '12px',
	alignItems: 'stretch',
	sm: { flexDirection: 'row', alignItems: 'flex-end' }
});

export const addField = css({ flex: '1', maxWidth: '420px' });

export const peersHead = css({
	display: 'flex',
	justifyContent: 'space-between',
	alignItems: 'baseline',
	gap: '8px',
	marginBottom: '12px'
});

export const accountCount = css({ textStyle: 'caption', fontWeight: 700, color: 'ink-muted' });

/** The roster: one ruled row per peer. */
export const peerList = css({
	display: 'flex',
	flexDirection: 'column',
	listStyle: 'none',
	borderTopWidth: '1px',
	borderTopStyle: 'solid',
	borderTopColor: 'ink'
});

export const peerRow = css({
	display: 'flex',
	flexDirection: 'column',
	gap: '10px',
	padding: '14px 0',
	borderBottomWidth: '1px',
	borderBottomStyle: 'solid',
	borderBottomColor: 'rule',
	sm: { flexDirection: 'row', justifyContent: 'space-between', alignItems: 'center', gap: '16px' }
});

export const peerMain = css({ display: 'flex', flexDirection: 'column', gap: '2px', minWidth: '0' });

export const peerNameRow = css({ display: 'flex', alignItems: 'center', gap: '10px', flexWrap: 'wrap' });

export const peerName = css({ fontSize: '16px', lineHeight: '22px', fontWeight: 800, color: 'ink' });

export const peerAdded = css({ textStyle: 'caption', color: 'ink-muted', fontVariantNumeric: 'tabular-nums' });

export const peerActions = css({ display: 'flex', alignItems: 'center', gap: '6px' });

export const passwordGrid = css({
	display: 'grid',
	gridTemplateColumns: '1fr',
	gap: '20px',
	md: { gridTemplateColumns: 'repeat(3, 1fr)', gap: '24px' }
});

export const passwordActions = css({
	marginTop: '24px',
	paddingTop: '20px',
	borderTopWidth: '1px',
	borderTopStyle: 'solid',
	borderTopColor: 'rule'
});
