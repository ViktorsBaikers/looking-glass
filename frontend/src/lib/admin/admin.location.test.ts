// Seam 3 (pure logic) for the Location editor: ASN client validation must agree
// word-for-word with central's 400 `invalid_asn`, and the Methods grid must split
// the eight supported methods by family.
import { describe, expect, it } from 'vitest';
import { asnError, asnValue, methodsByFamily, nullIfBlank, ASN_MESSAGE } from './editor.js';

describe('ASN client validation', () => {
	it('accepts the whole 1..4294967295 range and trims whitespace', () => {
		expect(asnValue('1')).toBe(1);
		expect(asnValue('64501')).toBe(64501);
		expect(asnValue(' 4294967295 ')).toBe(4294967295);
		expect(asnError('64501')).toBeNull();
	});

	it('treats an empty value as unset', () => {
		expect(asnValue('')).toBeNull();
		expect(asnValue('   ')).toBeNull();
		expect(asnError('')).toBeNull();
	});

	it.each(['0', '4294967296', '-3', '1.5', 'abc', '64 501', '0x10'])(
		'rejects %s with the coded server message',
		(raw) => {
			expect(asnValue(raw)).toBeNull();
			expect(asnError(raw)).toBe(ASN_MESSAGE);
		}
	);
});

describe('method families', () => {
	it('splits the eight supported methods into v4 and v6 groups', () => {
		expect(methodsByFamily()).toEqual({
			v4: ['ping', 'mtr', 'traceroute', 'bgp'],
			v6: ['ping6', 'mtr6', 'traceroute6', 'bgp6']
		});
	});
});

describe('nullIfBlank', () => {
	it('maps empty optional fields to null and keeps the rest', () => {
		expect(nullIfBlank('')).toBeNull();
		expect(nullIfBlank('  ')).toBeNull();
		expect(nullIfBlank('Equinix FR2')).toBe('Equinix FR2');
	});
});
