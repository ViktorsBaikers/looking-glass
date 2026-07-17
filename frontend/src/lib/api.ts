export type ApiFailure = { ok: false; error: string; message: string };
export type ApiResult = { ok: true } | ApiFailure;

async function failureFrom(response: Response): Promise<ApiFailure> {
	const body = await response.json().catch(() => ({}) as Record<string, unknown>);
	return {
		ok: false,
		error: typeof body.error === 'string' ? body.error : 'error',
		message: typeof body.message === 'string' ? body.message : 'The request could not be completed.'
	};
}

export async function postJson(path: string, payload: unknown): Promise<ApiResult> {
	let response: Response;
	try {
		response = await fetch(path, {
			method: 'POST',
			headers: { 'content-type': 'application/json' },
			body: JSON.stringify(payload)
		});
	} catch {
		return { ok: false, error: 'network', message: 'Could not reach the server.' };
	}
	return response.ok ? { ok: true } : failureFrom(response);
}

export type JsonResult<T> = { ok: true; data: T } | ApiFailure;

async function request<T>(method: string, path: string, payload?: unknown): Promise<JsonResult<T>> {
	let response: Response;
	try {
		response = await fetch(path, {
			method,
			headers: payload === undefined ? undefined : { 'content-type': 'application/json' },
			body: payload === undefined ? undefined : JSON.stringify(payload)
		});
	} catch {
		return { ok: false, error: 'network', message: 'Could not reach the server.' };
	}
	if (!response.ok) return failureFrom(response);
	if (response.status === 204) return { ok: true, data: undefined as T };
	return { ok: true, data: (await response.json()) as T };
}

export const getJson = <T>(path: string): Promise<JsonResult<T>> => request<T>('GET', path);
export const postJsonReturning = <T>(path: string, payload: unknown): Promise<JsonResult<T>> =>
	request<T>('POST', path, payload);
export const putJson = <T>(path: string, payload: unknown): Promise<JsonResult<T>> =>
	request<T>('PUT', path, payload);
export const del = (path: string): Promise<JsonResult<undefined>> =>
	request<undefined>('DELETE', path);

export async function fetchSetupStatus(): Promise<{ installed: boolean } | null> {
	try {
		const response = await fetch('/api/setup/status');
		if (!response.ok) return null;
		return (await response.json()) as { installed: boolean };
	} catch {
		return null;
	}
}
