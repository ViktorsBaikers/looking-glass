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
export const shell = css({ display: 'flex', flexDirection: 'column', minHeight: '100vh' });
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
	fontSize: '15px',
	lineHeight: '20px',
	fontWeight: 800,
	md: { fontSize: '17px' },
	letterSpacing: '-0.015em',
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
		height: 'calc(100vh - 56px)',
		overflowY: 'auto',
		padding: '40px 24px 24px 32px'
	}
});
export const heading = css({
	textStyle: 'caption',
	fontWeight: 700,
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
	fontSize: '15px',
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
	animation: 'fade-in'
});
export const drawerPositioner = css({
	position: 'fixed',
	inset: '0',
	display: 'flex',
	justifyContent: 'flex-start',
	zIndex: 50
});
export const drawerContent = css({
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
	animation: 'content-in'
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
export const copyOk = css({ color: 'ok', _hover: { color: 'ok' } });
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
