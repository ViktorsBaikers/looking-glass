<script lang="ts" module>
	export type ButtonVariant = 'primary' | 'secondary' | 'ghost' | 'danger';
	export type ButtonSize = 'sm' | 'md' | 'lg' | 'icon';
</script>

<script lang="ts">
	import type { HTMLButtonAttributes } from 'svelte/elements';
	import type { Snippet } from 'svelte';
	import { cx } from 'styled-system/css';
	import { button } from 'styled-system/recipes';
	import { spin } from '$lib/styles.js';
	import Spinner from '~icons/material-symbols/progress-activity';

	let {
		variant = 'primary',
		size = 'md',
		loading = false,
		disabled = false,
		class: className,
		children,
		...rest
	}: Omit<HTMLButtonAttributes, 'class'> & {
		class?: string;
		variant?: ButtonVariant;
		size?: ButtonSize;
		/** Shows a spinner and disables the button (sets aria-busy). */
		loading?: boolean;
		children?: Snippet;
	} = $props();
</script>

<button
	class={cx(button({ variant, size }), className)}
	disabled={disabled || loading}
	aria-busy={loading || undefined}
	{...rest}
>
	{#if loading}
		<Spinner class={spin} aria-hidden="true" />
	{/if}
	{@render children?.()}
</button>
