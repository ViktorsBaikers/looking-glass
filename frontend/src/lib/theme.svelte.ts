import { browser } from '$app/environment';
import type { PublicTheme } from '$lib/public/settings.js';

/** Paper colour per theme: mobile browsers tint their chrome to match. */
const PAPER = { light: '#f3f3f0', dark: '#111214' };

function paint(dark: boolean) {
	document.documentElement.classList.toggle('dark', dark);
	document
		.querySelector('meta[name="theme-color"]')
		?.setAttribute('content', dark ? PAPER.dark : PAPER.light);
}

function createTheme() {
	let dark = $state(browser && document.documentElement.classList.contains('dark'));

	return {
		get dark() {
			return dark;
		},
		toggle() {
			dark = !dark;
			paint(dark);
			localStorage.setItem('theme', dark ? 'dark' : 'light');
		},
		applyDefault(defaultTheme: PublicTheme) {
			if (!browser) return;
			const stored = localStorage.getItem('theme');
			const preference = stored === 'light' || stored === 'dark' ? stored : defaultTheme;
			dark =
				preference === 'dark' ||
				(preference === 'system' && window.matchMedia('(prefers-color-scheme: dark)').matches);
			paint(dark);
		}
	};
}

export const theme = createTheme();
