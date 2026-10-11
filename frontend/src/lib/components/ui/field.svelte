<script lang="ts">
	import type { Snippet } from 'svelte';
	import { field } from 'styled-system/recipes';
	import Info from '~icons/material-symbols/info';
	import Tooltip from './tooltip.svelte';
	import { fieldLabelRow, fieldInfoIcon, srOnly } from '$lib/styles.js';

	/**
	 * Vertical label → control → message stack. Pass the control as children and
	 * its `id` via `for` so the label is associated. `error` wins over `hint`.
	 * `info` adds an icon after the label that explains the field on hover/focus.
	 */
	let {
		label,
		error,
		hint,
		info,
		for: htmlFor,
		children
	}: {
		label?: string;
		error?: string;
		hint?: string;
		info?: string;
		for?: string;
		children: Snippet;
	} = $props();

	const s = field();
	let root: HTMLDivElement;

	// The control arrives as children, so its description is wired from here.
	// Look it up inside this field: another mounted field (an outgoing tab
	// panel) can hold an element with the same id.
	$effect(() => {
		const described = error ? `${htmlFor}-error` : hint ? `${htmlFor}-hint` : '';
		const control = htmlFor && described ? root.querySelector(`[id="${htmlFor}"]`) : null;
		if (!control) return;
		control.setAttribute('aria-describedby', described);
		return () => control.removeAttribute('aria-describedby');
	});
</script>

<div class={s.root} bind:this={root}>
	{#if label && info}
		<div class={fieldLabelRow}>
			<label class={s.label} for={htmlFor}>{label}</label>
			<Tooltip content={info}>
				<span class={fieldInfoIcon}>
					<Info aria-hidden="true" />
					<span class={srOnly}>About {label}</span>
				</span>
			</Tooltip>
		</div>
	{:else if label}
		<label class={s.label} for={htmlFor}>{label}</label>
	{/if}
	{@render children()}
	{#if error}
		<p class={s.error} role="alert" id={htmlFor ? `${htmlFor}-error` : undefined}>{error}</p>
	{:else if hint}
		<p class={s.hint} id={htmlFor ? `${htmlFor}-hint` : undefined}>{hint}</p>
	{/if}
</div>
