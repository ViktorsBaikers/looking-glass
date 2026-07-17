export type PublicTheme = 'system' | 'light' | 'dark';

export interface PublicSettings {
	site_title: string;
	logo_url: string | null;
	default_theme: PublicTheme;
	terms_url: string | null;
	custom_block: string | null;
}

function isHttpsUrl(value: unknown, maxLength: number): value is string {
	if (typeof value !== 'string' || value.length > maxLength) return false;
	try {
		const url = new URL(value);
		return url.protocol === 'https:' && url.hostname.length > 0;
	} catch {
		return false;
	}
}

function optionalHttpsUrl(value: unknown, maxLength: number): string | null | undefined {
	if (value === null) return null;
	if (typeof value !== 'string') return undefined;
	return isHttpsUrl(value, maxLength) ? value : null;
}

function parsePublicSettings(value: unknown): PublicSettings | null {
	if (!value || typeof value !== 'object' || Array.isArray(value)) return null;
	const record = value as Record<string, unknown>;
	if (
		Object.keys(record).length !== 5 ||
		typeof record.site_title !== 'string' ||
		record.site_title.length < 1 ||
		record.site_title.length > 100 ||
		(record.default_theme !== 'system' &&
			record.default_theme !== 'light' &&
			record.default_theme !== 'dark') ||
		(record.custom_block !== null &&
			(typeof record.custom_block !== 'string' || record.custom_block.length > 5000))
	) {
		return null;
	}

	const logoUrl = optionalHttpsUrl(record.logo_url, 500);
	const termsUrl = optionalHttpsUrl(record.terms_url, 300);
	if (logoUrl === undefined || termsUrl === undefined) return null;

	return {
		site_title: record.site_title,
		logo_url: logoUrl,
		default_theme: record.default_theme,
		terms_url: termsUrl,
		custom_block: record.custom_block
	};
}

export async function fetchPublicSettings(): Promise<PublicSettings | null> {
	try {
		const response = await fetch('/api/public/settings');
		if (!response.ok) return null;
		return parsePublicSettings(await response.json());
	} catch {
		return null;
	}
}
