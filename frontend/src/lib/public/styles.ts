// One-off atomic styles for the public diagnostics page.
// Panda cannot extract css() from .svelte templates, so every one-off class
// string for this feature lives here. Recipes (button/select/tabs/tooltip/…)
// are pre-generated and called directly in components.
//
// Elements that draw a Location's line carry `data-line` + `lineStyle(id)`
// (lib/lines.ts); `var(--line)` resolves to that Location's colour per theme.
import { css } from 'styled-system/css';

// ----- page frame -----
export const page = css({
	maxWidth: '1280px',
	marginInline: 'auto',
	display: 'flex',
	flexDirection: 'column',
	gap: '24px',
	width: '100%',
	minWidth: '0',
	padding: '20px 16px 64px',
	md: { padding: '48px 32px 96px', gap: '48px' }
});
export const pageHead = css({ display: 'flex', flexDirection: 'column', gap: '8px' });
export const pageTitle = css({ textStyle: 'display-sm', color: 'ink', md: { textStyle: 'display' } });
export const pageSubtitle = css({ textStyle: 'body', color: 'ink-muted', maxWidth: '60ch' });
export const locations = css({ display: 'flex', flexDirection: 'column', gap: '0' });

// ----- the selected Location's band: its line, the command row, the facts -----
export const band = css({
	background: 'panel',
	borderWidth: '1px',
	borderStyle: 'solid',
	borderColor: 'rule',
	borderTopWidth: '0',
	boxShadow: 'inset 0 6px 0 var(--line)',
	paddingTop: '6px',
	// Wide screens: the command on the left, the Location's facts beside it.
	lg: { display: 'grid', gridTemplateColumns: 'minmax(0, 5fr) minmax(0, 7fr)' }
});
export const panel = css({
	display: 'grid',
	gridTemplateColumns: 'minmax(0, 2fr) minmax(0, 3fr)',
	gap: '12px 10px',
	padding: '16px',
	alignItems: 'start',
	md: {
		gridTemplateColumns: 'minmax(180px, 240px) minmax(0, 1fr) auto',
		gap: '12px',
		padding: '24px'
	},
	lg: { gridTemplateColumns: 'minmax(150px, 190px) minmax(0, 1fr)', alignContent: 'start', gap: '16px 12px' }
});
// Sits level with the inputs (label 16px + 6px gap above them).
export const runButton = css({
	gridColumn: '1 / -1',
	width: '100%',
	md: { gridColumn: 'auto', width: 'auto', minWidth: '168px', marginTop: '22px' },
	lg: { gridColumn: '1 / -1', width: '100%', marginTop: '4px' }
});
export const panelNote = css({ textStyle: 'body-sm', color: 'ink-muted', gridColumn: '1 / -1' });
export const panelError = css({ textStyle: 'body-sm', color: 'danger' });
export const pageNote = css({
	textStyle: 'body',
	color: 'ink-muted',
	padding: '32px 0',
	borderTopWidth: '1px',
	borderTopStyle: 'solid',
	borderTopColor: 'rule'
});

// ----- station facts (StatusPanel) -----
export const statusList = css({
	display: 'grid',
	gridTemplateColumns: 'repeat(auto-fill, minmax(140px, 1fr))',
	gap: '12px 20px',
	padding: '14px 16px 16px',
	borderTopWidth: '1px',
	borderTopStyle: 'solid',
	borderTopColor: 'rule',
	md: { padding: '20px 24px 24px', gap: '20px 24px' },
	lg: {
		gridTemplateColumns: 'repeat(3, minmax(0, 1fr))',
		alignContent: 'start',
		padding: '24px',
		borderTopWidth: '0',
		borderLeftWidth: '1px',
		borderLeftStyle: 'solid',
		borderLeftColor: 'rule'
	}
});
export const statusRow = css({
	display: 'flex',
	flexDirection: 'column',
	gap: '4px',
	minWidth: '0',
	'& > span:first-child': { textStyle: 'caption', fontWeight: 700, color: 'ink-muted' }
});
export const statusValue = css({ fontSize: '15px', lineHeight: '22px', fontWeight: 600, color: 'ink', minWidth: '0' });
export const statusMono = css({
	fontFamily: 'mono',
	fontSize: '14px',
	lineHeight: '22px',
	color: 'ink',
	minWidth: '0',
	overflowWrap: 'anywhere'
});
export const statusValueRow = css({
	display: 'flex',
	alignItems: 'center',
	gap: '6px',
	minHeight: '32px',
	minWidth: '0',
	marginBlock: '-5px'
});
export const statusDotOnline = css({
	display: 'inline-block',
	width: '16px',
	height: '0',
	borderTopWidth: '4px',
	borderTopStyle: 'solid',
	borderTopColor: 'ok',
	flexShrink: '0'
});
export const statusDotOffline = css({
	display: 'inline-block',
	width: '16px',
	height: '0',
	borderTopWidth: '4px',
	borderTopStyle: 'dotted',
	borderTopColor: 'danger',
	flexShrink: '0'
});
export const iconLink = css({
	display: 'inline-flex',
	alignItems: 'center',
	color: 'ink-muted',
	flexShrink: '0',
	borderRadius: 'sm',
	_hover: { color: 'ink' },
	_focusVisible: { outline: '2px solid {colors.ink}', outlineOffset: '2px' },
	'& svg': { width: '15px', height: '15px' }
});
export const geoValue = css({
	display: 'flex',
	alignItems: 'center',
	gap: '6px',
	fontSize: '15px',
	lineHeight: '22px',
	fontWeight: 600,
	color: 'ink',
	minWidth: '0'
});
export const moreIps = css({ gridColumn: '1 / -1', marginTop: '-8px' });
export const moreIpList = css({
	display: 'grid',
	gridTemplateColumns: 'repeat(auto-fill, minmax(260px, 1fr))',
	gap: '4px 24px',
	paddingTop: '8px'
});
export const moreIpRow = css({
	display: 'flex',
	alignItems: 'center',
	gap: '10px',
	textStyle: 'body-sm',
	'& > span:first-child': { flex: '0 1 auto' }
});
export const moreIpMeta = css({ color: 'ink-muted', flex: '1', minWidth: '0' });
export const moreToggle = css({ marginLeft: '-12px' });

// ----- output (Console) -----
export const resultsSection = css({ display: 'flex', flexDirection: 'column', gap: '16px', scrollMarginTop: '72px' });
export const consoleHeader = css({
	display: 'flex',
	alignItems: 'center',
	justifyContent: 'space-between',
	gap: '12px',
	flexWrap: 'wrap'
});
export const consoleHeaderLeft = css({ display: 'flex', alignItems: 'baseline', gap: '14px' });
export const consoleHeaderRight = css({ display: 'flex', alignItems: 'center', gap: '8px' });
export const consoleHeading = css({ textStyle: 'title', color: 'ink' });
export const chip = css({
	display: 'inline-flex',
	alignItems: 'center',
	gap: '8px',
	fontSize: '13px',
	lineHeight: '16px',
	fontWeight: 700,
	color: 'ink'
});
export const chipText = css({});
export const chipDot = css({
	display: 'inline-block',
	width: '16px',
	height: '0',
	borderTopWidth: '4px',
	borderTopStyle: 'solid',
	flexShrink: '0'
});
export const chipDotTone = {
	connecting: css({ borderTopStyle: 'dashed', borderTopColor: 'ink-faint' }),
	streaming: css({ borderTopColor: 'var(--line)', animation: 'fade-in 700ms ease-in-out infinite alternate' }),
	done: css({ borderTopColor: 'ok' }),
	error: css({ borderTopStyle: 'dotted', borderTopColor: 'danger' }),
	canceled: css({ borderTopStyle: 'dashed', borderTopColor: 'ink-faint' })
} as const;
export const copyOutputIcon = css({ '& svg': { width: '16px', height: '16px' } });

/** Route | Raw switch, shown when the output has a route view. */
export const viewSwitch = css({
	display: 'inline-flex',
	borderWidth: '1px',
	borderStyle: 'solid',
	borderColor: 'rule-strong',
	borderRadius: 'sm',
	padding: '2px',
	gap: '2px'
});
export const viewOption = css({
	minHeight: '26px',
	padding: '0 10px',
	fontSize: '13px',
	fontWeight: 700,
	color: 'ink-muted',
	background: 'transparent',
	border: 'none',
	borderRadius: '1px',
	cursor: 'pointer',
	_hover: { color: 'ink' },
	'&[aria-pressed=true]': { background: 'ink', color: 'on-ink' },
	_focusVisible: { outline: '2px solid {colors.ink}', outlineOffset: '2px' }
});

export const terminal = css({
	background: 'sunk',
	borderWidth: '1px',
	borderStyle: 'solid',
	borderColor: 'rule',
	display: 'flex',
	flexDirection: 'column',
	minWidth: '0'
});
export const titlebar = css({
	display: 'flex',
	alignItems: 'center',
	gap: '10px',
	minHeight: '44px',
	padding: '8px 16px',
	background: 'panel',
	borderBottomWidth: '1px',
	borderBottomStyle: 'solid',
	borderBottomColor: 'rule'
});
/** Roundel for the run's Location in the output title bar. */
export const roundel = css({
	display: 'inline-flex',
	alignItems: 'center',
	justifyContent: 'center',
	width: '24px',
	height: '24px',
	borderRadius: 'full',
	background: 'var(--line)',
	color: 'var(--line-ink)',
	fontSize: '9px',
	fontWeight: 800,
	letterSpacing: '0.02em',
	flexShrink: '0'
});
export const roundelIdle = css({
	width: '14px',
	height: '14px',
	borderRadius: 'full',
	borderWidth: '3px',
	borderStyle: 'solid',
	borderColor: 'rule-strong',
	flexShrink: '0'
});
export const titlebarText = css({
	fontFamily: 'mono',
	fontSize: '13px',
	lineHeight: '20px',
	color: 'ink',
	overflow: 'hidden',
	textOverflow: 'ellipsis',
	whiteSpace: 'nowrap'
});
export const terminalBody = css({
	padding: '16px 20px 20px',
	textStyle: 'code',
	color: 'ink',
	minHeight: '200px',
	maxHeight: '560px',
	overflowY: 'auto',
	overflowX: 'auto',
	whiteSpace: 'pre-wrap',
	overflowWrap: 'break-word',
	md: { minHeight: '240px' }
});
export const linePlain = css({ margin: '0' });
export const lineBytes = css({ color: 'ink-muted' });
export const lineTime = css({ color: 'var(--line)', fontWeight: 700 });
export const lineError = css({ color: 'danger', fontWeight: 600 });
export const lineMeta = css({ color: 'ink-muted' });
export const lineHint = css({
	fontFamily: 'sans',
	fontSize: '15px',
	lineHeight: '22px',
	color: 'ink-muted',
	margin: '0',
	maxWidth: '52ch'
});
export const cursor = css({
	display: 'inline-block',
	width: '8px',
	height: '16px',
	marginTop: '2px',
	background: 'var(--line)',
	animation: 'fade-in 600ms steps(2) infinite alternate'
});
export const metaGap = css({ marginTop: '12px' });

// ----- route view (MtrTable / TraceTable): hops as stations on the line -----
export const tableWrap = css({ overflowX: 'auto', marginInline: '-12px' });
export const mtrTable = css({
	width: '100%',
	minWidth: '520px',
	borderCollapse: 'separate',
	borderSpacing: '0',
	whiteSpace: 'nowrap'
});
export const mtrTh = css({
	padding: '0 12px 10px',
	fontFamily: 'sans',
	fontSize: '12px',
	lineHeight: '16px',
	fontWeight: 700,
	color: 'ink-muted',
	textAlign: 'left',
	borderBottomWidth: '1px',
	borderBottomStyle: 'solid',
	borderBottomColor: 'rule'
});
export const mtrThNum = css({ textAlign: 'right' });
export const routeRow = css({
	animation: 'station-in',
	'& > td': { transitionProperty: 'background', transitionDuration: '120ms' },
	_hover: { '& > td': { background: 'panel' } }
});
export const mtrTd = css({
	padding: '13px 16px',
	fontFamily: 'mono',
	fontSize: '13px',
	lineHeight: '18px',
	color: 'ink-muted',
	fontVariantNumeric: 'tabular-nums'
});
export const mtrTdNum = css({ textAlign: 'right' });
export const mtrTdHop = css({ color: 'ink-faint', textAlign: 'right', paddingRight: '4px' });
export const mtrTdHost = css({ width: '100%', color: 'ink', fontWeight: 600, fontSize: '14px' });
/** Timetable grammar: the host runs into a dotted leader toward its times. */
export const hostLeader = css({
	display: 'flex',
	alignItems: 'baseline',
	gap: '12px',
	_after: {
		content: '""',
		flex: '1',
		minWidth: '24px',
		borderBottomWidth: '2px',
		borderBottomStyle: 'dotted',
		borderBottomColor: 'rule-strong'
	}
});
export const mtrTdStrong = css({ color: 'ink', fontWeight: 700 });
export const mtrTdLoss = css({ color: 'warn', fontWeight: 700 });
export const traceAddr = css({ color: 'ink-muted', fontWeight: 400, marginLeft: '8px' });

/** The line through a hop, and its station marker. */
export const stationCell = css({
	position: 'relative',
	width: '56px',
	minWidth: '56px',
	padding: '0',
	_before: {
		content: '""',
		position: 'absolute',
		left: '50%',
		top: '0',
		bottom: '0',
		width: '8px',
		marginLeft: '-4px',
		background: 'var(--line)'
	},
	'tr:first-child > &': { _before: { top: '50%' } },
	'tr:last-child > &': { _before: { bottom: '50%' } },
	'tr:only-child > &': { _before: { display: 'none' } }
});
/** A hop that answered no probe: the line runs dashed, no station stops here. */
export const stationSilent = css({
	_before: { background: 'transparent', borderLeftWidth: '8px', borderLeftStyle: 'dotted', borderLeftColor: 'var(--line)' }
});
export const station = css({
	position: 'relative',
	zIndex: 1,
	display: 'block',
	width: '18px',
	height: '18px',
	margin: '0 auto',
	borderRadius: 'full',
	background: 'panel',
	borderWidth: '4px',
	borderStyle: 'solid',
	borderColor: 'ink'
});
export const stationEnd = css({ width: '26px', height: '26px', borderWidth: '6px' });
/** The route's origin: the run's Location roundel heads the line. */
export const originRoundel = css({
	position: 'relative',
	zIndex: 1,
	display: 'flex',
	alignItems: 'center',
	justifyContent: 'center',
	width: '32px',
	height: '32px',
	margin: '0 auto',
	borderRadius: 'full',
	background: 'var(--line)',
	color: 'var(--line-ink)',
	fontFamily: 'sans',
	fontSize: '10px',
	fontWeight: 800,
	letterSpacing: '0.02em'
});
export const originName = css({ fontFamily: 'sans', fontSize: '15px', fontWeight: 800, color: 'ink' });
export const originNote = css({ fontFamily: 'sans', fontSize: '13px', color: 'ink-muted', marginLeft: '8px' });
export const stationLossy = css({ borderColor: 'warn', background: 'warn-soft' });
export const stationSilentMark = css({ width: '12px', height: '12px', borderWidth: '3px', borderColor: 'ink-faint', background: 'sunk' });
export const stationLive = css({ animation: 'here-pulse' });

// ----- run metrics -----
export const metricsGrid = css({
	display: 'grid',
	gridTemplateColumns: 'repeat(2, minmax(0, 1fr))',
	background: 'panel',
	borderWidth: '1px',
	borderStyle: 'solid',
	borderColor: 'rule',
	md: { gridTemplateColumns: 'repeat(4, minmax(0, 1fr))' }
});
export const metricCard = css({
	padding: '16px 20px 18px',
	display: 'flex',
	flexDirection: 'column',
	gap: '6px',
	minWidth: '0',
	borderColor: 'rule',
	borderLeftWidth: '1px',
	borderLeftStyle: 'solid',
	'&:nth-child(2n+1)': { borderLeftWidth: '0' },
	'&:nth-child(n+3)': { borderTopWidth: '1px', borderTopStyle: 'solid' },
	md: {
		'&:nth-child(2n+1)': { borderLeftWidth: '1px' },
		'&:nth-child(4n+1)': { borderLeftWidth: '0' },
		'&:nth-child(n+3)': { borderTopWidth: '0' },
		'&:nth-child(n+5)': { borderTopWidth: '1px' }
	}
});
export const metricLabelRow = css({
	display: 'flex',
	alignItems: 'center',
	gap: '6px',
	textStyle: 'label',
	color: 'ink-muted'
});
export const metricInfo = css({
	display: 'inline-flex',
	alignItems: 'center',
	color: 'ink-faint',
	cursor: 'help',
	'& svg': { width: '15px', height: '15px' }
});
export const metricValueRow = css({ display: 'flex', alignItems: 'baseline', gap: '6px' });
export const metricValue = css({ textStyle: 'numeral', color: 'ink' });
export const metricUnit = css({ fontSize: '14px', fontWeight: 600, color: 'ink-muted' });
export const metricCaption = css({ textStyle: 'caption', color: 'ink-muted' });

// ----- speed tests -----
export const speedSection = css({ display: 'flex', flexDirection: 'column', gap: '16px' });
export const speedTitle = css({ textStyle: 'title', color: 'ink' });
export const speedGrid = css({
	display: 'grid',
	gridTemplateColumns: '1fr',
	gap: '16px',
	lg: { gridTemplateColumns: 'repeat(2, minmax(0, 1fr))', gap: '24px' }
});
export const speedCard = css({
	background: 'panel',
	borderWidth: '1px',
	borderStyle: 'solid',
	borderColor: 'rule',
	padding: '20px',
	display: 'flex',
	flexDirection: 'column',
	gap: '16px',
	minWidth: '0',
	md: { padding: '24px' }
});
export const cardTitleRow = css({
	display: 'flex',
	alignItems: 'center',
	justifyContent: 'space-between',
	gap: '12px',
	flexWrap: 'wrap',
	minHeight: '32px'
});
export const iperfTitle = css({ fontSize: '16px', lineHeight: '22px', fontWeight: 800, color: 'ink' });
export const speedCardTitle = css({ fontSize: '16px', lineHeight: '22px', fontWeight: 800, color: 'ink' });
export const endpointBlock = css({
	display: 'flex',
	flexDirection: 'column',
	gap: '12px',
	'& + &': { paddingTop: '16px', borderTopWidth: '1px', borderTopStyle: 'solid', borderTopColor: 'rule' }
});
export const endpointName = css({
	display: 'flex',
	alignItems: 'baseline',
	justifyContent: 'space-between',
	gap: '8px',
	fontSize: '14px',
	fontWeight: 700,
	color: 'ink'
});
export const endpointHost = css({ fontFamily: 'mono', fontSize: '13px', color: 'ink-muted' });
export const cmdGroup = css({ display: 'flex', flexDirection: 'column', gap: '6px' });
export const cmdLabel = css({ textStyle: 'caption', fontWeight: 700, color: 'ink-muted' });
export const cmdRow = css({
	display: 'flex',
	alignItems: 'center',
	justifyContent: 'space-between',
	gap: '8px',
	background: 'sunk',
	padding: '4px 4px 4px 12px'
});
export const cmdCode = css({
	fontFamily: 'mono',
	fontSize: '14px',
	lineHeight: '22px',
	color: 'ink',
	flex: '1',
	minWidth: '0',
	overflowX: 'auto',
	whiteSpace: 'nowrap'
});
export const readouts = css({
	display: 'grid',
	gridTemplateColumns: '1fr 1fr',
	gap: '16px'
});
export const readoutLabel = css({
	display: 'flex',
	alignItems: 'center',
	gap: '6px',
	textStyle: 'label',
	color: 'ink-muted',
	marginBottom: '4px',
	'& svg': { width: '16px', height: '16px' }
});
export const readoutValueRow = css({ display: 'flex', alignItems: 'baseline', gap: '6px' });
export const readoutValue = css({
	fontSize: '40px',
	lineHeight: '44px',
	fontWeight: 800,
	letterSpacing: '-0.03em',
	fontVariantNumeric: 'tabular-nums',
	color: 'ink'
});
export const readoutUnit = css({ fontSize: '14px', fontWeight: 600, color: 'ink-muted' });
export const barTrack = css({
	width: '100%',
	background: 'sunk',
	height: '6px',
	display: 'flex',
	overflow: 'hidden'
});
export const barDownload = css({
	background: 'var(--line)',
	height: '100%',
	transitionProperty: 'width',
	transitionDuration: '240ms',
	transitionTimingFunction: 'out'
});
export const barUpload = css({
	background: 'ink',
	height: '100%',
	transitionProperty: 'width',
	transitionDuration: '240ms',
	transitionTimingFunction: 'out'
});
export const noFilesNote = css({ textStyle: 'body-sm', color: 'ink-muted' });
export const fileLinks = css({
	display: 'flex',
	flexDirection: 'column',
	borderTopWidth: '1px',
	borderTopStyle: 'solid',
	borderTopColor: 'rule'
});
export const fileLink = css({
	display: 'flex',
	alignItems: 'center',
	gap: '10px',
	minHeight: '44px',
	padding: '0 4px',
	fontSize: '14px',
	fontWeight: 600,
	color: 'ink',
	textDecoration: 'none',
	borderBottomWidth: '1px',
	borderBottomStyle: 'solid',
	borderBottomColor: 'rule',
	_hover: { background: 'sunk' },
	_focusVisible: { outline: '2px solid {colors.ink}', outlineOffset: '-2px' },
	'& svg': { width: '18px', height: '18px', color: 'ink-muted' }
});
export const fileSize = css({ fontFamily: 'mono', fontSize: '13px', color: 'ink-muted', marginLeft: 'auto' });
export const downloadIcon = css({});
