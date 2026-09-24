// One-off atomic styles for the shells and primitives.
//
// Panda's build-time extractor reads `css()` reliably from .ts but NOT from .svelte
// (the svelte→tsx transform leaves Svelte template syntax that defeats the parser),
// so every one-off class string lives here and is imported by the components.
// Recipes (button/card/dialog/…) are pre-generated via `staticCss` in panda.config.
import { css } from 'styled-system/css';

// ----- shared -----
export const spin = css({ animation: 'spin' });
export const srOnly = css({
	position: 'absolute',
	width: '1px',
	height: '1px',
	overflow: 'hidden',
	clip: 'rect(0,0,0,0)',
	whiteSpace: 'nowrap'
});

// ----- public shell (routes/+layout.svelte) -----
// dvh, not vh: on mobile, 100vh includes the collapsing browser bar.
export const shell = css({ display: 'flex', flexDirection: 'column', minHeight: '100dvh' });
/** Hidden until focused: the first Tab stop jumps past the header to the page. */
export const skipLink = css({
	position: 'absolute',
	left: '12px',
	top: '8px',
	zIndex: 70,
	padding: '8px 12px',
	background: 'ink',
	color: 'on-ink',
	textStyle: 'label',
	borderRadius: 'sm',
	transform: 'translateY(-200%)',
	_focusVisible: { transform: 'none', outline: '2px solid {colors.ink}', outlineOffset: '2px' }
});
export const header = css({
	position: 'sticky',
	top: '0',
	zIndex: 40,
	width: '100%',
	background: 'paper',
	borderBottomWidth: '1px',
	borderBottomStyle: 'solid',
	borderBottomColor: 'rule'
});
export const headerInner = css({
	display: 'flex',
	alignItems: 'center',
	justifyContent: 'space-between',
	gap: '8px',
	width: '100%',
	maxWidth: '1280px',
	margin: '0 auto',
	height: '56px',
	padding: '0 12px',
	md: { padding: '0 32px', gap: '16px' }
});
export const brand = css({
	display: 'flex',
	alignItems: 'center',
	gap: '10px',
	minWidth: '0',
	textDecoration: 'none',
	color: 'ink',
	textStyle: 'item',
	// Narrow headers share the row with nav and the theme toggle.
	fontSize: '14px',
	md: { fontSize: '16px' },
	'& > span:last-child': { overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' },
	_focusVisible: { outline: '2px solid {colors.ink}', outlineOffset: '4px' }
});
/** Fallback mark when no operator logo is set: an interchange ring. */
export const brandMark = css({
	width: '18px',
	height: '18px',
	borderRadius: 'full',
	borderWidth: '4px',
	borderStyle: 'solid',
	borderColor: 'ink',
	background: 'paper',
	flexShrink: 0
});
export const logo = css({ height: '28px', width: 'auto', maxWidth: '120px', objectFit: 'contain' });
export const nav = css({ display: 'flex', alignItems: 'stretch', alignSelf: 'stretch', gap: '4px' });
export const navLink = css({
	display: 'inline-flex',
	alignItems: 'center',
	padding: '0 6px',
	fontSize: '13px',
	md: { padding: '0 10px', fontSize: '14px' },
	fontWeight: 600,
	color: 'ink-muted',
	textDecoration: 'none',
	borderBottomWidth: '3px',
	borderBottomStyle: 'solid',
	borderBottomColor: 'transparent',
	borderTopWidth: '3px',
	borderTopStyle: 'solid',
	borderTopColor: 'transparent',
	transitionProperty: 'color, border-color',
	transitionDuration: '120ms',
	_hover: { color: 'ink' },
	_focusVisible: { outline: '2px solid {colors.ink}', outlineOffset: '-2px' }
});
export const navLinkActive = css({ color: 'ink', fontWeight: 800, borderBottomColor: 'ink' });
export const headerRight = css({ display: 'flex', alignItems: 'center', alignSelf: 'stretch', gap: '4px', md: { gap: '8px' } });
export const mainArea = css({ flex: '1', width: '100%', minWidth: '0' });
export const footer = css({
	borderTopWidth: '1px',
	borderTopStyle: 'solid',
	borderTopColor: 'rule'
});
export const footerInner = css({
	display: 'flex',
	flexDirection: 'column',
	gap: '12px',
	width: '100%',
	maxWidth: '1280px',
	margin: '0 auto',
	padding: '24px 16px 32px',
	md: { flexDirection: 'row', alignItems: 'baseline', justifyContent: 'space-between', padding: '24px 32px 40px' }
});
export const termsLink = css({
	textStyle: 'body-sm',
	fontWeight: 600,
	color: 'ink',
	textDecoration: 'underline',
	flexShrink: 0,
	_hover: { color: 'ink-muted' },
	_focusVisible: { outline: '2px solid {colors.ink}', outlineOffset: '2px' }
});
export const customText = css({ textStyle: 'body-sm', color: 'ink-muted', maxWidth: '72ch', whiteSpace: 'pre-line' });

// ----- admin shell (routes/admin/+layout.svelte) -----
export const checking = css({
	display: 'flex',
	alignItems: 'center',
	justifyContent: 'center',
	minHeight: '50vh',
	color: 'ink-muted',
	'& svg': { width: '24px', height: '24px' }
});
export const adminShell = css({
	display: 'flex',
	alignItems: 'flex-start',
	width: '100%',
	maxWidth: '1280px',
	marginInline: 'auto',
	minHeight: '100%'
});
export const sidebar = css({
	display: 'none',
	md: {
		display: 'flex',
		position: 'sticky',
		top: '56px',
		alignSelf: 'flex-start',
		flexDirection: 'column',
		width: '220px',
		flexShrink: 0,
		height: 'calc(100dvh - 56px)',
		overflowY: 'auto',
		padding: '40px 24px 24px 32px'
	}
});
export const heading = css({
	textStyle: 'key',
	color: 'ink-muted',
	marginBottom: '16px'
});
/** Admin sections drawn as stations on one vertical line. */
export const navList = css({
	position: 'relative',
	display: 'flex',
	flexDirection: 'column',
	gap: '4px',
	_before: {
		content: '""',
		position: 'absolute',
		left: '6px',
		top: '18px',
		bottom: '18px',
		width: '3px',
		background: 'rule'
	}
});
export const navItem = css({
	position: 'relative',
	display: 'flex',
	alignItems: 'center',
	gap: '14px',
	minHeight: '36px',
	fontSize: '14px',
	fontWeight: 600,
	textDecoration: 'none',
	color: 'ink-muted',
	background: 'transparent',
	border: 'none',
	width: '100%',
	cursor: 'pointer',
	textAlign: 'left',
	transitionProperty: 'color',
	transitionDuration: '120ms',
	_before: {
		content: '""',
		width: '15px',
		height: '15px',
		borderRadius: 'full',
		borderWidth: '3px',
		borderStyle: 'solid',
		borderColor: 'rule-strong',
		background: 'paper',
		flexShrink: 0,
		zIndex: 1,
		transitionProperty: 'background, border-color',
		transitionDuration: '160ms'
	},
	_hover: { color: 'ink', _before: { borderColor: 'ink' } },
	_focusVisible: { outline: '2px solid {colors.ink}', outlineOffset: '2px' }
});
export const navItemActive = css({
	color: 'ink',
	fontWeight: 800,
	_before: { background: 'ink', borderColor: 'ink' }
});
export const logoutWrap = css({ marginTop: 'auto', paddingTop: '24px' });
export const logoutBtn = css({
	display: 'inline-flex',
	alignItems: 'center',
	gap: '8px',
	padding: '6px 0',
	fontSize: '14px',
	fontWeight: 600,
	color: 'ink-muted',
	background: 'transparent',
	border: 'none',
	cursor: 'pointer',
	_hover: { color: 'ink' },
	_focusVisible: { outline: '2px solid {colors.ink}', outlineOffset: '2px' },
	'& svg': { width: '18px', height: '18px' }
});
export const contentCol = css({ flex: '1', minWidth: '0', display: 'flex', flexDirection: 'column' });
export const mobileBar = css({
	display: 'flex',
	alignItems: 'center',
	gap: '8px',
	padding: '8px 12px',
	borderBottomWidth: '1px',
	borderBottomStyle: 'solid',
	borderBottomColor: 'rule',
	fontSize: '14px',
	fontWeight: 700,
	md: { display: 'none' }
});
export const menuBtn = css({
	display: 'inline-flex',
	alignItems: 'center',
	justifyContent: 'center',
	width: '40px',
	height: '40px',
	borderRadius: 'sm',
	color: 'ink',
	background: 'transparent',
	border: 'none',
	cursor: 'pointer',
	_hover: { background: 'sunk' },
	_focusVisible: { outline: '2px solid {colors.ink}', outlineOffset: '2px' },
	'& svg': { width: '22px', height: '22px' }
});
export const content = css({
	flex: '1',
	minWidth: '0',
	padding: '24px 16px 64px',
	md: { padding: '40px 32px 80px' }
});
export const drawerBackdrop = css({
	position: 'fixed',
	inset: '0',
	background: 'rgba(10, 11, 13, 0.55)',
	zIndex: 50,
	_open: { animation: 'fade-in' },
	_closed: { animation: 'fade-out' }
});
export const drawerPositioner = css({
	position: 'fixed',
	inset: '0',
	display: 'flex',
	justifyContent: 'flex-start',
	zIndex: 50
});
export const drawerContent = css({
	// Anchors the close button; without it the X lands on the viewport's corner.
	position: 'relative',
	display: 'flex',
	flexDirection: 'column',
	width: '280px',
	maxWidth: '85vw',
	height: '100%',
	padding: '64px 24px 24px',
	background: 'paper',
	color: 'ink',
	borderRightWidth: '1px',
	borderRightStyle: 'solid',
	borderRightColor: 'rule',
	boxShadow: 'popup',
	outline: 'none',
	overflowY: 'auto',
	// Slides in from the edge it lives on; leaves faster than it arrives.
	_open: { animation: 'drawer-in' },
	_closed: { animation: 'drawer-out' }
});
export const drawerClose = css({
	position: 'absolute',
	top: '12px',
	right: '12px',
	display: 'flex',
	padding: '8px',
	borderRadius: 'sm',
	color: 'ink-muted',
	background: 'transparent',
	border: 'none',
	cursor: 'pointer',
	_hover: { color: 'ink', background: 'sunk' },
	_focusVisible: { outline: '2px solid {colors.ink}', outlineOffset: '2px' },
	'& svg': { width: '20px', height: '20px' }
});

// ----- admin page scaffolding (shared by every admin route) -----
export const adminPage = css({ display: 'flex', flexDirection: 'column', gap: '32px', maxWidth: '1040px' });
export const adminPageHead = css({
	display: 'flex',
	flexDirection: 'column',
	gap: '16px',
	paddingBottom: '24px',
	borderBottomWidth: '3px',
	borderBottomStyle: 'solid',
	borderBottomColor: 'ink',
	md: { flexDirection: 'row', alignItems: 'flex-end', justifyContent: 'space-between' }
});
export const adminTitle = css({ textStyle: 'display-sm', color: 'ink', md: { textStyle: 'display' } });
export const adminLede = css({ textStyle: 'body', color: 'ink-muted', marginTop: '8px', maxWidth: '60ch' });
export const sectionTitle = css({ textStyle: 'title', color: 'ink' });
export const sectionLede = css({ textStyle: 'body-sm', color: 'ink-muted', marginTop: '4px' });

// ----- primitive one-offs -----
export const textareaArea = css({ minHeight: 'auto', resize: 'vertical', lineHeight: '1.5' });
export const labelStyle = css({ textStyle: 'label', color: 'ink' });
/** Field label followed by its info tooltip trigger. */
export const fieldLabelRow = css({ display: 'flex', alignItems: 'center', gap: '4px' });
export const fieldInfoIcon = css({ display: 'inline-flex', '& svg': { width: '15px', height: '15px' } });
/** Status signal: a short line segment (solid, broken, or dashed). */
export const statusBadgeDot = css({
	display: 'inline-block',
	width: '16px',
	height: '0',
	borderTopWidth: '4px',
	borderRadius: '1px',
	flexShrink: 0
});
export const statusDotTone = {
	success: css({ borderTopStyle: 'solid', borderTopColor: 'ok' }),
	danger: css({ borderTopStyle: 'dotted', borderTopColor: 'danger' }),
	warning: css({ borderTopStyle: 'solid', borderTopColor: 'warn' }),
	neutral: css({ borderTopStyle: 'dashed', borderTopColor: 'ink-faint' })
} as const;
export const selectInvalid = css({ borderColor: 'danger' });
export const monoLabel = css({ fontFamily: 'mono', fontSize: '14px' });
export const collapsibleContent = css({ overflow: 'hidden' });
export const copyBtn = css({
	display: 'inline-flex',
	alignItems: 'center',
	justifyContent: 'center',
	width: '32px',
	height: '32px',
	borderRadius: 'sm',
	flexShrink: 0,
	color: 'ink-muted',
	background: 'transparent',
	border: 'none',
	cursor: 'pointer',
	transitionProperty: 'background, color',
	transitionDuration: '120ms',
	_hover: { color: 'ink', background: 'sunk' },
	_focusVisible: { outline: '2px solid {colors.ink}', outlineOffset: '1px' },
	'& svg': { width: '17px', height: '17px' }
});
export const copyOk = css({ color: 'ok', _hover: { color: 'ok' }, '& svg': { animation: 'check-in' } });
export const toastOkIcon = css({ color: 'inherit' });
export const toastBody = css({ minWidth: '0', flex: '1' });
export const themeToggleBtn = css({
	display: 'inline-flex',
	alignItems: 'center',
	justifyContent: 'center',
	width: '36px',
	height: '36px',
	borderRadius: 'full',
	color: 'ink-muted',
	background: 'transparent',
	border: 'none',
	cursor: 'pointer',
	transitionProperty: 'background, color',
	transitionDuration: '120ms',
	_hover: { color: 'ink', background: 'sunk' },
	_focusVisible: { outline: '2px solid {colors.ink}', outlineOffset: '2px' },
	'& svg': { width: '20px', height: '20px' }
});

export const confirmActions = css({
	display: 'flex',
	justifyContent: 'flex-end',
	gap: '8px',
	marginTop: '28px'
});

// ----- error page (routes/+error.svelte) -----
// The line runs out: the Looking Glass ring, solid ink, then a dotted
// no-service segment ending at an empty station named after the missing path.
export const errorPage = css({
	maxWidth: '1280px',
	width: '100%',
	marginInline: 'auto',
	display: 'flex',
	flexDirection: 'column',
	gap: '40px',
	padding: '48px 16px 80px',
	md: { padding: '96px 32px 144px', gap: '48px' }
});
export const lostMap = css({
	display: 'grid',
	gridTemplateColumns: 'auto minmax(48px, 2fr) minmax(48px, 3fr) auto',
	alignItems: 'center',
	rowGap: '12px',
	width: '100%',
	maxWidth: '560px'
});
export const lostOrigin = css({
	width: '24px',
	height: '24px',
	borderRadius: 'full',
	borderWidth: '5px',
	borderStyle: 'solid',
	borderColor: 'ink',
	background: 'paper'
});
export const lostTrack = css({ height: '6px', background: 'ink' });
export const lostGap = css({
	height: '0',
	marginInline: '6px',
	borderTopWidth: '6px',
	borderTopStyle: 'dotted',
	borderTopColor: 'rule-strong'
});
export const lostStop = css({
	width: '24px',
	height: '24px',
	borderRadius: 'full',
	borderWidth: '3px',
	borderStyle: 'dashed',
	borderColor: 'ink-muted'
});
export const lostFrom = css({ gridRow: '2', gridColumn: '1 / 3', justifySelf: 'start', textStyle: 'key', color: 'ink' });
export const lostTo = css({
	gridRow: '2',
	gridColumn: '3 / 5',
	justifySelf: 'end',
	maxWidth: '100%',
	overflow: 'hidden',
	textOverflow: 'ellipsis',
	whiteSpace: 'nowrap',
	fontFamily: 'mono',
	fontSize: '13px',
	lineHeight: '16px',
	color: 'ink-muted'
});
export const errorText = css({ display: 'flex', flexDirection: 'column', gap: '12px' });
export const errorTitle = css({ textStyle: 'display-sm', color: 'ink', md: { textStyle: 'display' } });
export const errorLede = css({ textStyle: 'body', color: 'ink-muted', maxWidth: '52ch' });
export const errorPath = css({ textStyle: 'data', color: 'ink', overflowWrap: 'anywhere' });
export const errorActions = css({ display: 'flex', flexWrap: 'wrap', alignItems: 'center', gap: '12px' });
