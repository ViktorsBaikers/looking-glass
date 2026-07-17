import { render, screen } from '@testing-library/svelte';
import { expect, it } from 'vitest';
import Tabs from './tabs.svelte';

it('renders the selected tab for DOM assertions', () => {
	render(Tabs, {
		props: {
			tabs: [
				{ id: 'network', label: 'Network' },
				{ id: 'speedtest', label: 'Speedtest' }
			],
			active: 'network'
		}
	});

	expect(screen.getByRole('tab', { name: 'Network' }).getAttribute('aria-selected')).toBe('true');
});
