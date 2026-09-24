<script lang="ts">
	import { Switch } from '@ark-ui/svelte';
	import { cx } from 'styled-system/css';
	import { toggle } from 'styled-system/recipes';

	let {
		checked = $bindable(false),
		disabled = false,
		name,
		label,
		class: className
	}: {
		checked?: boolean;
		disabled?: boolean;
		name?: string;
		label?: string;
		class?: string;
	} = $props();

	const s = toggle();
</script>

<Switch.Root bind:checked {disabled} {name} class={cx(s.root, className)}>
	<Switch.Control class={s.control}>
		<Switch.Thumb class={s.thumb} />
	</Switch.Control>
	{#if label}
		<Switch.Label class={s.label}>{label}</Switch.Label>
	{/if}
	<Switch.HiddenInput />
</Switch.Root>
