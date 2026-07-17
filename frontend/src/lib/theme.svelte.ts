import { browser } from '$app/environment';
import type { PublicTheme } from '$lib/public/settings.js';

function createTheme() {
	let dark = $state(browser && document.documentElement.classList.contains('dark'));

	return {
		get dark() {
			return dark;
		},
		toggle() {
			dark = !dark;
			document.documentElement.classList.toggle('dark', dark);
			localStorage.setItem('theme', dark ? 'dark' : 'light');
		},
		applyDefault(defaultTheme: PublicTheme) {
			if (!browser) return;
			const stored = localStorage.getItem('theme');
			const preference = stored === 'light' || stored === 'dark' ? stored : defaultTheme;
			dark =
				preference === 'dark' ||
				(preference === 'system' && window.matchMedia('(prefers-color-scheme: dark)').matches);
			document.documentElement.classList.toggle('dark', dark);
		}
	};
}

export const theme = createTheme();
