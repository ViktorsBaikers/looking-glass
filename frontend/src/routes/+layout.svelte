<script lang="ts">
	import { onMount } from 'svelte';
	import { goto } from '$app/navigation';
	import '../app.css';
	import ThemeToggle from '$lib/components/theme-toggle.svelte';
	import { fetchSetupStatus } from '$lib/api.js';
	import { fetchPublicSettings } from '$lib/public/settings.js';
	import { theme } from '$lib/theme.svelte.js';

	let { children } = $props();
	let siteTitle = $state('Looking Glass');
	let logoUrl = $state<string | null>(null);
	let termsUrl = $state<string | null>(null);
	let customBlock = $state<string | null>(null);

	onMount(async () => {
		const [status, settings] = await Promise.all([fetchSetupStatus(), fetchPublicSettings()]);
		if (status && !status.installed && window.location.pathname !== '/install') {
			goto('/install');
		}
		if (settings) {
			siteTitle = settings.site_title;
			logoUrl = settings.logo_url;
			termsUrl = settings.terms_url;
			customBlock = settings.custom_block;
			document.title = settings.site_title;
			theme.applyDefault(settings.default_theme);
		}
	});
</script>

<div class="flex min-h-screen flex-col">
	<header class="border-b border-border">
		<div class="mx-auto flex h-14 w-full max-w-5xl items-center justify-between px-4">
				<a href="/" class="flex min-h-11 items-center gap-2 font-semibold tracking-tight">
					{#if logoUrl}
						<img src={logoUrl} alt="" class="size-7 object-contain" />
					{:else}
						<span class="inline-block size-2 rounded-full bg-status-online" aria-hidden="true"></span>
					{/if}
					{siteTitle}
				</a>
				<div class="flex items-center gap-2">
					{#if termsUrl}
						<a
							href={termsUrl}
							target="_blank"
							rel="noopener noreferrer"
							class="inline-flex min-h-11 items-center text-sm text-primary underline-offset-4 hover:underline"
						>
							Terms
						</a>
					{/if}
					<ThemeToggle />
				</div>
			</div>
			{#if customBlock}
				<p class="mx-auto w-full max-w-5xl px-4 pb-3 text-sm text-muted-foreground">{customBlock}</p>
			{/if}
	</header>

	<main class="mx-auto w-full max-w-5xl flex-1 px-4 py-10">
		{@render children?.()}
	</main>
</div>
