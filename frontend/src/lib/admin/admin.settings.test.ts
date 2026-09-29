import { cleanup, render, screen, waitFor } from '@testing-library/svelte';
import userEvent from '@testing-library/user-event';
import { tick } from 'svelte';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import PublicLayout from '../../routes/+layout.svelte';
import SettingsPage from '../../routes/admin/settings/+page.svelte';
import { fetchPublicSettings } from '../public/settings.js';
import { isDirty, toPayload, type SettingsDraft } from './settings.js';
import { saveSettings, savedSettings } from './api.js';
import { theme } from '../theme.svelte.js';
import type { GlobalSettings } from './types.js';

const saved: GlobalSettings = {
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
};

const draft = (over: Partial<SettingsDraft> = {}): SettingsDraft => ({ ...saved, ...over });

function jsonResponse(body: unknown, status = 200) {
	return new Response(JSON.stringify(body), {
		status,
		headers: { 'content-type': 'application/json' }
	});
}

describe('settings payload coercion', () => {
	it('turns blank optional branding into null and keeps integers', () => {
		expect(
			toPayload(
				draft({ logo_url: '   ', terms_url: '', custom_block: '  ', exec_rate_max: 20 })
			)
		).toEqual(saved);
	});

	it('keeps real values untouched', () => {
		const payload = toPayload(
			draft({
				site_title: 'Frankfurt Glass',
				logo_url: 'https://cdn.example.test/logo.svg',
				terms_url: 'https://example.test/terms',
				custom_block: 'Operated by Example',
				default_theme: 'dark',
				exec_timeout_secs: 45
			})
		);
		expect(payload).toEqual({
			...saved,
			site_title: 'Frankfurt Glass',
			logo_url: 'https://cdn.example.test/logo.svg',
			terms_url: 'https://example.test/terms',
			custom_block: 'Operated by Example',
			default_theme: 'dark',
			exec_timeout_secs: 45
		});
	});
});

describe('unsaved-changes detection', () => {
	it('is clean for an untouched draft and after reverting an edit', () => {
		expect(isDirty(saved, draft())).toBe(false);
		const edited = draft({ site_title: 'Changed' });
		expect(isDirty(saved, edited)).toBe(true);
		edited.site_title = saved.site_title;
		expect(isDirty(saved, edited)).toBe(false);
	});

	it('flags every section: branding, theme and limits', () => {
		expect(isDirty(saved, draft({ custom_block: 'notice' }))).toBe(true);
		expect(isDirty(saved, draft({ default_theme: 'light' }))).toBe(true);
		expect(isDirty(saved, draft({ exec_rate_window_secs: 90 }))).toBe(true);
	});

	it('treats a cleared optional field as unchanged, not as an edit', () => {
		const withTerms = { ...saved, terms_url: 'https://example.test/terms' };
		expect(isDirty(withTerms, draft({ terms_url: '' }))).toBe(true);
		expect(isDirty(saved, draft({ terms_url: '   ' }))).toBe(false);
	});
});

describe('public branding application', () => {
	beforeEach(() => {
		localStorage.clear();
		document.title = 'Looking Glass';
		vi.stubGlobal(
			'matchMedia',
			vi.fn().mockReturnValue({ matches: true, addEventListener: vi.fn(), removeEventListener: vi.fn() })
		);
	});

	afterEach(() => {
		cleanup();
		vi.unstubAllGlobals();
		savedSettings.set(null);
	});

	it('applies saved branding to the public shell and honours a stored theme', async () => {
		vi.stubGlobal(
			'fetch',
			vi.fn(async (input: RequestInfo | URL) => {
				const path = input.toString();
				if (path === '/api/setup/status') return jsonResponse({ installed: true });
				if (path === '/api/admin/me') return jsonResponse({ error: 'unauthorized' }, 401);
				if (path === '/api/public/settings') {
					return jsonResponse({
						site_title: 'Frankfurt Glass',
						logo_url: 'https://cdn.example.test/logo.svg',
						default_theme: 'dark',
						terms_url: 'https://example.test/terms',
						custom_block: 'Operated by Example'
					});
				}
				throw new Error(`unexpected fetch ${path}`);
			})
		);
		render(PublicLayout);

		await screen.findByRole('link', { name: 'Frankfurt Glass' });
		expect(document.title).toBe('Frankfurt Glass');
		const logo = screen.getByRole('link', { name: 'Frankfurt Glass' }).querySelector('img');
		expect(logo?.getAttribute('src')).toBe('https://cdn.example.test/logo.svg');
		expect(logo?.getAttribute('alt')).toBe('');
		expect(screen.getByRole('link', { name: 'Terms' }).getAttribute('href')).toBe(
			'https://example.test/terms'
		);
		expect(screen.getByText('Operated by Example')).not.toBeNull();
		expect(document.documentElement.classList.contains('dark')).toBe(true);
	});

	it('keeps the fallback on failed or malformed settings and omits unsafe fixture URLs', async () => {
		const fetchMock = vi.fn(async (input: RequestInfo | URL) => {
			const path = input.toString();
			if (path === '/api/setup/status') return jsonResponse({ installed: true });
			if (path === '/api/admin/me') return jsonResponse({ error: 'unauthorized' }, 401);
			return jsonResponse({
				site_title: 'Safe Glass',
				logo_url: 'javascript:alert(1)',
				default_theme: 'light',
				terms_url: 'http://example.test/terms',
				custom_block: '<b>Text only</b>'
			});
		});
		vi.stubGlobal('fetch', fetchMock);
		render(PublicLayout);
		const brand = await screen.findByRole('link', { name: 'Safe Glass' });
		// The logo is decorative (alt=""), so it has no img role: look for the element.
		expect(brand.querySelector('img')).toBeNull();
		expect(screen.queryByRole('link', { name: 'Terms' })).toBeNull();
		expect(screen.getByText('<b>Text only</b>')).not.toBeNull();

		fetchMock.mockResolvedValueOnce(jsonResponse({}, 500));
		expect(await fetchPublicSettings()).toBeNull();
		fetchMock.mockResolvedValueOnce(
			new Response('{', { headers: { 'content-type': 'application/json' } })
		);
		expect(await fetchPublicSettings()).toBeNull();
		// The public shape (five keys), so only the empty title can reject it.
		const publicSettings = {
			site_title: 'Safe Glass',
			logo_url: null,
			default_theme: 'light',
			terms_url: null,
			custom_block: null
		};
		fetchMock.mockResolvedValueOnce(jsonResponse(publicSettings));
		expect(await fetchPublicSettings()).toEqual(publicSettings);
		fetchMock.mockResolvedValueOnce(jsonResponse({ ...publicSettings, site_title: '' }));
		expect(await fetchPublicSettings()).toBeNull();
		fetchMock.mockRejectedValueOnce(new TypeError('offline'));
		expect(await fetchPublicSettings()).toBeNull();
	});

	// An admin's save must update the running shell (header, logo,
	// document title, default theme) without a reload; a failed save must not.
	function stubShell(putStatus: number) {
		vi.stubGlobal(
			'fetch',
			vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
				const path = input.toString();
				if (path === '/api/setup/status') return jsonResponse({ installed: true });
				if (path === '/api/admin/me') return jsonResponse({ error: 'unauthorized' }, 401);
				if (path === '/api/public/settings') {
					return jsonResponse({
						site_title: 'Old Glass',
						logo_url: null,
						default_theme: 'light',
						terms_url: null,
						custom_block: null
					});
				}
				if (path === '/api/admin/settings' && init?.method === 'PUT') {
					return putStatus === 200
						? jsonResponse(JSON.parse(String(init.body)))
						: jsonResponse({ error: 'invalid', message: 'Rejected' }, putStatus);
				}
				throw new Error(`unexpected fetch ${path}`);
			})
		);
	}

	const newBranding: GlobalSettings = {
		...saved,
		site_title: 'New Glass',
		logo_url: 'https://cdn.example.test/new.svg',
		default_theme: 'dark'
	};

	it('applies a successful settings save to the running shell', async () => {
		stubShell(200);
		render(PublicLayout);
		await screen.findByRole('link', { name: 'Old Glass' });
		expect(document.documentElement.classList.contains('dark')).toBe(false);

		expect((await saveSettings(newBranding)).ok).toBe(true);
		await tick();

		const brand = screen.getByRole('link', { name: 'New Glass' });
		expect(brand.querySelector('img')?.getAttribute('src')).toBe('https://cdn.example.test/new.svg');
		expect(document.title).toBe('New Glass');
		expect(document.documentElement.classList.contains('dark')).toBe(true);
	});

	it('leaves the running shell unchanged when a settings save fails', async () => {
		stubShell(400);
		render(PublicLayout);
		await screen.findByRole('link', { name: 'Old Glass' });

		expect((await saveSettings(newBranding)).ok).toBe(false);
		await tick();

		await waitFor(() => expect(document.title).toBe('Old Glass'));
		expect(screen.getByRole('link', { name: 'Old Glass' }).querySelector('img')).toBeNull();
		expect(screen.queryByRole('link', { name: 'New Glass' })).toBeNull();
		expect(document.documentElement.classList.contains('dark')).toBe(false);
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

// F-192: the saved copy replaces the whole form, so nothing may be typed while
// a save is in flight. The fields refuse input without `disabled`, which would
// drop focus to <body> (R-TS-05).
describe('settings page while saving', () => {
	afterEach(() => {
		cleanup();
		vi.unstubAllGlobals();
		savedSettings.set(null);
	});

	it('keeps focus on Save and refuses typing until the saved copy lands', async () => {
		let release: (response: Response) => void = () => {};
		vi.stubGlobal(
			'fetch',
			vi.fn((_input: RequestInfo | URL, init?: RequestInit) =>
				init?.method === 'PUT'
					? new Promise<Response>((resolve) => (release = resolve))
					: Promise.resolve(jsonResponse(saved))
			)
		);
		render(SettingsPage);
		const user = userEvent.setup();
		const title = (await screen.findByLabelText('Site title')) as HTMLInputElement;
		const block = screen.getByLabelText('Custom content block (optional)') as HTMLTextAreaElement;
		const cap = screen.getByLabelText('Global concurrency cap') as HTMLInputElement;
		await user.clear(title);
		await user.type(title, 'New');
		const save = screen.getByRole('button', { name: 'Save settings' });
		await user.click(save);
		expect(document.activeElement).toBe(save);
		expect(save.getAttribute('aria-busy')).toBe('true');

		await user.type(block, 'typed while saving');
		expect(block.value).toBe('');
		for (const field of [title, block, cap]) {
			expect(field.readOnly).toBe(true);
			expect(field.getAttribute('aria-disabled')).toBe('true');
		}
		expect(screen.getByText('Unsaved changes.', { exact: true })).toBeTruthy();

		release(jsonResponse({ ...saved, site_title: 'New' }));
		await waitFor(() => expect(screen.getByText('No unsaved changes.', { exact: true })).toBeTruthy());
		expect(title.value).toBe('New');
		for (const field of [title, block, cap]) {
			expect(field.readOnly).toBe(false);
			expect(field.hasAttribute('aria-disabled')).toBe(false);
		}
		await user.type(block, 'typed after');
		expect(block.value).toBe('typed after');
	});
});
