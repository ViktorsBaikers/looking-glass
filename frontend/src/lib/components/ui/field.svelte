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
</script>

<div class={s.root}>
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
		<p class={s.hint}>{hint}</p>
	{/if}
</div>
