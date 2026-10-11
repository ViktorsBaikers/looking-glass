import { describe, expect, it } from 'vitest';
import { parseMtr } from './mtr.js';

describe('parseMtr', () => {
	it('parses report hop lines into rows', () => {
		const rows = parseMtr([
			'HOST: mtr --report 1.1.1.1   Loss%   Snt   Last   Avg  Best  Wrst StDev',
			'  1.|-- 192.0.2.1             0.0%    10    1.1   1.2   1.0   1.5   0.2',
			'  2.|-- 1.1.1.1               0.0%    10   12.0  12.2  11.8  12.9   0.4'
		]);
		expect(rows).toEqual([
			{
				hop: 1,
				host: '192.0.2.1',
				lossPct: '0.0%',
				sent: '10',
				last: '1.1',
				avg: '1.2',
				best: '1.0',
				worst: '1.5',
				stdev: '0.2'
			},
			{
				hop: 2,
				host: '1.1.1.1',
				lossPct: '0.0%',
				sent: '10',
				last: '12.0',
				avg: '12.2',
				best: '11.8',
				worst: '12.9',
				stdev: '0.4'
			}
		]);
	});

	it('returns null for non-mtr output so callers fall back to plain text', () => {
		expect(parseMtr(['64 bytes from 1.1.1.1: icmp_seq=1 ttl=56 time=12.3 ms'])).toBeNull();
		expect(parseMtr([])).toBeNull();
	});

	it('skips hop lines with too few columns', () => {
		expect(parseMtr(['  1.|-- 192.0.2.1   0.0%   10'])).toBeNull();
	});
});
