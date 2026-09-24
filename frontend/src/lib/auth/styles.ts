// One-off atomic styles for the auth screens (login / install / activate) and
// the one-time activation link display. Panda's extractor reads css() from .ts
// only, never from .svelte templates.
import { css } from 'styled-system/css';

/**
 * Auth routes render without shell chrome. The panel is a terminus: one ink
 * line runs in from the viewport's left edge and ends at an interchange ring
 * on the panel's edge, level with its heading.
 */
export const authPage = css({
	display: 'flex',
	alignItems: 'center',
	justifyContent: 'center',
	width: '100%',
	minHeight: '100vh',
	padding: '48px 24px',
	overflowX: 'hidden',
	md: { justifyContent: 'flex-start', paddingLeft: 'max(96px, calc(34vw - 224px))' }
});

export const authCard = css({
	position: 'relative',
	width: '100%',
	maxWidth: '440px',
	md: { padding: '32px 36px 36px' },
	_before: {
		content: '""',
		position: 'absolute',
		right: '100%',
		top: '37px',
		width: '100vw',
		height: '6px',
		background: 'ink',
		md: { top: '45px' }
	},
	_after: {
		content: '""',
		position: 'absolute',
		left: '-14px',
		top: '26px',
		width: '28px',
		height: '28px',
		borderRadius: 'full',
		borderWidth: '6px',
		borderStyle: 'solid',
		borderColor: 'ink',
		background: 'paper',
		md: { top: '34px' }
	}
});

export const authTitle = css({ textStyle: 'display-sm' });

export const formStack = css({ display: 'flex', flexDirection: 'column', gap: '18px' });

export const formErrorText = css({ textStyle: 'body-sm', color: 'danger', fontWeight: 600 });

export const fullWidth = css({ width: '100%', marginTop: '6px' });

/** The one-time activation link, shown once inside a dialog. */
export const activationLink = css({
	fontFamily: 'mono',
	fontSize: '13px',
	lineHeight: '20px',
	color: 'ink',
	background: 'sunk',
	padding: '12px 14px',
	wordBreak: 'break-all'
});

export const activationRow = css({
	display: 'flex',
	alignItems: 'center',
	justifyContent: 'space-between',
	gap: '8px',
	marginTop: '12px'
});

export const activationNote = css({ textStyle: 'body-sm', color: 'ink-muted' });

/** The pending peer's username on the activation card. */
export const activateWho = css({
	fontFamily: 'mono',
	fontSize: '14px',
	fontWeight: 600,
	color: 'ink',
	marginBottom: '18px'
});

export const activateMessage = css({ textStyle: 'body', color: 'ink-muted' });
