// One-off atomic styles for the centred auth cards (login / install /
// activate) and the one-time activation link display. Panda's extractor reads
// css() from .ts only, never from .svelte templates.
import { css } from 'styled-system/css';

/** Centred single-card page: auth routes render without shell chrome. */
export const authPage = css({
	display: 'flex',
	alignItems: 'center',
	justifyContent: 'center',
	width: '100%',
	minHeight: '100vh',
	padding: '24px'
});

export const authCard = css({ width: '100%', maxWidth: '448px' });

export const formStack = css({ display: 'flex', flexDirection: 'column', gap: '16px' });

export const formErrorText = css({ textStyle: 'body-sm', color: 'error' });

export const fullWidth = css({ width: '100%' });

/** The one-time activation link, shown once inside a dialog. */
export const activationLink = css({
	fontFamily: 'mono',
	textStyle: 'mono-data',
	color: 'primary-fixed-dim',
	background: 'surface-container-lowest',
	borderWidth: '1px',
	borderStyle: 'solid',
	borderColor: 'outline-variant',
	borderRadius: 'md',
	padding: '12px',
	wordBreak: 'break-all'
});

export const activationRow = css({
	display: 'flex',
	alignItems: 'center',
	justifyContent: 'space-between',
	gap: '8px',
	marginTop: '8px'
});

export const activationNote = css({ textStyle: 'body-sm', color: 'on-surface-variant' });

/** The pending peer's username on the activation card. */
export const activateWho = css({
	fontFamily: 'mono',
	textStyle: 'mono-data',
	color: 'primary-fixed-dim',
	marginBottom: '16px'
});

export const activateMessage = css({ textStyle: 'body-md', color: 'on-surface-variant' });
