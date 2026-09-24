// Pure helpers for the Location editor (issue #7): ASN client validation that
// agrees word-for-word with central's 400 `invalid_asn`, the Methods grid's
// family split, and the enrollment countdown. No DOM, no fetch — unit-tested in
// admin.location.test.ts and enroll.test.ts.
import { OFFERED_METHODS, type OfferedMethod } from './types.js';

export const ASN_MESSAGE = 'ASN must be a whole number between 1 and 4294967295.';

/** '' → null (unset); a whole number in 1..4294967295 → that number; anything
 * else null + [`ASN_MESSAGE`] via [`asnError`], mirroring clean_asn in central. */
export function asnValue(raw: string): number | null {
	const trimmed = raw.trim();
	if (trimmed === '') return null;
	if (!/^\d+$/.test(trimmed)) return null;
	const value = Number(trimmed);
	return value >= 1 && value <= 4294967295 ? value : null;
}

export function asnError(raw: string): string | null {
	return raw.trim() !== '' && asnValue(raw) === null ? ASN_MESSAGE : null;
}

/** The eight supported methods grouped by family: the `6` suffix is the v6 twin. */
export function methodsByFamily(): Record<'v4' | 'v6', OfferedMethod[]> {
	return {
		v4: OFFERED_METHODS.filter((method) => !method.endsWith('6')),
		v6: OFFERED_METHODS.filter((method) => method.endsWith('6'))
	};
}

/** mm:ss for the enrollment ticket's remaining lifetime. */
export function formatCountdown(remainingMs: number): string {
	const total = Math.max(0, Math.floor(remainingMs / 1000));
	const mm = String(Math.floor(total / 60)).padStart(2, '0');
	const ss = String(total % 60).padStart(2, '0');
	return `${mm}:${ss}`;
}

export function nullIfBlank(value: string): string | null {
	return value.trim() === '' ? null : value;
}
