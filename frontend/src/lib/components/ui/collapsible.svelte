<script lang="ts">
	import type { Snippet } from 'svelte';
	import type { HTMLAttributes } from 'svelte/elements';
	import { Collapsible } from '@ark-ui/svelte';
	import { collapsibleContent as content } from '$lib/styles.js';

	/**
	 * Disclosure region: `trigger` renders the always-visible control and spreads
	 * the trigger props it receives onto it (so a Button is the trigger itself,
	 * not nested in one); `children` is the collapsible body. `open` is bindable.
	 */
	let {
		open = $bindable(false),
		disabled = false,
		trigger,
		children
	}: {
		open?: boolean;
		disabled?: boolean;
		trigger: Snippet<[() => HTMLAttributes<HTMLElement>]>;
		children: Snippet;
	} = $props();
</script>

<Collapsible.Root bind:open {disabled}>
	<Collapsible.Trigger>
		{#snippet asChild(props)}{@render trigger(props)}{/snippet}
	</Collapsible.Trigger>
	<Collapsible.Content class={content}>{@render children()}</Collapsible.Content>
</Collapsible.Root>
