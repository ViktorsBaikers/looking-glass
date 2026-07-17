// A minimal toast queue for admin success/error feedback. Runes-based so any
// component can push a toast and the layout's <Toaster> renders it.

export type ToastKind = 'success' | 'error';
export interface Toast {
	id: number;
	kind: ToastKind;
	message: string;
}

const DISMISS_MS = 4000;

function createToaster() {
	let toasts = $state<Toast[]>([]);
	let seq = 0;

	function push(kind: ToastKind, message: string) {
		const id = ++seq;
		toasts.push({ id, kind, message });
		setTimeout(() => dismiss(id), DISMISS_MS);
	}

	function dismiss(id: number) {
		toasts = toasts.filter((t) => t.id !== id);
	}

	return {
		get items() {
			return toasts;
		},
		success: (message: string) => push('success', message),
		error: (message: string) => push('error', message),
		dismiss
	};
}

export const toaster = createToaster();
