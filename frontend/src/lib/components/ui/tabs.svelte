<script lang="ts">
	import { Tabs as ArkTabs } from '@ark-ui/svelte';
	import type { Snippet } from 'svelte';
	import { cx } from 'styled-system/css';
	import { tabs } from 'styled-system/recipes';

	export type TabItem = {
		id: string;
		label: string;
		/** Secondary text rendered after the label as " (meta)". */
		meta?: string;
		/** `routes` only: roundel code and `lineStyle()` for the Location line. */
		code?: string;
		line?: string;
	};

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
		variant = 'stops',
		class: className,
		contentClass,
		panel
	}: {
		tabs: TabItem[];
		active: string;
		label?: string;
		/** `stops`: sequential sections on one line. `routes`: Location lines. */
		variant?: 'stops' | 'routes';
		class?: string;
		/** Class applied to each rendered tabpanel (Tabs.Content). */
		contentClass?: string;
		panel?: Snippet<[TabItem]>;
	} = $props();

	const s = $derived(tabs({ variant }));
</script>

<ArkTabs.Root bind:value={active} lazyMount unmountOnExit class={cx(s.root, className)}>
	<ArkTabs.List class={s.list} aria-label={label}>
		{#each items as tab (tab.id)}
			<ArkTabs.Trigger
				class={s.trigger}
				value={tab.id}
				data-code={tab.code}
				data-line={tab.line ? '' : undefined}
				style={tab.line}
				>{tab.label}{#if tab.meta}{' '}<span class={s.meta}>({tab.meta})</span>{/if}</ArkTabs.Trigger
			>
		{/each}
	</ArkTabs.List>
	{#if panel}
		{#each items as tab (tab.id)}
			<ArkTabs.Content class={cx(s.content, contentClass)} value={tab.id}>
				{@render panel(tab)}
			</ArkTabs.Content>
		{/each}
	{/if}
</ArkTabs.Root>
