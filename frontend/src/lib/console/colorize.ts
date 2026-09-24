// Minimal syntax colouring for streamed diagnostic output: highlight the byte
// counts and RTT values the design colours ("64 bytes", "time=12.3 ms").
// Pure so the console template stays a flat render.

export type Tone = 'plain' | 'bytes' | 'time';

export interface Segment {
	text: string;
	tone: Tone;
}

const TOKEN = /(\d+ bytes|time=[\d.]+ ms)/g;

export function colorize(text: string): Segment[] {
	const segments: Segment[] = [];
	let last = 0;
	for (const match of text.matchAll(TOKEN)) {
		const index = match.index ?? 0;
		if (index > last) segments.push({ text: text.slice(last, index), tone: 'plain' });
		segments.push({ text: match[0], tone: match[0].endsWith('bytes') ? 'bytes' : 'time' });
		last = index + match[0].length;
	}
	if (last < text.length) segments.push({ text: text.slice(last), tone: 'plain' });
	return segments;
}
