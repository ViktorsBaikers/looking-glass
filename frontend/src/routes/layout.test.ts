import { cleanup, render, screen, waitFor } from '@testing-library/svelte';
import { afterEach, describe, expect, it, vi } from 'vitest';
import Layout from './+layout.svelte';

vi.mock('$app/navigation', () => ({ goto: vi.fn(), afterNavigate: () => {} }));
vi.mock('$app/state', () => ({ page: { url: new URL('http://looking-glass.test/') } }));

afterEach(() => {
	cleanup();
	vi.unstubAllGlobals();
});

// F-319 (M04): with no public settings (central unreachable), the shell still
// names itself "Looking Glass" in the header and the tab title.
describe('root layout', () => {
	it('falls back to the default site title when settings cannot load', async () => {
		const fetchMock = vi.fn(() => Promise.reject(new TypeError('Failed to fetch')));
		vi.stubGlobal('fetch', fetchMock);
		render(Layout);
		await waitFor(() => expect(fetchMock).toHaveBeenCalledWith('/api/public/settings'));
		await new Promise((resolve) => setTimeout(resolve, 10));
		expect(screen.getByRole('link', { name: 'Looking Glass' })).toBeTruthy();
		expect(document.title).toBe('Looking Glass');
	});

	it('credits the project in the footer even with no terms or custom block', async () => {
		vi.stubGlobal('fetch', vi.fn(() => Promise.reject(new TypeError('Failed to fetch'))));
		render(Layout);
		const credit = await screen.findByRole('link', { name: 'Powered by Looking Glass' });
		expect(credit.getAttribute('href')).toBe('https://github.com/ViktorsBaikers/looking-glass');
		expect(credit.getAttribute('target')).toBe('_blank');
		expect(credit.getAttribute('rel')).toBe('noopener noreferrer');
	});
});
