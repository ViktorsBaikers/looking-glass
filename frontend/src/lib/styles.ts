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
	width: 'full',
	background: 'surface',
	borderBottomWidth: '1px',
	borderBottomStyle: 'solid',
	borderBottomColor: 'outline-variant'
});
export const headerInner = css({
	display: 'flex',
	alignItems: 'center',
	justifyContent: 'space-between',
	gap: '16px',
	flexWrap: 'wrap',
	width: 'full',
	maxWidth: '1200px',
	margin: '0 auto',
	minHeight: '64px',
	padding: '12px 16px',
	md: { padding: '12px 32px' }
});
export const brand = css({
	display: 'flex',
	alignItems: 'center',
	gap: '12px',
	textDecoration: 'none',
	color: 'on-surface',
	textStyle: 'headline-sm',
	fontWeight: 700
});
export const statusDot = css({
	width: '10px',
	height: '10px',
	borderRadius: 'full',
	background: 'primary',
	boxShadow: 'glow',
	flexShrink: 0
});
export const logo = css({ width: '28px', height: '28px', objectFit: 'contain' });
export const nav = css({ display: 'flex', alignItems: 'center', gap: '16px', md: { gap: '24px' } });
export const navLink = css({
	textStyle: 'label-md',
	color: 'on-surface-variant',
	textDecoration: 'none',
	paddingBottom: '2px',
	borderBottomWidth: '2px',
	borderBottomStyle: 'solid',
	borderBottomColor: 'transparent',
	transitionProperty: 'color, border-color',
	transitionDuration: '150ms',
	_hover: { color: 'primary' },
	_focusVisible: { outline: '2px solid {colors.primary}', outlineOffset: '2px' }
});
export const navLinkActive = css({ color: 'primary', fontWeight: 700, borderBottomColor: 'primary' });
export const headerRight = css({ display: 'flex', alignItems: 'center', gap: '8px' });
export const mainArea = css({ flex: '1', width: 'full', minWidth: '0' });
export const footer = css({
	borderTopWidth: '1px',
	borderTopStyle: 'solid',
	borderTopColor: 'outline-variant',
	background: 'surface'
});
export const footerInner = css({
	display: 'flex',
	flexDirection: 'column',
	alignItems: 'center',
	gap: '8px',
	width: 'full',
	maxWidth: '1200px',
	margin: '0 auto',
	padding: '24px 16px',
	textAlign: 'center',
	md: { padding: '24px 32px' }
});
export const termsLink = css({
	textStyle: 'body-sm',
	color: 'primary',
	textDecoration: 'underline',
	textUnderlineOffset: '3px',
	_hover: { color: 'on-surface' }
});
export const customText = css({ textStyle: 'body-sm', color: 'on-surface-variant' });

// ----- admin shell (routes/admin/+layout.svelte) -----
export const checking = css({
	display: 'flex',
	alignItems: 'center',
	justifyContent: 'center',
	minHeight: '50vh',
	color: 'on-surface-variant',
	'& svg': { width: '24px', height: '24px' }
});
export const adminShell = css({
	display: 'flex',
	alignItems: 'flex-start',
	width: 'full',
	minHeight: '100%'
});
export const sidebar = css({
	display: 'none',
	md: {
		display: 'flex',
		position: 'sticky',
		top: '64px',
		alignSelf: 'flex-start',
		flexDirection: 'column',
		gap: '8px',
		width: '240px',
		flexShrink: 0,
		maxHeight: 'calc(100vh - 64px)',
		overflowY: 'auto',
		padding: '24px 16px',
		background: 'surface-container-low',
		borderRightWidth: '1px',
		borderRightStyle: 'solid',
		borderRightColor: 'outline-variant'
	}
});
export const heading = css({ marginBottom: '24px', paddingInline: '12px' });
export const headingTitle = css({ textStyle: 'headline-sm', color: 'primary', fontWeight: 700 });
export const headingSub = css({ textStyle: 'body-sm', color: 'on-surface-variant' });
export const navList = css({ display: 'flex', flexDirection: 'column', gap: '8px', flex: '1' });
export const navItem = css({
	display: 'flex',
	alignItems: 'center',
	gap: '12px',
	padding: '10px 12px',
	borderRadius: 'lg',
	textStyle: 'label-md',
	textDecoration: 'none',
	color: 'on-surface-variant',
	background: 'transparent',
	border: 'none',
	width: 'full',
	cursor: 'pointer',
	textAlign: 'left',
	transitionProperty: 'background, color',
	transitionDuration: '150ms',
	_hover: { background: 'surface-container-highest', color: 'primary' },
	_focusVisible: { outline: '2px solid {colors.primary}', outlineOffset: '-2px' },
	'& svg': { width: '20px', height: '20px', flexShrink: 0 }
});
export const navItemActive = css({
	background: 'secondary-container',
	color: 'on-secondary-container',
	fontWeight: 600,
	_hover: { background: 'secondary-container', color: 'on-secondary-container' }
});
export const logoutWrap = css({ marginTop: 'auto', paddingTop: '16px' });
export const contentCol = css({ flex: '1', minWidth: '0', display: 'flex', flexDirection: 'column' });
export const mobileBar = css({
	display: 'flex',
	padding: '12px 16px',
	borderBottomWidth: '1px',
	borderBottomStyle: 'solid',
	borderBottomColor: 'outline-variant',
	md: { display: 'none' }
});
export const menuBtn = css({
	display: 'inline-flex',
	alignItems: 'center',
	justifyContent: 'center',
	width: '40px',
	height: '40px',
	borderRadius: 'md',
	color: 'on-surface-variant',
	background: 'transparent',
	border: 'none',
	cursor: 'pointer',
	_hover: { color: 'primary', background: 'surface-container-high' },
	_focusVisible: { outline: '2px solid {colors.primary}', outlineOffset: '2px' },
	'& svg': { width: '24px', height: '24px' }
});
export const content = css({ flex: '1', minWidth: '0', padding: '24px 16px', md: { padding: '32px' } });
export const drawerBackdrop = css({
	position: 'fixed',
	inset: '0',
	background: 'black/60',
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
	gap: '8px',
	width: '280px',
	maxWidth: '85vw',
	height: 'full',
	padding: '24px 16px',
	background: 'surface-container-low',
	color: 'on-surface',
	borderRightWidth: '1px',
	borderRightStyle: 'solid',
	borderRightColor: 'outline-variant',
	outline: 'none',
	overflowY: 'auto'
});
export const drawerClose = css({
	position: 'absolute',
	top: '12px',
	right: '12px',
	display: 'flex',
	padding: '4px',
	borderRadius: 'md',
	color: 'on-surface-variant',
	background: 'transparent',
	border: 'none',
	cursor: 'pointer',
	_hover: { color: 'on-surface', background: 'surface-container-high' },
	'& svg': { width: '20px', height: '20px' }
});

// ----- primitive one-offs -----
export const textareaArea = css({ minHeight: 'auto', resize: 'vertical', lineHeight: '1.5' });
export const labelStyle = css({ textStyle: 'label-md', color: 'on-surface-variant' });
export const statusBadgeDot = css({
	width: '8px',
	height: '8px',
	borderRadius: 'full',
	flexShrink: 0
});
export const statusDotTone = {
	success: css({ background: 'primary', boxShadow: 'glow' }),
	danger: css({ background: 'error' }),
	warning: css({ background: 'warning' }),
	neutral: css({ background: 'outline' })
} as const;
export const selectInvalid = css({ borderColor: 'error' });
export const monoLabel = css({ fontFamily: 'mono', fontSize: '14px' });
export const collapsibleContent = css({ overflow: 'hidden' });
export const copyBtn = css({
	display: 'inline-flex',
	alignItems: 'center',
	justifyContent: 'center',
	width: '40px',
	height: '40px',
	borderRadius: 'md',
	flexShrink: 0,
	color: 'on-surface-variant',
	background: 'transparent',
	border: 'none',
	cursor: 'pointer',
	transitionProperty: 'background, color',
	transitionDuration: '150ms',
	_hover: { color: 'primary', background: 'surface-container-high' },
	_focusVisible: { outline: '2px solid {colors.primary}', outlineOffset: '2px' },
	'& svg': { width: '20px', height: '20px' }
});
export const copyOk = css({ color: 'primary' });
export const toastOkIcon = css({ color: 'primary' });
export const toastBody = css({ minWidth: '0', flex: '1' });
export const themeToggleBtn = css({
	display: 'inline-flex',
	alignItems: 'center',
	justifyContent: 'center',
	width: '40px',
	height: '40px',
	borderRadius: 'full',
	color: 'on-surface-variant',
	background: 'transparent',
	border: 'none',
	cursor: 'pointer',
	transitionProperty: 'background, color',
	transitionDuration: '150ms',
	_hover: { color: 'primary', background: 'surface-variant' },
	_focusVisible: { outline: '2px solid {colors.primary}', outlineOffset: '2px' },
	'& svg': { width: '22px', height: '22px' }
});

export const confirmActions = css({
	display: 'flex',
	justifyContent: 'flex-end',
	gap: '8px',
	marginTop: '24px'
});
