// Metric cards are derived client-side from finished Run output (spec #1):
// ping → latency/loss/jitter/TTL, mtr → final-hop avg/loss/stdev + hop count,
// traceroute → hop count, BGP → none. Pure so vitest can pin the parsing.

import { parseMtr } from './mtr.js';

export interface Metric {
	label: string;
	value: string;
	unit?: string;
	caption?: string;
	tooltip: string;
}

// "rtt min/avg/max/mdev = 11.9/12.1/12.3/0.2 ms"
const RTT = /(?:rtt|round-trip)\s+min\/avg\/max\/(?:mdev|stddev)\s*=\s*([\d.]+)\/([\d.]+)\/([\d.]+)\/([\d.]+)/i;
// "4 packets transmitted, 4 received, 0% packet loss"
const LOSS = /(\d+)\s+packets?\s+transmitted,\s+(\d+)\s+(?:packets?\s+)?received/i;
// "icmp_seq=4 ttl=56"
const TTL = /\bttl=(\d+)\b/i;
// A traceroute hop line: " 1  host ..." or " 4  * * *"
const TRACEROUTE_HOP = /^\s*\d+\s+\S/;

function pingMetrics(lines: string[]): Metric[] {
	// The statistics line arrives even when every probe timed out ("4 packets
	// transmitted, 0 received, 100% packet loss" with no rtt line), so packet
	// loss is derived on its own; latency/jitter need replies' rtt line and
	// TTL a reply line.
	const loss = lines.map((line) => LOSS.exec(line)).find((match) => match);
	if (!loss) return [];
	const rtt = lines.map((line) => RTT.exec(line)).find((match) => match);

	const ttl = lines
		.map((line) => TTL.exec(line))
		.filter((match): match is RegExpExecArray => match !== null)
		.pop();

	const transmitted = Number(loss[1]);
	const received = Number(loss[2]);
	const metrics: Metric[] = [];
	if (rtt) {
		metrics.push({
			label: 'Latency',
			value: rtt[2],
			unit: 'ms',
			caption: 'avg',
			tooltip: 'Average round-trip time across all replies.'
		});
	}
	metrics.push({
		label: 'Packet loss',
		value: String(Math.round(((transmitted - received) / Math.max(transmitted, 1)) * 100)),
		unit: '%',
		caption: `${transmitted - received}/${transmitted}`,
		tooltip: 'Packets lost versus packets sent.'
	});
	if (rtt) {
		metrics.push({
			label: 'Jitter',
			value: rtt[4],
			unit: 'ms',
			caption: 'mdev',
			tooltip: 'Mean deviation of the round-trip times — how much latency varies.'
		});
	}
	if (ttl) {
		metrics.push({
			label: 'TTL',
			value: ttl[1],
			caption: 'hops',
			tooltip: 'Time-to-live in the last reply; each routing hop decrements it by one.'
		});
	}
	return metrics;
}

function mtrMetrics(lines: string[]): Metric[] {
	const rows = parseMtr(lines);
	if (!rows || rows.length === 0) return [];
	const last = rows[rows.length - 1];
	return [
		{
			label: 'Latency',
			value: last.avg,
			unit: 'ms',
			caption: 'final hop avg',
			tooltip: 'Average round-trip time to the final hop (the target).'
		},
		{
			label: 'Packet loss',
			value: last.lossPct.replace('%', ''),
			unit: '%',
			caption: 'final hop',
			tooltip: 'Packet loss measured at the final hop (the target).'
		},
		{
			label: 'Jitter',
			value: last.stdev,
			unit: 'ms',
			caption: 'final hop stdev',
			tooltip: 'Standard deviation of round-trip times at the final hop.'
		},
		{
			label: 'Hops',
			value: String(rows.length),
			caption: 'path length',
			tooltip: 'Number of routing hops the probes traversed.'
		}
	];
}

function tracerouteMetrics(lines: string[]): Metric[] {
	const hops = lines.filter((line) => TRACEROUTE_HOP.test(line)).length;
	if (hops === 0) return [];
	return [
		{
			label: 'Hops',
			value: String(hops),
			caption: 'path length',
			tooltip: 'Number of routing hops the probes traversed.'
		}
	];
}

export function metricsFor(method: string, lines: string[]): Metric[] {
	const family = method.replace(/6$/, '');
	if (family === 'ping') return pingMetrics(lines);
	if (family === 'mtr') return mtrMetrics(lines);
	if (family === 'traceroute') return tracerouteMetrics(lines);
	return [];
}
