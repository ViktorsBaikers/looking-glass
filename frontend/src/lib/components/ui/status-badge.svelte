<script lang="ts">
	import type { Snippet } from 'svelte';
	import type { BadgeTone } from './badge.svelte';
	import { cx } from 'styled-system/css';
	import { badge } from 'styled-system/recipes';
	import { statusBadgeDot as dot, statusDotTone as dotTone } from '$lib/styles.js';

	/** A Badge prefixed with a status dot (Online / Offline / Pending / etc.). */
	let {
		tone = 'neutral',
		class: className,
		children
	}: { tone?: BadgeTone; class?: string; children?: Snippet } = $props();
</script>

<span class={cx(badge({ tone }), className)}>
	<span class={cx(dot, dotTone[tone])} aria-hidden="true"></span>
	{@render children?.()}
</span>
