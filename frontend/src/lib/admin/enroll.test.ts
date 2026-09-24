// Seam 3 (pure logic) for the Enrollment tab: the ticket expiry countdown.
import { describe, expect, it } from 'vitest';
import { formatCountdown } from './editor.js';

describe('enrollment countdown', () => {
	it('renders whole minutes and seconds as mm:ss', () => {
		expect(formatCountdown(125_000)).toBe('02:05');
		expect(formatCountdown(900_000)).toBe('15:00');
		expect(formatCountdown(5_000)).toBe('00:05');
	});

	it('clamps at zero once the ticket has expired', () => {
		expect(formatCountdown(0)).toBe('00:00');
		expect(formatCountdown(-5_000)).toBe('00:00');
	});

	it('truncates partial seconds', () => {
		expect(formatCountdown(1_999)).toBe('00:01');
	});
});
