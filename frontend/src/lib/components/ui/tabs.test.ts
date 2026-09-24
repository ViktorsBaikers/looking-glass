import { cleanup, render, screen } from '@testing-library/svelte';
import { afterEach, expect, it } from 'vitest';
import Tabs from './tabs.svelte';

const TABS = [
	{ id: 'settings', label: 'Settings' },
	{ id: 'methods', label: 'Methods' },
	{ id: 'testips', label: 'Test IPs' }
];

afterEach(cleanup);

// Role/state only: a plausible bug (wrong initial selection, or tabs not exposed
// as ARIA tabs) fails these. Arrow-key navigation is an Ark/zag machine behaviour
// that does not fire under jsdom; per the spec it is covered by Playwright (Seam 2).
it('exposes each entry as an ARIA tab and marks only the active one selected', () => {
	render(Tabs, { props: { tabs: TABS, active: 'methods' } });

	expect(screen.getAllByRole('tab')).toHaveLength(3);
	expect(screen.getByRole('tab', { name: 'Methods' }).getAttribute('aria-selected')).toBe('true');
	expect(screen.getByRole('tab', { name: 'Settings' }).getAttribute('aria-selected')).toBe('false');
	expect(screen.getByRole('tab', { name: 'Test IPs' }).getAttribute('aria-selected')).toBe('false');
});

it('labels the tablist for assistive tech', () => {
	render(Tabs, { props: { tabs: TABS, active: 'settings', label: 'Location editor sections' } });

	expect(screen.getByRole('tablist', { name: 'Location editor sections' })).toBeTruthy();
});
