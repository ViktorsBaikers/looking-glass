<script lang="ts">
	import type { Snippet } from 'svelte';
	import { Checkbox } from '@ark-ui/svelte';
	import { cx } from 'styled-system/css';
	import { checkbox } from 'styled-system/recipes';
	import Check from '~icons/material-symbols/check';

	let {
		checked = $bindable(false),
		disabled = false,
		name,
		value,
		class: className,
		children
	}: {
		checked?: boolean;
		disabled?: boolean;
		name?: string;
		value?: string;
		class?: string;
		/** Optional label content rendered beside the box. */
		children?: Snippet;
	} = $props();

	const s = checkbox();
</script>

<Checkbox.Root bind:checked {disabled} {name} {value} class={cx(s.root, className)}>
	<Checkbox.Control class={s.control}>
		<Checkbox.Indicator class={s.indicator}>
			<Check />
		</Checkbox.Indicator>
	</Checkbox.Control>
	{#if children}
		<Checkbox.Label class={s.label}>{@render children()}</Checkbox.Label>
	{/if}
	<Checkbox.HiddenInput />
</Checkbox.Root>
