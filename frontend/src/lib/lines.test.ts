// Every line colour is also text (run-output times, roundel codes), so each must
// reach WCAG AA 4.5:1 wherever it lands, in both themes.
import { describe, expect, it } from 'vitest';
import { LINES } from './lines.js';

function luminance(hex: string): number {
	const [r, g, b] = [1, 3, 5].map((i) => {
		const s = parseInt(hex.slice(i, i + 2), 16) / 255;
		return s <= 0.03928 ? s / 12.92 : ((s + 0.055) / 1.055) ** 2.4;
	});
	return 0.2126 * r + 0.7152 * g + 0.0722 * b;
}

function contrast(a: string, b: string): number {
	const [x, y] = [luminance(a), luminance(b)];
	return (Math.max(x, y) + 0.05) / (Math.min(x, y) + 0.05);
}

// panda.config.ts semantic tokens: `sunk` is the darkest light surface (the
// console), `panel` the lightest dark one; #111214 is the dark roundel ink.
const LIGHT_SUNK = '#ebebe7';
const DARK_PANEL = '#18191c';
const DARK_INK = '#111214';

describe('line palette contrast', () => {
	it.each(LINES)('%s / %s reaches 4.5:1 in both themes', (light, dark, ink) => {
		expect(contrast(light, LIGHT_SUNK), 'light line on sunk').toBeGreaterThanOrEqual(4.5);
		expect(contrast(ink, light), 'light roundel text').toBeGreaterThanOrEqual(4.5);
		expect(contrast(dark, DARK_PANEL), 'dark line on panel').toBeGreaterThanOrEqual(4.5);
		expect(contrast(DARK_INK, dark), 'dark roundel text').toBeGreaterThanOrEqual(4.5);
	});
});
