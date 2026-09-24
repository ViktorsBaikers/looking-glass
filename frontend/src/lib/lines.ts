// Every Location is drawn as a transit line: a fixed colour and a short roundel
// code. Colour is keyed off the Location id, so a Location keeps its line on the
// public page and in admin, whatever else is listed.
// ponytail: id hash can give two Locations one colour; assign by admin order if
// operators run more than a handful of Locations.

/** Light-map colour, night-map colour. Roundel text is white on light, ink on night. */
const LINES: readonly (readonly [string, string, string])[] = [
	['#c8102e', '#ff6b6b', '#ffffff'], // red
	['#0b5cad', '#5ea8ff', '#ffffff'], // blue
	['#00783a', '#45cf85', '#ffffff'], // green
	['#8a6500', '#f5c842', '#ffffff'], // ochre
	['#6b3fa0', '#b48cff', '#ffffff'], // violet
	['#00747a', '#35c6cb', '#ffffff'], // teal
	['#b34a0c', '#ff9147', '#ffffff'], // orange
	['#b0006d', '#ff66b8', '#ffffff'], // magenta
	['#7a4a1e', '#d19a5f', '#ffffff'] // brown
];

function hash(id: string): number {
	let h = 0x811c9dc5;
	for (let i = 0; i < id.length; i++) {
		h ^= id.charCodeAt(i);
		h = Math.imul(h, 0x01000193);
	}
	return h >>> 0;
}

/**
 * Inline style for an element carrying `data-line`: global CSS resolves it to
 * `--line` (stroke/fill) and `--line-ink` (roundel text) for the active theme.
 */
export function lineStyle(id: string): string {
	const [light, dark, ink] = LINES[hash(id) % LINES.length];
	return `--line-light: ${light}; --line-dark: ${dark}; --line-ink-light: ${ink}`;
}

/** Roundel code: "Frankfurt" → "FRA", "San Francisco Hub" → "SFH". */
export function lineCode(name: string): string {
	const words = name.toUpperCase().match(/[\p{L}\p{N}]+/gu) ?? [];
	if (words.length === 0) return '·';
	if (words.length === 1) return words[0].slice(0, 3);
	return words
		.slice(0, 3)
		.map((word) => word[0])
		.join('');
}
