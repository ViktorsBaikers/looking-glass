<script lang="ts">
	import type { Snippet } from 'svelte';
	import type { BadgeTone } from './badge.svelte';
	import { cx } from 'styled-system/css';
	import { badge } from 'styled-system/recipes';
	import { statusBadgeDot as dot, statusDotTone as dotTone } from '$lib/styles.js';

	/** A Badge prefixed with a status dot (Online / Offline / Pending / etc.). */
	let {
		tone = 'neutral',
		size = 'sm',
		class: className,
		children
	}: { tone?: BadgeTone; size?: 'sm' | 'lg'; class?: string; children?: Snippet } = $props();
</script>

<span class={cx(badge({ tone, size }), className)}>
	<span class={cx(dot, dotTone[tone])} aria-hidden="true"></span>
	{@render children?.()}
</span>
