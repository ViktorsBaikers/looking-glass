import { describe, expect, it } from 'vitest';
import { parseTraceroute } from './trace.js';

describe('parseTraceroute', () => {
	it('parses named, bare, partial and silent hops', () => {
		const rows = parseTraceroute([
			'traceroute to 1.1.1.1 (1.1.1.1), 30 hops max, 60 byte packets',
			' 1  gw.example.net (192.0.2.1)  1.045 ms  1.012 ms  0.998 ms',
			' 2  198.51.100.1 (198.51.100.1)  4.221 ms * 4.177 ms',
			' 3  * * *',
			' 4  1.1.1.1  12.043 ms  12.001 ms  11.987 ms'
		]);
		expect(rows).toEqual([
			{ hop: 1, host: 'gw.example.net', address: '192.0.2.1', rtts: ['1.045', '1.012', '0.998'] },
			{ hop: 2, host: '198.51.100.1', address: '', rtts: ['4.221', '*', '4.177'] },
			{ hop: 3, host: '*', address: '', rtts: ['*', '*', '*'] },
			{ hop: 4, host: '1.1.1.1', address: '', rtts: ['12.043', '12.001', '11.987'] }
		]);
	});

	it('returns null for output with no hop lines', () => {
		expect(parseTraceroute(['64 bytes from 1.1.1.1: icmp_seq=1 ttl=56 time=12.3 ms'])).toBeNull();
		expect(parseTraceroute(['traceroute to 1.1.1.1 (1.1.1.1), 30 hops max'])).toBeNull();
	});
});
