<script lang="ts">
	import type { Snippet } from 'svelte';
	import { field } from 'styled-system/recipes';

	/**
	 * Vertical label → control → message stack. Pass the control as children and
	 * its `id` via `for` so the label is associated. `error` wins over `hint`.
	 */
	let {
		label,
		error,
		hint,
		for: htmlFor,
		children
	}: {
		label?: string;
		error?: string;
		hint?: string;
		for?: string;
		children: Snippet;
	} = $props();

	const s = field();
</script>

<div class={s.root}>
	{#if label}
		<label class={s.label} for={htmlFor}>{label}</label>
	{/if}
	{@render children()}
	{#if error}
		<p class={s.error} role="alert" id={htmlFor ? `${htmlFor}-error` : undefined}>{error}</p>
	{:else if hint}
		<p class={s.hint}>{hint}</p>
	{/if}
</div>
