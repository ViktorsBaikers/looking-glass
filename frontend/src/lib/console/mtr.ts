// Parse `mtr --report -n` output into structured hop rows so the console can
// render the signature table view instead of raw text. Returns null when the
// lines are not mtr report output (e.g. ping/traceroute), so the caller falls
// back to plain text.

export type MtrRow = {
	hop: number;
	host: string;
	lossPct: string;
	sent: string;
	last: string;
	avg: string;
	best: string;
	worst: string;
	stdev: string;
};

// A hop line: "  1.|-- 192.168.1.1   0.0%   4   1.2   1.3   1.1   1.5   0.2"
const HOP = /^\s*(\d+)\.\|--\s+(\S+)\s+(.+)$/;

export function parseMtr(lines: string[]): MtrRow[] | null {
	const rows: MtrRow[] = [];
	for (const line of lines) {
		const match = HOP.exec(line);
		if (!match) continue;
		const columns = match[3].trim().split(/\s+/);
		if (columns.length < 7) continue;
		rows.push({
			hop: Number(match[1]),
			host: match[2],
			lossPct: columns[0],
			sent: columns[1],
			last: columns[2],
			avg: columns[3],
			best: columns[4],
			worst: columns[5],
			stdev: columns[6]
		});
	}
	return rows.length > 0 ? rows : null;
}
