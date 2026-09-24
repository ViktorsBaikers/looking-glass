<script lang="ts">
	import { Tabs as ArkTabs } from '@ark-ui/svelte';
	import type { Snippet } from 'svelte';
	import { cx } from 'styled-system/css';
	import { tabs } from 'styled-system/recipes';

	/**
	 * Controlled tab list (Ark Tabs machine → arrow-key navigation + ARIA for
	 * free). When a `panel` snippet is passed it renders inside Ark
	 * `Tabs.Content` for the active tab, so triggers and panels stay linked
	 * (aria-controls ↔ tabpanel id/aria-labelledby); `active` is bindable so a
	 * page can drive it from a URL query parameter.
	 */
	let {
		tabs: items,
		active = $bindable(),
		label = 'Sections',
		class: className,
		contentClass,
		panel
	}: {
		tabs: { id: string; label: string }[];
		active: string;
		label?: string;
		class?: string;
		/** Class applied to each rendered tabpanel (Tabs.Content). */
		contentClass?: string;
		panel?: Snippet<[{ id: string; label: string }]>;
	} = $props();

	const s = tabs();
</script>

<ArkTabs.Root bind:value={active} lazyMount unmountOnExit class={cx(s.root, className)}>
	<ArkTabs.List class={s.list} aria-label={label}>
		{#each items as tab (tab.id)}
			<ArkTabs.Trigger class={s.trigger} value={tab.id}>{tab.label}</ArkTabs.Trigger>
		{/each}
	</ArkTabs.List>
	{#if panel}
		{#each items as tab (tab.id)}
			<ArkTabs.Content class={contentClass} value={tab.id}>
				{@render panel(tab)}
			</ArkTabs.Content>
		{/each}
	{/if}
</ArkTabs.Root>
