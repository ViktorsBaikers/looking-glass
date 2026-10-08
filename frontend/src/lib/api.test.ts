import { afterEach, describe, expect, it, vi } from 'vitest';
import { del, getJson, postJson, postJsonReturning, putJson } from './api.js';

afterEach(() => {
	vi.unstubAllGlobals();
});

function respond(body: string | null, init: ResponseInit) {
	vi.stubGlobal('fetch', vi.fn(async () => new Response(body, init)));
}

const generic = { ok: false, error: 'error', message: 'The request could not be completed.' };

describe('request()', () => {
	it('resolves an unparseable 2xx body (a proxy page) to the generic failure', async () => {
		for (const call of [
			() => getJson('/api/x'),
			() => putJson('/api/x', {}),
			() => postJsonReturning('/api/x', {}),
			() => del('/api/x')
		]) {
			respond('<html>Bad gateway</html>', { status: 200, headers: { 'content-type': 'text/html' } });
			await expect(call()).resolves.toEqual(generic);
		}
	});

	it('returns valid JSON data unchanged', async () => {
		respond('{"id":7,"name":"Vienna"}', { status: 200 });
		await expect(getJson('/api/x')).resolves.toEqual({ ok: true, data: { id: 7, name: 'Vienna' } });
	});

	it('returns no data for a 204', async () => {
		respond(null, { status: 204 });
		await expect(del('/api/x')).resolves.toEqual({ ok: true, data: undefined });
	});

	it('maps a 4xx with a non-JSON body to the generic failure', async () => {
		respond('Not Found', { status: 404 });
		await expect(getJson('/api/x')).resolves.toEqual(generic);
	});

	// F-290: a JSON body that is not an object (a proxy's `null`, a number, an
	// array, a string) carries no error fields. It is the generic failure, not a
	// TypeError that rejects the caller's promise.
	for (const status of [404, 502]) {
		for (const body of ['null', '42', '["x"]', '"x"']) {
			it(`maps a ${status} whose JSON body is ${body} to the generic failure`, async () => {
				for (const call of [
					() => getJson('/api/x'),
					() => putJson('/api/x', {}),
					() => postJsonReturning('/api/x', {}),
					() => del('/api/x'),
					() => postJson('/api/x', {})
				]) {
					respond(body, { status, headers: { 'content-type': 'application/json' } });
					await expect(call()).resolves.toEqual(generic);
				}
			});
		}
	}
});
