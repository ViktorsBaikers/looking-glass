import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/svelte';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import SettingsPage from '../../routes/admin/settings/+page.svelte';
import PublicLayout from '../../routes/+layout.svelte';
import { fetchPublicSettings } from '../public/settings.js';
import { theme } from '../theme.svelte.js';

const settings = {
	site_title: 'Looking Glass',
	logo_url: null,
	default_theme: 'system',
	terms_url: null,
	custom_block: null,
	exec_max_concurrent: 8,
	exec_timeout_secs: 30,
	exec_max_output_kib: 256,
	exec_rate_max: 20,
	exec_rate_window_secs: 60
} as const;

function jsonResponse(body: unknown, status = 200) {
	return new Response(JSON.stringify(body), {
		status,
		headers: { 'content-type': 'application/json' }
	});
}

describe('admin settings public branding', () => {
	let saved = { ...settings };

	beforeEach(() => {
		saved = { ...settings };
		localStorage.clear();
		document.title = 'Looking Glass';
		vi.stubGlobal(
			'matchMedia',
			vi.fn().mockReturnValue({ matches: true, addEventListener: vi.fn(), removeEventListener: vi.fn() })
		);
		vi.stubGlobal(
			'fetch',
			vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
				const path = input.toString();
				if (path === '/api/setup/status') return jsonResponse({ installed: true });
				if (path === '/api/admin/settings' && init?.method === 'PUT') {
					saved = JSON.parse(String(init.body));
					return jsonResponse(saved);
				}
				if (path === '/api/admin/settings') return jsonResponse(saved);
				if (path === '/api/public/settings') {
					return jsonResponse({
						site_title: saved.site_title,
						logo_url: saved.logo_url,
						default_theme: saved.default_theme,
						terms_url: saved.terms_url,
						custom_block: saved.custom_block
					});
				}
				throw new Error(`unexpected fetch ${path}`);
			})
		);
	});

	afterEach(() => {
		cleanup();
		vi.unstubAllGlobals();
	});

	it('saves branding and applies it after a public reload', async () => {
		const user = userEvent.setup();
		const page = render(SettingsPage);
		await screen.findByDisplayValue('Looking Glass');
		await user.clear(screen.getByLabelText('Site title'));
		await user.type(screen.getByLabelText('Site title'), 'Frankfurt Glass');
		await user.type(screen.getByLabelText('Logo URL (optional)'), 'https://cdn.example.test/logo.svg');
		await user.selectOptions(screen.getByLabelText('Default theme'), 'dark');
		await user.type(screen.getByLabelText('Terms-of-service URL (optional)'), 'https://example.test/terms');
		await user.type(screen.getByLabelText('Custom content block (optional)'), 'Operated by Example');
		await user.click(screen.getByRole('button', { name: 'Save settings' }));

		await waitFor(() => expect(saved.site_title).toBe('Frankfurt Glass'));
		page.unmount();
		render(PublicLayout);

		await screen.findByRole('link', { name: 'Frankfurt Glass' });
		expect(document.title).toBe('Frankfurt Glass');
		const brandingLink = screen.getByRole('link', { name: 'Frankfurt Glass' });
		const logo = brandingLink.querySelector('img');
		expect(logo?.getAttribute('src')).toBe('https://cdn.example.test/logo.svg');
		expect(logo?.getAttribute('alt')).toBe('');
		expect(screen.getByRole('link', { name: 'Terms' }).getAttribute('href')).toBe(
			'https://example.test/terms'
		);
		expect(screen.getByText('Operated by Example')).not.toBeNull();
		expect(document.documentElement.classList.contains('dark')).toBe(true);
	});

	it('keeps the fallback on failed or malformed settings and omits unsafe fixture URLs', async () => {
		const fetchMock = vi.mocked(fetch);
		fetchMock.mockImplementation(async (input: RequestInfo | URL) => {
			if (input.toString() === '/api/setup/status') return jsonResponse({ installed: true });
			return jsonResponse({
				site_title: 'Safe Glass',
				logo_url: 'javascript:alert(1)',
				default_theme: 'light',
				terms_url: 'http://example.test/terms',
				custom_block: '<b>Text only</b>'
			});
		});
		render(PublicLayout);
		await screen.findByRole('link', { name: 'Safe Glass' });
		expect(screen.queryByRole('img')).toBeNull();
		expect(screen.queryByRole('link', { name: 'Terms' })).toBeNull();
		expect(screen.getByText('<b>Text only</b>')).not.toBeNull();

		fetchMock.mockResolvedValueOnce(jsonResponse({}, 500));
		expect(await fetchPublicSettings()).toBeNull();
		fetchMock.mockResolvedValueOnce(new Response('{', { headers: { 'content-type': 'application/json' } }));
		expect(await fetchPublicSettings()).toBeNull();
		fetchMock.mockResolvedValueOnce(jsonResponse({ ...settings, site_title: '' }));
		expect(await fetchPublicSettings()).toBeNull();
		fetchMock.mockRejectedValueOnce(new TypeError('offline'));
		expect(await fetchPublicSettings()).toBeNull();
	});

	it('honors a valid stored theme over the configured default without rewriting storage', () => {
		localStorage.setItem('theme', 'light');
		theme.applyDefault('dark');
		expect(document.documentElement.classList.contains('dark')).toBe(false);
		expect(localStorage.getItem('theme')).toBe('light');

		localStorage.setItem('theme', 'invalid');
		theme.applyDefault('system');
		expect(document.documentElement.classList.contains('dark')).toBe(true);
		expect(localStorage.getItem('theme')).toBe('invalid');
	});
});
