<script lang="ts">
	import type { HTMLLabelAttributes } from 'svelte/elements';
	import type { Snippet } from 'svelte';
	import { cx } from 'styled-system/css';
	import { labelStyle as style } from '$lib/styles.js';

	let {
		class: className,
		children,
		...rest
	}: Omit<HTMLLabelAttributes, 'class'> & { class?: string; children?: Snippet } = $props();
</script>

<label class={cx(style, className)} {...rest}>
	{@render children?.()}
</label>
