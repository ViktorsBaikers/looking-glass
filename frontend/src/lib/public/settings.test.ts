import { afterEach, describe, expect, it, vi } from 'vitest';
import { fetchPublicSettings } from './settings.js';

afterEach(() => vi.unstubAllGlobals());

const settings = {
	site_title: 'Acme LG',
	logo_url: null,
	default_theme: 'dark',
	terms_url: null,
	custom_block: null
};
const reply = (status: number) =>
	vi.fn(async () => new Response(JSON.stringify(settings), { status, headers: { 'content-type': 'application/json' } }));

// F-319 (M02): only a successful answer brands the shell; an error status is
// ignored even when its body looks like valid settings.
describe('fetchPublicSettings', () => {
	it('applies a well-formed 200 answer', async () => {
		vi.stubGlobal('fetch', reply(200));
		expect(await fetchPublicSettings()).toEqual(settings);
	});

	it('ignores an error-status answer whose body parses as settings', async () => {
		vi.stubGlobal('fetch', reply(500));
		expect(await fetchPublicSettings()).toBeNull();
	});
});
