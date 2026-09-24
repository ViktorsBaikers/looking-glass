// Parse streamed `traceroute` output into hop rows for the route view. Works on
// partial output: every complete hop line seen so far becomes a row.

export type TraceRow = {
	hop: number;
	/** Hostname (or the address with -n); '*' when no probe answered. */
	host: string;
	/** Address in parentheses when traceroute resolved a name; else ''. */
	address: string;
	/** Probe round-trip times in ms, '*' for a lost probe. */
	rtts: string[];
};

// " 1  router.example (192.0.2.1)  1.045 ms  1.012 ms  0.998 ms"
// " 2  198.51.100.1  4.221 ms * 4.177 ms"
// " 3  * * *"
const HOP = /^\s*(\d+)\s+(?:(\S+)(?:\s+\(([^)]+)\))?\s+)?((?:(?:\*|[\d.]+ ms)\s*)+)$/;
const PROBE = /\*|[\d.]+(?= ms)/g;

export function parseTraceroute(lines: string[]): TraceRow[] | null {
	const rows: TraceRow[] = [];
	for (const line of lines) {
		const match = HOP.exec(line);
		if (!match) continue;
		const [, hop, name, address, probes] = match;
		// "3  * * *": the first '*' is a lost probe, not a host.
		const silent = name === undefined || name === '*';
		const rtts = [...(silent && name ? `* ${probes}` : probes).matchAll(PROBE)].map((p) => p[0]);
		rows.push({
			hop: Number(hop),
			host: silent ? '*' : name,
			address: address && address !== name ? address : '',
			rtts
		});
	}
	return rows.length > 0 ? rows : null;
}
