// Pure helpers for the admin Settings form: payload coercion and unsaved-changes
// detection. Kept out of the page so vitest can exercise them without a DOM.
import type { GlobalSettings, Theme } from './types.js';

/** The form's working copy; the theme select binds a plain string. */
export interface SettingsDraft {
	site_title: string;
	logo_url: string | null;
	default_theme: string;
	terms_url: string | null;
	custom_block: string | null;
	exec_max_concurrent: number;
	exec_timeout_secs: number;
	exec_max_output_kib: number;
	exec_rate_max: number;
	exec_rate_window_secs: number;
}

export function nullIfBlank(value: string | null): string | null {
	return value && value.trim() !== '' ? value : null;
}

/** Coerce the draft into the server DTO: blanks become null, texts become ints. */
export function toPayload(draft: SettingsDraft): GlobalSettings {
	return {
		site_title: draft.site_title,
		logo_url: nullIfBlank(draft.logo_url),
		default_theme: draft.default_theme as Theme,
		terms_url: nullIfBlank(draft.terms_url),
		custom_block: nullIfBlank(draft.custom_block),
		exec_max_concurrent: Number(draft.exec_max_concurrent),
		exec_timeout_secs: Number(draft.exec_timeout_secs),
		exec_max_output_kib: Number(draft.exec_max_output_kib),
		exec_rate_max: Number(draft.exec_rate_max),
		exec_rate_window_secs: Number(draft.exec_rate_window_secs)
	};
}

/** True when the draft differs from what is saved, after payload coercion. */
export function isDirty(saved: GlobalSettings, draft: SettingsDraft): boolean {
	return JSON.stringify(toPayload(saved)) !== JSON.stringify(toPayload(draft));
}
