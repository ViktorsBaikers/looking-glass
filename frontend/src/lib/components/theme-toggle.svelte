<script lang="ts">
	import Moon from '@lucide/svelte/icons/moon';
	import Sun from '@lucide/svelte/icons/sun';
	import { Button } from '$lib/components/ui/button/index.js';
	import { theme } from '$lib/theme.svelte.js';
</script>

<Button
	variant="ghost"
	size="icon"
	onclick={() => theme.toggle()}
	aria-label={theme.dark ? 'Switch to light theme' : 'Switch to dark theme'}
>
	{#if theme.dark}
		<Sun aria-hidden="true" />
	{:else}
		<Moon aria-hidden="true" />
	{/if}
</Button>
