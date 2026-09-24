// One-off atomic styles for the admin Settings page. Panda's extractor cannot
// read css() out of .svelte templates (see CATALOGUE.md), so they live here.
import { css } from 'styled-system/css';

export const pageHead = css({ marginBottom: '8px' });
export const pageTitle = css({ textStyle: 'headline-lg', color: 'on-surface' });
export const pageSub = css({ textStyle: 'body-lg', color: 'on-surface-variant' });

export const pageGrid = css({
	display: 'grid',
	gridTemplateColumns: '1fr',
	gap: '32px',
	alignItems: 'start',
	lg: { gridTemplateColumns: 'repeat(12, 1fr)', gap: '24px' }
});
export const formCol = css({
	display: 'flex',
	flexDirection: 'column',
	gap: '24px',
	lg: { gridColumn: 'span 8' }
});
export const loadingRow = css({
	display: 'flex',
	alignItems: 'center',
	gap: '8px',
	textStyle: 'body-sm',
	color: 'on-surface-variant'
});
export const errorText = css({ textStyle: 'body-sm', color: 'error', marginBottom: '12px' });
export const previewLabel = css({ textStyle: 'body-sm', color: 'on-surface-variant', marginBottom: '8px' });
export const sectionTitle = css({ color: 'primary' });
export const previewCol = css({
	display: 'flex',
	flexDirection: 'column',
	gap: '8px',
	lg: { gridColumn: 'span 4' }
});
export const fieldStack = css({ display: 'flex', flexDirection: 'column', gap: '16px' });
export const limitsGrid = css({
	display: 'grid',
	gridTemplateColumns: '1fr',
	gap: '16px',
	md: { gridTemplateColumns: '1fr 1fr' }
});
export const actionsRow = css({ display: 'flex', alignItems: 'center', gap: '24px', paddingTop: '8px' });
export const indicator = css({ textStyle: 'body-sm', color: 'on-surface-variant' });
export const previewHeading = css({ textStyle: 'headline-sm', color: 'primary' });

// Preview cards keep their own scheme in both page themes: the light card stays
// light and the dark card stays dark, so inverse tokens swap with the page theme.
const previewCard = {
	borderWidth: '1px',
	borderStyle: 'solid',
	borderRadius: 'xl',
	padding: '16px',
	display: 'flex',
	flexDirection: 'column',
	gap: '12px'
} as const;
export const previewCardLight = css({
	...previewCard,
	background: { base: 'surface-container-low', _dark: 'inverse-surface' },
	color: { base: 'on-surface', _dark: 'inverse-on-surface' },
	borderColor: { base: 'outline-variant', _dark: 'inverse-on-surface' }
});
export const previewCardDark = css({
	...previewCard,
	background: { base: 'inverse-surface', _dark: 'surface-container-lowest' },
	color: { base: 'inverse-on-surface', _dark: 'on-surface' },
	borderColor: { base: 'inverse-on-surface', _dark: 'outline-variant' }
});
export const previewMutedLight = css({
	color: { base: 'on-surface-variant', _dark: 'inverse-on-surface' }
});
export const previewMutedDark = css({
	color: { base: 'inverse-on-surface', _dark: 'on-surface-variant' }
});

export const previewHead = css({ display: 'flex', alignItems: 'center', gap: '8px' });
export const previewMark = css({
	width: '32px',
	height: '32px',
	borderRadius: 'md',
	display: 'flex',
	alignItems: 'center',
	justifyContent: 'center',
	fontSize: '14px',
	fontWeight: 700,
	flexShrink: 0
});
export const previewMarkLight = css({ background: 'primary-container', color: 'on-primary-container' });
export const previewMarkDark = css({ background: 'primary', color: 'on-primary' });
export const previewLogo = css({
	width: '32px',
	height: '32px',
	objectFit: 'contain',
	borderRadius: 'md',
	flexShrink: 0
});
export const previewTitle = css({ fontSize: '14px', lineHeight: '18px', fontWeight: 700 });
export const previewCaption = css({ fontSize: '12px', lineHeight: '16px' });
export const previewMessage = css({ fontSize: '14px', lineHeight: '20px', whiteSpace: 'pre-wrap' });
export const previewMeta = css({
	display: 'flex',
	flexWrap: 'wrap',
	columnGap: '6px',
	fontSize: '12px',
	lineHeight: '16px'
});
