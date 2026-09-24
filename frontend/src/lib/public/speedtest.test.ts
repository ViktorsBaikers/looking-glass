import { describe, expect, it } from 'vitest';
import { declaredSizeBytes, largestTestFile, mbps } from './speedtest.js';
import type { TestFile } from '../admin/types.js';

function file(id: string, declared: string): TestFile {
	return { id, location_id: 'fra', label: id, declared_size: declared, source_ref: `${id}.bin` };
}

describe('declaredSizeBytes', () => {
	it('parses decimal units', () => {
		expect(declaredSizeBytes('100 MB')).toBe(100_000_000);
		expect(declaredSizeBytes('10 MB')).toBe(10_000_000);
		expect(declaredSizeBytes('1.5 GB')).toBe(1_500_000_000);
		expect(declaredSizeBytes('512 KB')).toBe(512_000);
	});

	it('parses binary units', () => {
		expect(declaredSizeBytes('1 GiB')).toBe(1024 ** 3);
		expect(declaredSizeBytes('2 MiB')).toBe(2 * 1024 ** 2);
	});

	it('returns 0 for unparseable declarations', () => {
		expect(declaredSizeBytes('')).toBe(0);
		expect(declaredSizeBytes('huge')).toBe(0);
		expect(declaredSizeBytes('10 parsecs')).toBe(0);
	});
});

describe('largestTestFile', () => {
	it('picks the largest declared size regardless of order', () => {
		const files = [file('small', '10 MB'), file('big', '100 MB'), file('mid', '50 MB')];
		expect(largestTestFile(files)?.id).toBe('big');
	});

	it('returns null when there are no files', () => {
		expect(largestTestFile([])).toBeNull();
	});
});

describe('mbps', () => {
	it('converts bytes over elapsed time to whole megabits per second', () => {
		expect(mbps(12_500_000, 1000)).toBe(100); // 100 Mbit in 1 s
		expect(mbps(1_250_000, 2000)).toBe(5); // 10 Mbit in 2 s
	});

	it('guards against zero or negative inputs', () => {
		expect(mbps(1000, 0)).toBe(0);
		expect(mbps(0, 1000)).toBe(0);
	});
});
