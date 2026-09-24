<script lang="ts">
	import { Tabs as ArkTabs } from '@ark-ui/svelte';
	import { cx } from 'styled-system/css';
	import { tabs } from 'styled-system/recipes';

	/**
	 * Controlled tab list (Ark Tabs machine → arrow-key navigation + ARIA for
	 * free). Panels are rendered by the page keyed off `active`; this component
	 * owns only the tablist. `active` is bindable so a page can drive it from a
	 * URL query parameter.
	 */
	let {
		tabs: items,
		active = $bindable(),
		label = 'Sections',
		class: className
	}: {
		tabs: { id: string; label: string }[];
		active: string;
		label?: string;
		class?: string;
	} = $props();

	const s = tabs();
</script>

<ArkTabs.Root bind:value={active} class={cx(s.root, className)}>
	<ArkTabs.List class={s.list} aria-label={label}>
		{#each items as tab (tab.id)}
			<ArkTabs.Trigger class={s.trigger} value={tab.id}>{tab.label}</ArkTabs.Trigger>
		{/each}
	</ArkTabs.List>
</ArkTabs.Root>
