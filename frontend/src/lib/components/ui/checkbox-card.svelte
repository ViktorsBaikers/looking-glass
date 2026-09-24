<script lang="ts">
	import { Checkbox } from '@ark-ui/svelte';
	import { cx } from 'styled-system/css';
	import { checkbox, checkboxCard } from 'styled-system/recipes';
	import { monoLabel as mono } from '$lib/styles.js';
	import Check from '~icons/material-symbols/check';

	/**
	 * A whole-card checkbox for the Methods grid: the card is the control, with a
	 * monospace method name as its label.
	 */
	let {
		checked = $bindable(false),
		disabled = false,
		name,
		value,
		label,
		class: className
	}: {
		checked?: boolean;
		disabled?: boolean;
		name?: string;
		value?: string;
		label: string;
		class?: string;
	} = $props();

	const cb = checkbox();
</script>

<Checkbox.Root
	bind:checked
	{disabled}
	{name}
	{value}
	class={cx(checkboxCard(), className)}
>
	<Checkbox.Control class={cb.control}>
		<Checkbox.Indicator class={cb.indicator}>
			<Check />
		</Checkbox.Indicator>
	</Checkbox.Control>
	<Checkbox.Label class={cx(cb.label, mono)}>{label}</Checkbox.Label>
	<Checkbox.HiddenInput />
</Checkbox.Root>
