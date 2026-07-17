import { describe, it, expect } from 'vitest';
import { formatLastSeen } from './lastSeen.js';

// A fixed "now" so every case is deterministic. 1_000_000s in ms.
const NOW_MS = 1_000_000 * 1000;

describe('formatLastSeen', () => {
	it('reports a never-seen agent', () => {
		expect(formatLastSeen(null, NOW_MS)).toBe('Never');
		expect(formatLastSeen(undefined, NOW_MS)).toBe('Never');
	});

	it('reports a beat within the last minute as just now', () => {
		expect(formatLastSeen(1_000_000, NOW_MS)).toBe('Just now'); // 0s ago
		expect(formatLastSeen(1_000_000 - 59, NOW_MS)).toBe('Just now');
	});

	it('reports minutes for a beat under an hour old', () => {
		expect(formatLastSeen(1_000_000 - 60, NOW_MS)).toBe('1m ago');
		expect(formatLastSeen(1_000_000 - 59 * 60, NOW_MS)).toBe('59m ago');
	});

	it('reports hours for a beat under a day old', () => {
		expect(formatLastSeen(1_000_000 - 60 * 60, NOW_MS)).toBe('1h ago');
		expect(formatLastSeen(1_000_000 - 23 * 3600, NOW_MS)).toBe('23h ago');
	});

	it('reports days for an older beat', () => {
		expect(formatLastSeen(1_000_000 - 24 * 3600, NOW_MS)).toBe('1d ago');
		expect(formatLastSeen(1_000_000 - 5 * 86400, NOW_MS)).toBe('5d ago');
	});

	it('treats a future timestamp (clock skew) as current, never negative', () => {
		expect(formatLastSeen(1_000_000 + 30, NOW_MS)).toBe('Just now');
	});
});
