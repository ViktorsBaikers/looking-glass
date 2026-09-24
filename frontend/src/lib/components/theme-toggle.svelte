<script lang="ts">
	import { themeToggleBtn as btn } from '$lib/styles.js';
	import { theme } from '$lib/theme.svelte.js';
	import LightMode from '~icons/material-symbols/light-mode';
	import DarkMode from '~icons/material-symbols/dark-mode';
</script>

<button
	type="button"
	class={btn}
	onclick={() => theme.toggle()}
	aria-label={theme.dark ? 'Switch to light theme' : 'Switch to dark theme'}
>
	{#if theme.dark}
		<LightMode aria-hidden="true" />
	{:else}
		<DarkMode aria-hidden="true" />
	{/if}
</button>
