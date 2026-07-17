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

		await user.type(password, 'too-short');
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
		expect((screen.getByRole('button', { name: 'Creating account' }) as HTMLButtonElement).disabled).toBe(true);
		expect(screen.getByRole('button', { name: 'Creating account' }).querySelector('svg')).not.toBeNull();

		completeSetup?.(new Response(null, { status: 204 }));
		await waitFor(() => expect(goto).toHaveBeenCalledWith('/login'));
	});
});
