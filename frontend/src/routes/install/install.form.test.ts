import { cleanup, render, screen, waitFor } from '@testing-library/svelte';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import InstallPage from './+page.svelte';

const { goto } = vi.hoisted(() => ({ goto: vi.fn() }));

vi.mock('$app/navigation', () => ({ goto }));

function response(body: unknown, status = 200) {
	return new Response(JSON.stringify(body), {
		status,
		headers: { 'content-type': 'application/json' }
	});
}

describe('installer form', () => {
	beforeEach(() => {
		goto.mockReset();
		vi.stubGlobal(
			'fetch',
			vi.fn((input: RequestInfo | URL) => {
				const path = new URL(input.toString(), 'http://looking-glass.test').pathname;
				if (path === '/api/setup/status') return Promise.resolve(response({ installed: false }));
				throw new Error(`unexpected request ${path}`);
			})
		);
	});

	afterEach(() => {
		cleanup();
		vi.unstubAllGlobals();
	});

	it('accepts only a non-empty username made from the documented characters', async () => {
		render(InstallPage);
		const user = userEvent.setup();
		const username = screen.getByLabelText('Username');
		const submit = screen.getByRole('button', { name: 'Create account' }) as HTMLButtonElement;

		expect((username as HTMLInputElement).value).toBe('');
		expect(submit.disabled).toBe(true);
		await user.type(username, 'bad name');
		expect(screen.getByRole('alert').textContent).toBe('Use only letters, digits, and . _ -');
		expect(username.getAttribute('aria-invalid')).toBe('true');

		await user.clear(username);
		await user.type(username, 'admin.ops-1');
		expect(screen.queryByText('Use only letters, digits, and . _ -')).toBeNull();
		expect(username.hasAttribute('aria-invalid')).toBe(false);
	});

	it('rejects a short password and accepts a valid matching password', async () => {
		render(InstallPage);
		const user = userEvent.setup();
		const password = screen.getByLabelText('Password');
		const confirm = screen.getByLabelText('Confirm password');

		// F-234: a hint while typing, an error once the field is left.
		await user.type(password, 'too-short');
		expect(screen.queryByRole('alert')).toBeNull();
		expect(document.getElementById('password-hint')?.textContent).toBe('At least 12 characters.');
		expect(password.hasAttribute('aria-invalid')).toBe(false);
		await user.tab();
		expect(screen.getByRole('alert').textContent).toBe('At least 12 characters.');
		expect(password.getAttribute('aria-invalid')).toBe('true');

		await user.clear(password);
		await user.type(password, 'long-enough-password');
		await user.type(confirm, 'long-enough-password');
		expect(screen.queryByText('At least 12 characters.')).toBeNull();
		expect(screen.queryByText('Passwords do not match.')).toBeNull();
		expect(password.hasAttribute('aria-invalid')).toBe(false);
		expect(confirm.hasAttribute('aria-invalid')).toBe(false);
	});

	// F-194: central counts password length in UTF-8 bytes (12 to 512), so the
	// form does too; a non-ASCII password gets a note on how it is counted.
	it('counts the password length in UTF-8 bytes like central, with a limit on both ends', async () => {
		render(InstallPage);
		const user = userEvent.setup();
		const password = screen.getByLabelText('Password');
		const confirm = screen.getByLabelText('Confirm password');
		await user.type(screen.getByLabelText('Setup token'), 'setup-token');
		await user.type(screen.getByLabelText('Username'), 'admin');
		const submit = screen.getByRole('button', { name: 'Create account' }) as HTMLButtonElement;

		// 4 CJK characters are 12 bytes: central accepts them.
		await user.type(password, '中中中中');
		await user.type(confirm, '中中中中');
		expect(screen.queryByRole('alert')).toBeNull();
		expect(submit.disabled).toBe(false);

		// 300 x é is 300 UTF-16 units but 600 bytes: central refuses it.
		await user.clear(password);
		await user.click(password);
		await user.paste('é'.repeat(300));
		await user.clear(confirm);
		await user.click(confirm);
		await user.paste('é'.repeat(300));
		expect(document.getElementById('password-error')?.textContent).toMatch(/^At most 512 characters\. .*count as 2 to 4/);
		expect(submit.disabled).toBe(true);

		await user.clear(password);
		await user.click(password);
		await user.paste('x'.repeat(513));
		await user.clear(confirm);
		await user.click(confirm);
		await user.paste('x'.repeat(513));
		expect(document.getElementById('password-error')?.textContent).toBe('At most 512 characters.');
		expect(submit.disabled).toBe(true);

		await user.clear(password);
		await user.type(password, 'éééée');
		expect(document.getElementById('password-hint')?.textContent).toMatch(/^At least 12 characters\. .*count as 2 to 4/);
	});

	it('enables only a complete form and submits once while showing progress', async () => {
		let completeSetup: ((response: Response) => void) | undefined;
		const fetchMock = vi.fn((input: RequestInfo | URL, init?: RequestInit) => {
			const path = new URL(input.toString(), 'http://looking-glass.test').pathname;
			if (path === '/api/setup/status') return Promise.resolve(response({ installed: false }));
			if (path === '/api/setup' && init?.method === 'POST') {
				return new Promise<Response>((resolve) => {
					completeSetup = resolve;
				});
			}
			throw new Error(`unexpected request ${init?.method ?? 'GET'} ${path}`);
		});
		vi.stubGlobal('fetch', fetchMock);
		render(InstallPage);
		const user = userEvent.setup();
		const submit = screen.getByRole('button', { name: 'Create account' }) as HTMLButtonElement;

		expect(submit.disabled).toBe(true);
		await user.type(screen.getByLabelText('Setup token'), 'setup-token');
		await user.type(screen.getByLabelText('Username'), 'admin');
		await user.type(screen.getByLabelText('Password'), 'long-enough-password');
		await user.type(screen.getByLabelText('Confirm password'), 'long-enough-password');
		expect(submit.disabled).toBe(false);

		await user.click(submit);
		expect(fetchMock.mock.calls.filter(([input]) => input === '/api/setup')).toHaveLength(1);
		const busy = screen.getByRole('button', { name: 'Creating account' }) as HTMLButtonElement;
		// Busy, not native disabled: a real browser drops focus from a control that becomes disabled.
		expect(busy.disabled).toBe(false);
		expect(busy.getAttribute('aria-busy')).toBe('true');
		expect(busy.getAttribute('aria-disabled')).toBe('true');
		expect(busy.querySelector('svg')).not.toBeNull();
		await user.click(busy);
		busy.focus();
		await user.keyboard('{Enter}');
		await user.keyboard(' ');
		// Busy fields are read-only, not disabled: they keep focus and ignore typing and Enter.
		const username = screen.getByLabelText('Username') as HTMLInputElement;
		await user.type(username, 'x{Enter}');
		expect(username.value).toBe('admin');
		expect(document.activeElement).toBe(username);
		expect(fetchMock.mock.calls.filter(([input]) => input === '/api/setup')).toHaveLength(1);

		completeSetup?.(new Response(null, { status: 204 }));
		await waitFor(() => expect(goto).toHaveBeenCalledWith('/login'));
	});
});
