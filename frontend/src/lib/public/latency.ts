// A client-side round-trip probe to the node, shown as the network block's
// latency readout. It times a few lightweight requests and keeps the fastest —
// the minimum RTT is the cleanest estimate, least perturbed by scheduling jitter
// or a warming connection.

export async function measureLatency(samples = 4): Promise<number | null> {
	let best: number | null = null;
	for (let i = 0; i < samples; i++) {
		const start = performance.now();
		try {
			await fetch('/api/visitor', { cache: 'no-store' });
		} catch {
			return best;
		}
		const rtt = performance.now() - start;
		if (best === null || rtt < best) best = rtt;
	}
	return best === null ? null : Math.round(best);
}
