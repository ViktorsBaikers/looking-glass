import { describe, expect, it } from 'vitest';
import { metricsFor } from './metrics.js';

const PING = [
	'$ ping -c 4 1.1.1.1',
	'PING 1.1.1.1 (1.1.1.1) 56(84) bytes of data.',
	'64 bytes from 1.1.1.1: icmp_seq=1 ttl=56 time=12.3 ms',
	'64 bytes from 1.1.1.1: icmp_seq=2 ttl=56 time=12.1 ms',
	'64 bytes from 1.1.1.1: icmp_seq=3 ttl=55 time=11.9 ms',
	'64 bytes from 1.1.1.1: icmp_seq=4 ttl=56 time=12.0 ms',
	'--- 1.1.1.1 ping statistics ---',
	'4 packets transmitted, 4 received, 0% packet loss, time 3004ms',
	'rtt min/avg/max/mdev = 11.9/12.1/12.3/0.2 ms'
];

const MTR = [
	'HOST: mtr --report 1.1.1.1   Loss%   Snt   Last   Avg  Best  Wrst StDev',
	'  1.|-- 192.0.2.1             0.0%    10    1.1   1.2   1.0   1.5   0.2',
	'  2.|-- 198.51.100.1         10.0%    10    4.3   4.5   4.1   5.0   0.3',
	'  3.|-- 1.1.1.1               0.0%    10   12.0  12.2  11.8  12.9   0.4'
];

const TRACEROUTE = [
	'traceroute to 1.1.1.1 (1.1.1.1), 30 hops max, 60 byte packets',
	' 1  192.0.2.1 (192.0.2.1)  1.045 ms  1.012 ms  0.998 ms',
	' 2  198.51.100.1 (198.51.100.1)  4.221 ms  4.190 ms  4.177 ms',
	' 3  1.1.1.1 (1.1.1.1)  12.043 ms  12.001 ms  11.987 ms'
];

describe('metricsFor ping', () => {
	it('derives latency, loss, jitter and TTL cards', () => {
		const metrics = metricsFor('ping', PING);
		expect(metrics.map((metric) => [metric.label, metric.value, metric.unit, metric.caption])).toEqual([
			['Latency', '12.1', 'ms', 'avg'],
			['Packet loss', '0', '%', '0/4'],
			['Jitter', '0.2', 'ms', 'mdev'],
			['TTL', '56', undefined, 'hops']
		]);
		expect(metrics.every((metric) => metric.tooltip.length > 0)).toBe(true);
	});

	it('computes lost/transmitted from partial loss', () => {
		const lossy = PING.map((line) =>
			line.replace('4 packets transmitted, 4 received, 0% packet loss', '4 packets transmitted, 3 received, 25% packet loss')
		);
		const loss = metricsFor('ping', lossy).find((metric) => metric.label === 'Packet loss');
		expect(loss?.value).toBe('25');
		expect(loss?.caption).toBe('1/4');
	});

	it('yields no cards when the statistics never arrived (failed run)', () => {
		expect(metricsFor('ping', PING.slice(0, 4))).toEqual([]);
	});

	it('treats ping6 like ping', () => {
		expect(metricsFor('ping6', PING).map((metric) => metric.label)).toEqual([
			'Latency',
			'Packet loss',
			'Jitter',
			'TTL'
		]);
	});
});

describe('metricsFor mtr', () => {
	it('derives final-hop latency, loss, stdev and hop count', () => {
		const metrics = metricsFor('mtr', MTR);
		expect(metrics.map((metric) => [metric.label, metric.value, metric.unit, metric.caption])).toEqual([
			['Latency', '12.2', 'ms', 'final hop avg'],
			['Packet loss', '0.0', '%', 'final hop'],
			['Jitter', '0.4', 'ms', 'final hop stdev'],
			['Hops', '3', undefined, 'path length']
		]);
	});

	it('yields no cards for unparseable output', () => {
		expect(metricsFor('mtr', ['nothing to see here'])).toEqual([]);
	});
});

describe('metricsFor traceroute', () => {
	it('counts hops', () => {
		const metrics = metricsFor('traceroute', TRACEROUTE);
		expect(metrics.map((metric) => [metric.label, metric.value, metric.caption])).toEqual([
			['Hops', '3', 'path length']
		]);
	});

	it('counts timeout hops too', () => {
		const withStars = [...TRACEROUTE, ' 4  * * *'];
		expect(metricsFor('traceroute', withStars)[0].value).toBe('4');
	});
});

describe('metricsFor bgp', () => {
	it('shows no metric cards', () => {
		expect(metricsFor('bgp', ['BGP routing table entry for 8.8.8.0/24'])).toEqual([]);
		expect(metricsFor('bgp6', ['BGP routing table entry for 2001:db8::/32'])).toEqual([]);
	});
});
