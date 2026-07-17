import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/svelte';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import EnrollDialog from './EnrollDialog.svelte';

const NOW_MS = 1_800_000_000_000;
const INSTALL_COMMAND = 'curl -fsSL https://lg.example/install.sh | sh -s -- --token single-use';

function response(body: unknown, status = 200) {
	return new Response(JSON.stringify(body), {
		status,
		headers: { 'content-type': 'application/json' }
	});
}

describe('agent enrollment dialog', () => {
	let pollLocation: (() => void) | undefined;
	let writeText: ReturnType<typeof vi.fn>;

	beforeEach(() => {
		if (!HTMLDialogElement.prototype.showModal) {
			HTMLDialogElement.prototype.showModal = function () {
				this.open = true;
			};
			HTMLDialogElement.prototype.close = function () {
				this.open = false;
				this.dispatchEvent(new Event('close'));
			};
		}
		vi.spyOn(Date, 'now').mockReturnValue(NOW_MS);
		vi.spyOn(globalThis, 'setInterval').mockImplementation((callback, delay) => {
			if (delay === 3000 && typeof callback === 'function') pollLocation = () => void callback();
			return 1 as unknown as ReturnType<typeof setInterval>;
		});
		writeText = vi.fn().mockResolvedValue(undefined);
		Object.defineProperty(navigator, 'clipboard', {
			configurable: true,
			value: { writeText }
		});
		vi.stubGlobal(
			'fetch',
			vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
				const path = new URL(input.toString(), 'http://looking-glass.test').pathname;
				if (init?.method === 'POST' && path === '/api/admin/locations/fra/enroll') {
					return response({
						install_command: INSTALL_COMMAND,
						token: 'single-use',
						fingerprint: 'sha256:fingerprint',
						expires_at: NOW_MS / 1000 + 120
					}, 201);
				}
				if (path === '/api/admin/locations/fra') {
					return response({ id: 'fra', status: 'online' });
				}
				throw new Error(`unexpected fetch ${init?.method ?? 'GET'} ${path}`);
			})
		);
	});

	afterEach(() => {
		cleanup();
		vi.restoreAllMocks();
		vi.unstubAllGlobals();
	});

	it('shows an uneditable install command, copies it, and reports its expiry', async () => {
		render(EnrollDialog, {
			open: true,
			locationId: 'fra',
			locationName: 'Frankfurt',
			onclose: vi.fn()
		});

		const command = await screen.findByText(INSTALL_COMMAND, { selector: 'code' });
		expect(command.closest('pre')).not.toBeNull();
		expect(screen.queryByDisplayValue(INSTALL_COMMAND)).toBeNull();
		expect(screen.getByText('02:00')).toBeTruthy();

		await fireEvent.click(screen.getByRole('button', { name: 'Copy install command' }));
		await waitFor(() => expect(writeText).toHaveBeenCalledWith(INSTALL_COMMAND));
	});

	it('changes from waiting for the agent to Connected after liveness reports online', async () => {
		render(EnrollDialog, {
			open: true,
			locationId: 'fra',
			locationName: 'Frankfurt',
			onclose: vi.fn()
		});

		expect(await screen.findByText(/Waiting for the agent to connect/)).toBeTruthy();
		expect(pollLocation).toBeTypeOf('function');
		pollLocation?.();

		expect(await screen.findByText('Connected — Frankfurt is online.')).toBeTruthy();
	});
});
