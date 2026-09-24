<script lang="ts">
	import { onMount } from 'svelte';
	import { afterNavigate, goto } from '$app/navigation';
	import { page } from '$app/state';
	import '../app.css';
	// Self-hosted variable fonts: Overpass (UI) and Overpass Mono (tool output).
	import '@fontsource-variable/overpass';
	import '@fontsource-variable/overpass-mono';
	import ThemeToggle from '$lib/components/theme-toggle.svelte';
	import Toaster from '$lib/components/ui/toaster.svelte';
	import { fetchSetupStatus, getJson } from '$lib/api.js';
	import { fetchPublicSettings } from '$lib/public/settings.js';
	import { theme } from '$lib/theme.svelte.js';
	import { cx } from 'styled-system/css';
	import {
		shell,
		header,
		headerInner,
		brand,
		brandMark,
		logo,
		nav,
		navLink,
		navLinkActive,
		headerRight,
		mainArea as main,
		footer,
		footerInner,
		termsLink,
		customText
	} from '$lib/styles.js';

	let { children } = $props();

	let siteTitle = $state('Looking Glass');
	let logoUrl = $state<string | null>(null);
	let termsUrl = $state<string | null>(null);
	let customBlock = $state<string | null>(null);
	let isAdmin = $state(false);

	// Route classes drive which shell chrome shows. Login / install / activate are
	// bare centred cards; admin pages get the sidebar (in admin/+layout) and no
	// public footer.
	const path = $derived(page.url.pathname);
	const isAuthRoute = $derived(
		path === '/login' || path === '/install' || path.startsWith('/activate')
	);
	const isAdminRoute = $derived(path === '/admin' || path.startsWith('/admin/'));
	const isDiagnostics = $derived(path === '/');
	const showHeader = $derived(!isAuthRoute);
	const showFooter = $derived(!isAuthRoute && !isAdminRoute && !!(termsUrl || customBlock));

	// Re-check the session after every navigation so the Administration link
	// appears right after an in-app sign-in and disappears after log-out.
	afterNavigate(async () => {
		isAdmin = (await getJson<{ id: number; username: string }>('/api/admin/me')).ok;
	});

	onMount(async () => {
		const [status, settings] = await Promise.all([fetchSetupStatus(), fetchPublicSettings()]);
		if (status && !status.installed && path !== '/install') {
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

<div class={shell}>
	{#if showHeader}
		<header class={header}>
			<div class={headerInner}>
				<a href="/" class={brand}>
					{#if logoUrl}
						<img src={logoUrl} alt="" class={logo} />
					{:else}
						<span class={brandMark} aria-hidden="true"></span>
					{/if}
					<span>{siteTitle}</span>
				</a>
				<div class={headerRight}>
					<nav class={nav} aria-label="Main">
						<a
							href="/"
							class={cx(navLink, isDiagnostics ? navLinkActive : '')}
							aria-current={isDiagnostics ? 'page' : undefined}>Diagnostics</a
						>
						{#if isAdmin}
							<a
								href="/admin"
								class={cx(navLink, isAdminRoute ? navLinkActive : '')}
								aria-current={isAdminRoute ? 'page' : undefined}>Administration</a
							>
						{/if}
					</nav>
					<ThemeToggle />
				</div>
			</div>
		</header>
	{/if}

	<main class={main}>
		{@render children?.()}
	</main>

	{#if showFooter}
		<footer class={footer}>
			<div class={footerInner}>
				{#if termsUrl}
					<a
						href={termsUrl}
						target="_blank"
						rel="noopener noreferrer"
						class={termsLink}
					>
						Terms
					</a>
				{/if}
				{#if customBlock}
					<p class={customText}>{customBlock}</p>
				{/if}
			</div>
		</footer>
	{/if}
</div>

<Toaster />
