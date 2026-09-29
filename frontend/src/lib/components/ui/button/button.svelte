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
		onclick,
		children,
		...rest
	}: Omit<HTMLButtonAttributes, 'class'> & {
		class?: string;
		variant?: ButtonVariant;
		size?: ButtonSize;
		/** Shows a spinner and makes the button busy and inert (aria-busy + aria-disabled). */
		loading?: boolean;
		children?: Snippet;
	} = $props();
</script>

<!-- Loading is busy, not `disabled`: disabling the focused button drops focus
     to <body>. aria-disabled gives the recipe's dimmed look; pointer-events
     stays on so a press lands here and is swallowed (no click, no submit, and
     no implicit submission from Enter in a field) instead of falling through.
     Loading wins over an explicit disabled: forms pass disabled={!canSubmit}
     with !submitting inside canSubmit. -->
<button
	class={cx(button({ variant, size }), className)}
	disabled={disabled && !loading}
	aria-busy={loading || undefined}
	aria-disabled={loading || undefined}
	style={loading ? 'pointer-events: auto' : undefined}
	{...rest}
	onclick={(event) => {
		if (loading) event.preventDefault();
		else onclick?.(event);
	}}
>
	{#if loading}
		<Spinner class={spin} aria-hidden="true" />
	{/if}
	{@render children?.()}
</button>
