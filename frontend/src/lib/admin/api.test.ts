import { afterEach, describe, expect, it, vi } from 'vitest';
import * as api from './api.js';

afterEach(() => {
	vi.unstubAllGlobals();
});

function stubFetch() {
	const fetchMock = vi.fn(
		async (_input: RequestInfo | URL, _init?: RequestInit) =>
			new Response('{}', { headers: { 'content-type': 'application/json' } })
	);
	vi.stubGlobal('fetch', fetchMock);
	return () => fetchMock.mock.calls.map(([input]) => String(input));
}

// Ids and tokens often come from decoded route params; a '/', '?' or
// '#' in one must not steer the request to another endpoint.
describe('admin API path parameters', () => {
	const crafted = '../x?y#z';
	const encoded = '..%2Fx%3Fy%23z';
	const body = {} as never;

	it.each([
		['getLocation', () => api.getLocation(crafted), `/api/admin/locations/${encoded}`],
		['updateLocation', () => api.updateLocation(crafted, body), `/api/admin/locations/${encoded}`],
		['deleteLocation', () => api.deleteLocation(crafted), `/api/admin/locations/${encoded}`],
		['revokeAgent', () => api.revokeAgent(crafted), `/api/admin/locations/${encoded}/agent/revoke`],
		['createEnrollment', () => api.createEnrollment(crafted), `/api/admin/locations/${encoded}/enroll`],
		['createTestIp', () => api.createTestIp(crafted, body), `/api/admin/locations/${encoded}/test-ips`],
		['updateTestIp', () => api.updateTestIp(crafted, body), `/api/admin/test-ips/${encoded}`],
		['deleteTestIp', () => api.deleteTestIp(crafted), `/api/admin/test-ips/${encoded}`],
		['createIperf', () => api.createIperf(crafted, body), `/api/admin/locations/${encoded}/iperf`],
		['updateIperf', () => api.updateIperf(crafted, body), `/api/admin/iperf/${encoded}`],
		['deleteIperf', () => api.deleteIperf(crafted), `/api/admin/iperf/${encoded}`],
		['createTestFile', () => api.createTestFile(crafted, body), `/api/admin/locations/${encoded}/files`],
		['updateTestFile', () => api.updateTestFile(crafted, body), `/api/admin/files/${encoded}`],
		['deleteTestFile', () => api.deleteTestFile(crafted), `/api/admin/files/${encoded}`],
		[
			'regenerateActivation',
			() => api.regenerateActivation(crafted),
			`/api/admin/administrators/${encoded}/activation`
		],
		['removeAdministrator', () => api.removeAdministrator(crafted), `/api/admin/administrators/${encoded}`],
		['getActivation', () => api.getActivation(crafted), `/api/activate/${encoded}`],
		['activate', () => api.activate(crafted, 'fixture-password'), `/api/activate/${encoded}`]
	])('%s encodes its path parameter', async (_name, call, expected) => {
		const urls = stubFetch();
		await call();
		expect(urls()).toEqual([expected]);
	});

	it('leaves a plain id unchanged', async () => {
		const urls = stubFetch();
		await api.getLocation('fra-1_x');
		expect(urls()).toEqual(['/api/admin/locations/fra-1_x']);
	});
});
