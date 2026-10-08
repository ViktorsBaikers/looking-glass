<script lang="ts">
	import { page } from '$app/state';
	import { button } from 'styled-system/recipes';
	import { Button } from '$lib/components/ui/button/index.js';
	import {
		errorPage,
		lostMap,
		lostOrigin,
		lostTrack,
		lostGap,
		lostStop,
		lostFrom,
		lostTo,
		errorText,
		errorTitle,
		errorLede,
		errorPath,
		errorActions
	} from '$lib/styles.js';

	const missing = $derived(page.status === 404);
</script>

<div class={errorPage}>
	{#if missing}
		<!-- The line runs out before the requested stop. -->
		<div class={lostMap} aria-hidden="true">
			<span class={lostOrigin}></span>
			<span class={lostTrack}></span>
			<span class={lostGap}></span>
			<span class={lostStop}></span>
			<span class={lostFrom}>Looking Glass</span>
			<span class={lostTo}>{page.url.pathname}</span>
		</div>
	{/if}

	<div class={errorText}>
		<h1 class={errorTitle}>{missing ? 'Page not found' : 'This page could not be loaded'}</h1>
		<p class={errorLede}>
			{#if missing}
				There's no page at <span class={errorPath}>{page.url.pathname}</span>. The link may be
				mistyped, or the page has moved.
			{:else}
				Something failed while opening it. Reload to try again.
			{/if}
		</p>
	</div>

	<div class={errorActions}>
		{#if missing}
			<a href="/" class={button({ variant: 'primary' })}>Homepage</a>
		{:else}
			<Button onclick={() => location.reload()}>Reload</Button>
			<a href="/" class={button({ variant: 'secondary' })}>Homepage</a>
		{/if}
	</div>
</div>
