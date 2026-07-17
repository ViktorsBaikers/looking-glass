import { cleanup, render, screen, waitFor } from '@testing-library/svelte';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import LoginPage from './+page.svelte';

const { goto } = vi.hoisted(() => ({ goto: vi.fn() }));

vi.mock('$app/navigation', () => ({ goto }));

function response(body: unknown, status = 200) {
	return new Response(JSON.stringify(body), {
		status,
		headers: { 'content-type': 'application/json' }
	});
}

describe('login form', () => {
	beforeEach(() => {
		goto.mockReset();
	});

	afterEach(() => {
		cleanup();
		vi.unstubAllGlobals();
	});

	it('shows one generic error for wrong credentials without identifying either field', async () => {
		vi.stubGlobal(
			'fetch',
			vi.fn((input: RequestInfo | URL, init?: RequestInit) => {
				const path = new URL(input.toString(), 'http://looking-glass.test').pathname;
				if (path === '/api/setup/status') return Promise.resolve(response({ installed: true }));
				if (path === '/api/auth/login' && init?.method === 'POST') {
					return Promise.resolve(
						response({ error: 'invalid_credentials', message: 'Invalid username or password.' }, 401)
					);
				}
				throw new Error(`unexpected request ${init?.method ?? 'GET'} ${path}`);
			})
		);
		render(LoginPage);
		const user = userEvent.setup();
		await user.type(screen.getByLabelText('Username'), 'admin');
		await user.type(screen.getByLabelText('Password'), 'wrong-password');
		await user.click(screen.getByRole('button', { name: 'Sign in' }));

		const alerts = await screen.findAllByRole('alert');
		expect(alerts).toHaveLength(1);
		expect(alerts[0].textContent).toBe('Invalid username or password.');
		expect(alerts[0].textContent).not.toMatch(/username (?:is|was)|password (?:is|was)/i);
		expect(screen.getByLabelText('Username').hasAttribute('aria-invalid')).toBe(false);
		expect(screen.getByLabelText('Password').hasAttribute('aria-invalid')).toBe(false);
	});

	it('enables only filled credentials and submits once while showing progress', async () => {
		let completeLogin: ((response: Response) => void) | undefined;
		const fetchMock = vi.fn((input: RequestInfo | URL, init?: RequestInit) => {
			const path = new URL(input.toString(), 'http://looking-glass.test').pathname;
			if (path === '/api/setup/status') return Promise.resolve(response({ installed: true }));
			if (path === '/api/auth/login' && init?.method === 'POST') {
				return new Promise<Response>((resolve) => {
					completeLogin = resolve;
				});
			}
			throw new Error(`unexpected request ${init?.method ?? 'GET'} ${path}`);
		});
		vi.stubGlobal('fetch', fetchMock);
		render(LoginPage);
		const user = userEvent.setup();
		const submit = screen.getByRole('button', { name: 'Sign in' }) as HTMLButtonElement;

		expect(submit.disabled).toBe(true);
		await user.type(screen.getByLabelText('Username'), 'admin');
		expect(submit.disabled).toBe(true);
		await user.type(screen.getByLabelText('Password'), 'password');
		expect(submit.disabled).toBe(false);

		await user.click(submit);
		expect(fetchMock.mock.calls.filter(([input]) => input === '/api/auth/login')).toHaveLength(1);
		expect((screen.getByRole('button', { name: 'Signing in' }) as HTMLButtonElement).disabled).toBe(true);
		expect(screen.getByRole('button', { name: 'Signing in' }).querySelector('svg')).not.toBeNull();

		completeLogin?.(new Response(null, { status: 204 }));
		await waitFor(() => expect(goto).toHaveBeenCalledWith('/'));
	});
});
