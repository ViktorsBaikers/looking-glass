// One-off atomic styles for the public diagnostics page (issues #4/#5).
// Panda cannot extract css() from .svelte templates, so every one-off class
// string for this feature lives here. Recipes (button/select/tabs/tooltip/…)
// are pre-generated and called directly in components.
import { css } from 'styled-system/css';

// ----- page frame -----
export const page = css({
	maxWidth: '1200px',
	marginInline: 'auto',
	display: 'flex',
	flexDirection: 'column',
	gap: '32px',
	width: '100%',
	minWidth: '0',
	padding: '16px',
	md: { padding: '32px' }
});
/** The selected Location's tab panel keeps the page's 32px rhythm below the tabs. */
export const locationPanel = css({ paddingTop: '32px' });
export const pageTitle = css({
	textStyle: 'headline-mobile',
	color: 'on-surface',
	marginBottom: '8px',
	md: { textStyle: 'headline-lg' }
});
export const pageSubtitle = css({ textStyle: 'body-lg', color: 'on-surface-variant' });

// ----- control panel -----
export const panel = css({
	background: 'surface-container',
	borderWidth: '1px',
	borderStyle: 'solid',
	borderColor: 'outline-variant',
	borderRadius: 'xl',
	overflow: 'hidden',
	display: 'flex',
	flexDirection: 'column',
	md: { flexDirection: 'row' }
});
export const panelLeft = css({
	flex: '1',
	minWidth: '0',
	padding: '24px',
	display: 'flex',
	flexDirection: 'column',
	gap: '16px'
});
export const inputsGrid = css({
	display: 'grid',
	gap: '16px',
	md: { gridTemplateColumns: '1fr 1fr' }
});
export const panelRight = css({
	background: 'surface-container-high',
	borderTopWidth: '1px',
	borderTopStyle: 'solid',
	borderColor: 'outline-variant',
	padding: '24px',
	display: 'flex',
	flexDirection: 'column',
	gap: '16px',
	width: '100%',
	md: {
		width: '340px',
		flexShrink: '0',
		borderTopWidth: '0',
		borderLeftWidth: '1px',
		borderLeftStyle: 'solid'
	}
});
export const runButton = css({ width: '100%', textStyle: 'headline-sm', marginTop: 'auto' });
export const panelNote = css({ textStyle: 'body-sm', color: 'on-surface-variant' });
export const panelError = css({ textStyle: 'body-sm', color: 'error' });

// ----- status panel (right column rows) -----
export const statusList = css({ display: 'flex', flexDirection: 'column', gap: '10px' });
export const statusRow = css({
	display: 'flex',
	alignItems: 'center',
	justifyContent: 'space-between',
	gap: '8px',
	textStyle: 'body-md',
	color: 'on-surface-variant',
	minWidth: '0'
});
export const statusValue = css({ color: 'on-surface', minWidth: '0' });
export const statusMono = css({
	textStyle: 'mono-data',
	color: 'on-surface',
	minWidth: '0',
	overflowWrap: 'anywhere'
});
export const statusValueRow = css({
	display: 'flex',
	alignItems: 'center',
	gap: '4px',
	minWidth: '0'
});
export const statusDotOnline = css({
	width: '8px',
	height: '8px',
	borderRadius: 'full',
	background: 'primary-container',
	boxShadow: 'glow',
	flexShrink: '0'
});
export const statusDotOffline = css({
	width: '8px',
	height: '8px',
	borderRadius: 'full',
	background: 'error',
	flexShrink: '0'
});
export const iconLink = css({
	display: 'inline-flex',
	alignItems: 'center',
	color: 'on-surface-variant',
	flexShrink: '0',
	_hover: { color: 'primary' },
	'& svg': { width: '16px', height: '16px' }
});
export const geoValue = css({
	display: 'flex',
	alignItems: 'center',
	gap: '4px',
	color: 'on-surface',
	minWidth: '0'
});
export const moreIpList = css({
	display: 'flex',
	flexDirection: 'column',
	gap: '8px',
	paddingTop: '8px'
});
export const moreIpRow = css({
	display: 'flex',
	alignItems: 'center',
	justifyContent: 'space-between',
	gap: '8px',
	textStyle: 'body-sm'
});
export const moreIpMeta = css({ color: 'on-surface-variant' });

// ----- live console -----
export const resultsSection = css({ display: 'flex', flexDirection: 'column', gap: '16px' });
export const consoleHeader = css({
	display: 'flex',
	alignItems: 'center',
	justifyContent: 'space-between',
	gap: '12px',
	flexWrap: 'wrap'
});
export const consoleHeaderLeft = css({ display: 'flex', alignItems: 'center', gap: '12px' });
export const consoleHeading = css({ textStyle: 'headline-sm', color: 'on-surface' });
export const chip = css({
	display: 'inline-flex',
	alignItems: 'center',
	gap: '6px',
	background: 'surface-container-high',
	borderWidth: '1px',
	borderStyle: 'solid',
	borderColor: 'outline-variant',
	borderRadius: 'full',
	padding: '4px 10px'
});
export const chipText = css({ textStyle: 'label-sm', color: 'on-surface-variant' });
export const chipDot = css({ width: '6px', height: '6px', borderRadius: 'full', flexShrink: '0' });
export const chipDotTone = {
	connecting: css({ background: 'warning' }),
	streaming: css({ background: 'primary' }),
	done: css({ background: 'primary-container' }),
	error: css({ background: 'error' }),
	canceled: css({ background: 'outline' })
} as const;
export const copyOutputIcon = css({ '& svg': { width: '16px', height: '16px' } });

export const terminal = css({
	background: 'surface-container-lowest',
	borderWidth: '1px',
	borderStyle: 'solid',
	borderColor: 'outline-variant',
	borderRadius: 'xl',
	overflow: 'hidden',
	display: 'flex',
	flexDirection: 'column'
});
export const titlebar = css({
	background: 'surface-container',
	borderBottomWidth: '1px',
	borderBottomStyle: 'solid',
	borderColor: 'outline-variant',
	padding: '6px 12px',
	display: 'flex',
	alignItems: 'center',
	gap: '6px'
});
export const trafficDot = css({ width: '10px', height: '10px', borderRadius: 'full', flexShrink: '0' });
export const trafficRed = css({ background: 'error' });
export const trafficYellow = css({ background: 'warning' });
export const trafficGreen = css({ background: 'primary-container' });
export const titlebarText = css({
	textStyle: 'label-sm',
	color: 'on-surface-variant',
	marginLeft: '8px',
	overflow: 'hidden',
	textOverflow: 'ellipsis',
	whiteSpace: 'nowrap'
});
export const terminalBody = css({
	padding: '16px',
	textStyle: 'mono-data',
	color: 'on-surface',
	height: '300px',
	overflowY: 'auto',
	overflowX: 'auto',
	whiteSpace: 'pre-wrap',
	overflowWrap: 'break-word'
});
export const linePlain = css({ margin: '0' });
// Syntax colouring: dark values match the design 1:1; light values are the
// readable token equivalents on the light console background.
export const lineBytes = css({ color: 'primary', _dark: { color: 'primary-fixed' } });
export const lineTime = css({ color: 'tertiary', _dark: { color: 'primary-container' } });
export const lineError = css({ color: 'error' });
export const lineMeta = css({ color: 'on-surface-variant' });
export const lineHint = css({ color: 'on-surface-variant', margin: '0' });
export const cursor = css({ color: 'primary', margin: '0' });

// ----- mtr hop table -----
export const tableWrap = css({
	borderRadius: 'lg',
	borderWidth: '1px',
	borderStyle: 'solid',
	borderColor: 'outline-variant',
	overflowX: 'auto',
	marginTop: '8px'
});
export const mtrTable = css({ width: '100%', borderCollapse: 'collapse', minWidth: '560px' });
export const mtrTh = css({
	padding: '8px 12px',
	textStyle: 'label-sm',
	color: 'on-surface-variant',
	background: 'surface-container',
	textAlign: 'left',
	whiteSpace: 'nowrap'
});
export const mtrThNum = css({ textAlign: 'right' });
export const mtrTd = css({
	padding: '6px 12px',
	textStyle: 'body-sm',
	color: 'on-surface',
	borderBottomWidth: '1px',
	borderBottomStyle: 'solid',
	borderColor: 'surface-bright',
	textAlign: 'right',
	fontVariantNumeric: 'tabular-nums',
	whiteSpace: 'nowrap'
});
export const mtrTdHop = css({ color: 'on-surface-variant', textAlign: 'left' });
export const mtrTdHost = css({
	textAlign: 'left',
	fontFamily: 'mono',
	color: 'primary',
	_dark: { color: 'primary-fixed-dim' }
});

// ----- metric cards -----
export const metricsGrid = css({
	display: 'grid',
	gridTemplateColumns: '1fr 1fr',
	gap: '16px',
	md: { gridTemplateColumns: 'repeat(4, 1fr)', gap: '24px' }
});
export const metricCard = css({
	background: 'surface-container',
	borderWidth: '1px',
	borderStyle: 'solid',
	borderColor: 'outline-variant',
	borderRadius: 'xl',
	padding: '16px',
	display: 'flex',
	flexDirection: 'column',
	gap: '4px',
	minWidth: '0'
});
export const metricLabelRow = css({
	display: 'flex',
	alignItems: 'center',
	gap: '6px',
	textStyle: 'label-md',
	color: 'on-surface-variant',
	marginBottom: '4px'
});
export const metricInfo = css({
	display: 'inline-flex',
	alignItems: 'center',
	color: 'on-surface-variant',
	cursor: 'help',
	'& svg': { width: '14px', height: '14px' }
});
export const metricValueRow = css({ display: 'flex', alignItems: 'baseline', gap: '6px' });
export const metricValue = css({ textStyle: 'headline-mobile', color: 'on-surface' });
export const metricUnit = css({ textStyle: 'body-md', color: 'on-surface-variant' });
export const metricCaption = css({ textStyle: 'label-sm', color: 'on-surface-variant' });

// ----- speed tests section -----
export const speedSection = css({ display: 'flex', flexDirection: 'column', gap: '16px' });
export const speedTitle = css({ textStyle: 'headline-sm', color: 'on-surface' });
export const speedGrid = css({
	display: 'grid',
	gridTemplateColumns: '1fr',
	gap: '16px',
	md: { gridTemplateColumns: '1fr 1fr' }
});
export const speedCard = css({
	background: 'surface-container',
	borderWidth: '1px',
	borderStyle: 'solid',
	borderColor: 'outline-variant',
	borderRadius: 'xl',
	padding: '16px',
	display: 'flex',
	flexDirection: 'column',
	gap: '12px',
	minWidth: '0'
});
export const cardTitleRow = css({
	display: 'flex',
	alignItems: 'center',
	justifyContent: 'space-between',
	gap: '8px',
	flexWrap: 'wrap'
});
export const iperfTitle = css({ textStyle: 'headline-sm', color: 'on-surface' });
export const speedCardTitle = css({ textStyle: 'body-lg', fontWeight: '600', color: 'on-surface' });
export const endpointBlock = css({ display: 'flex', flexDirection: 'column', gap: '12px' });
export const endpointName = css({
	display: 'flex',
	alignItems: 'baseline',
	justifyContent: 'space-between',
	gap: '8px',
	textStyle: 'label-md',
	color: 'on-surface'
});
export const endpointHost = css({ textStyle: 'mono-data', color: 'on-surface-variant' });
export const cmdGroup = css({ display: 'flex', flexDirection: 'column', gap: '4px' });
export const cmdLabel = css({ textStyle: 'label-sm', color: 'on-surface-variant' });
export const cmdRow = css({
	display: 'flex',
	alignItems: 'center',
	justifyContent: 'space-between',
	gap: '8px',
	background: 'surface-container-high',
	borderWidth: '1px',
	borderStyle: 'solid',
	borderColor: 'outline-variant',
	borderRadius: 'md',
	padding: '8px 12px'
});
export const cmdCode = css({
	textStyle: 'mono-data',
	color: 'on-surface',
	flex: '1',
	minWidth: '0',
	overflowX: 'auto',
	whiteSpace: 'nowrap'
});
export const readouts = css({
	display: 'flex',
	justifyContent: 'space-between',
	gap: '16px',
	marginTop: '8px'
});
export const readoutLabel = css({
	display: 'flex',
	alignItems: 'center',
	gap: '4px',
	textStyle: 'label-sm',
	color: 'on-surface-variant',
	marginBottom: '4px',
	'& svg': { width: '14px', height: '14px' }
});
export const readoutValueRow = css({ display: 'flex', alignItems: 'baseline', gap: '6px' });
export const readoutValue = css({ textStyle: 'headline-mobile', color: 'on-surface' });
export const readoutUnit = css({ textStyle: 'body-md', color: 'on-surface-variant' });
export const barTrack = css({
	width: '100%',
	background: 'surface-variant',
	borderRadius: 'full',
	height: '6px',
	display: 'flex',
	overflow: 'hidden',
	marginTop: '8px'
});
export const barDownload = css({ background: 'primary', height: '100%' });
export const barUpload = css({ background: 'tertiary', height: '100%' });
export const noFilesNote = css({ textStyle: 'body-sm', color: 'on-surface-variant' });
export const fileLinks = css({ display: 'flex', flexDirection: 'column', gap: '8px', alignItems: 'stretch' });
export const fileLink = css({ justifyContent: 'flex-start', width: '100%' });
export const fileSize = css({ textStyle: 'label-sm', color: 'on-surface-variant', marginLeft: 'auto' });
export const downloadIcon = css({ '& svg': { width: '16px', height: '16px' } });
