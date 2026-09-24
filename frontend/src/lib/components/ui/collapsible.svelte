<script lang="ts">
	import type { Snippet } from 'svelte';
	import { Collapsible } from '@ark-ui/svelte';
	import { collapsibleContent as content } from '$lib/styles.js';

	/**
	 * Disclosure region: `trigger` is the always-visible control (a button/summary
	 * snippet), `children` is the collapsible body. `open` is bindable.
	 */
	let {
		open = $bindable(false),
		disabled = false,
		trigger,
		children
	}: { open?: boolean; disabled?: boolean; trigger: Snippet; children: Snippet } = $props();
</script>

<Collapsible.Root bind:open {disabled}>
	<Collapsible.Trigger>{@render trigger()}</Collapsible.Trigger>
	<Collapsible.Content class={content}>{@render children()}</Collapsible.Content>
</Collapsible.Root>
