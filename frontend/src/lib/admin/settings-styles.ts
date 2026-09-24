// One-off atomic styles for the admin Settings page. Panda's extractor cannot
// read css() out of .svelte templates (see CATALOGUE.md), so they live here.
import { css } from 'styled-system/css';

export const pageGrid = css({
	display: 'grid',
	gridTemplateColumns: '1fr',
	gap: '32px',
	alignItems: 'start',
	lg: { gridTemplateColumns: 'minmax(0, 1fr) 320px', gap: '32px' }
});
export const formCol = css({ display: 'flex', flexDirection: 'column', gap: '24px', minWidth: '0' });
export const loadingRow = css({
	display: 'flex',
	alignItems: 'center',
	gap: '8px',
	textStyle: 'body',
	color: 'ink-muted',
	'& svg': { width: '20px', height: '20px' }
});
export const errorText = css({ textStyle: 'body-sm', color: 'danger', fontWeight: 600, marginBottom: '12px' });
export const sectionTitle = css({ textStyle: 'title', color: 'ink' });
export const fieldStack = css({ display: 'flex', flexDirection: 'column', gap: '20px' });
export const limitsGrid = css({
	display: 'grid',
	gridTemplateColumns: '1fr',
	gap: '20px 24px',
	md: { gridTemplateColumns: '1fr 1fr' },
	'& input': { fontFamily: 'mono', fontVariantNumeric: 'tabular-nums' }
});
export const actionsRow = css({
	position: 'sticky',
	bottom: '0',
	zIndex: 2,
	display: 'flex',
	alignItems: 'center',
	gap: '20px',
	padding: '16px 0',
	background: 'paper',
	borderTopWidth: '1px',
	borderTopStyle: 'solid',
	borderTopColor: 'rule'
});
export const indicator = css({ textStyle: 'body-sm', color: 'ink-muted' });

// ----- preview: the real header in miniature, once per theme -----
export const previewCol = css({
	display: 'flex',
	flexDirection: 'column',
	gap: '12px',
	lg: { position: 'sticky', top: '88px' }
});
export const previewHeading = css({ textStyle: 'title', color: 'ink' });
export const previewLabel = css({ textStyle: 'body-sm', color: 'ink-muted', marginTop: '-8px' });
/** Scheme comes from the `light` / `dark` class beside it; tokens follow. */
export const previewCard = css({
	background: 'paper',
	color: 'ink',
	borderWidth: '1px',
	borderStyle: 'solid',
	borderColor: 'rule',
	overflow: 'hidden'
});
export const previewBar = css({
	display: 'flex',
	alignItems: 'center',
	gap: '8px',
	height: '40px',
	padding: '0 12px',
	borderBottomWidth: '1px',
	borderBottomStyle: 'solid',
	borderBottomColor: 'rule'
});
export const previewMark = css({
	width: '14px',
	height: '14px',
	borderRadius: 'full',
	borderWidth: '3px',
	borderStyle: 'solid',
	borderColor: 'ink',
	flexShrink: 0
});
export const previewLogo = css({ height: '20px', width: 'auto', maxWidth: '72px', objectFit: 'contain', flexShrink: 0 });
export const previewTitle = css({
	fontSize: '13px',
	lineHeight: '16px',
	fontWeight: 800,
	overflow: 'hidden',
	textOverflow: 'ellipsis',
	whiteSpace: 'nowrap',
	minWidth: '0'
});
export const previewNav = css({
	marginLeft: 'auto',
	fontSize: '11px',
	fontWeight: 700,
	color: 'ink-muted',
	flexShrink: 0
});
export const previewBody = css({ display: 'flex', flexDirection: 'column', gap: '10px', padding: '14px 12px 12px' });
export const previewLine = css({
	display: 'flex',
	alignItems: 'center',
	'&::after': { content: '""', flex: '1', height: '4px', background: 'var(--line)' }
});
export const previewRoundel = css({
	display: 'inline-flex',
	alignItems: 'center',
	justifyContent: 'center',
	width: '22px',
	height: '22px',
	borderRadius: 'full',
	background: 'var(--line)',
	color: 'var(--line-ink)',
	fontSize: '8px',
	fontWeight: 800
});
export const previewMessage = css({ fontSize: '13px', lineHeight: '18px', color: 'ink-muted', whiteSpace: 'pre-wrap' });
export const previewMeta = css({
	display: 'flex',
	flexWrap: 'wrap',
	columnGap: '6px',
	paddingTop: '10px',
	borderTopWidth: '1px',
	borderTopStyle: 'solid',
	borderTopColor: 'rule',
	fontSize: '11px',
	lineHeight: '16px',
	color: 'ink-muted'
});
export const previewScheme = css({ fontWeight: 700, color: 'ink' });
