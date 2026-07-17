<script lang="ts">
	import { onMount } from 'svelte';
	import { goto } from '$app/navigation';
	import { page } from '$app/state';
	import LoaderCircle from '@lucide/svelte/icons/loader-circle';
	import MapPin from '@lucide/svelte/icons/map-pin';
	import Settings from '@lucide/svelte/icons/settings';
	import LogOut from '@lucide/svelte/icons/log-out';
	import ThemeToggle from '$lib/components/theme-toggle.svelte';
	import Toaster from '$lib/components/ui/toaster.svelte';
	import { getJson, postJson } from '$lib/api.js';
	import { cn } from '$lib/utils.js';

	let { children } = $props();

	// 'checking' until the fail-closed admin gate answers; the panel renders only
	// once a session is confirmed, otherwise we route to login.
	let state = $state<'checking' | 'ready'>('checking');

	onMount(async () => {
		const me = await getJson<{ username: string }>('/api/admin/me');
		if (me.ok) {
			state = 'ready';
		} else {
			goto('/login');
		}
	});

	async function logout() {
		await postJson('/api/auth/logout', {});
		goto('/login');
	}

	const nav = [
		{ href: '/admin', label: 'Locations', icon: MapPin },
		{ href: '/admin/settings', label: 'Settings', icon: Settings }
	];

	function isActive(href: string): boolean {
		return href === '/admin' ? page.url.pathname === '/admin' : page.url.pathname.startsWith(href);
	}
</script>

{#if state === 'checking'}
	<div class="flex min-h-[50vh] items-center justify-center text-muted-foreground">
		<LoaderCircle class="size-5 animate-spin" aria-hidden="true" />
		<span class="sr-only">Checking your session…</span>
	</div>
{:else}
	<div class="grid gap-8 md:grid-cols-[12rem_1fr]">
		<nav class="flex flex-wrap gap-1 md:flex-col" aria-label="Admin sections">
			{#each nav as item (item.href)}
				<a
					href={item.href}
					aria-current={isActive(item.href) ? 'page' : undefined}
					class={cn(
						'flex min-h-11 items-center gap-2 rounded-md px-3 py-2 text-sm font-medium transition-colors',
						isActive(item.href)
							? 'bg-secondary text-secondary-foreground'
							: 'text-muted-foreground hover:bg-accent hover:text-accent-foreground'
					)}
				>
					<item.icon class="size-4" aria-hidden="true" />
					{item.label}
				</a>
			{/each}
			<div class="flex items-center justify-between gap-2 md:mt-4 md:flex-col md:items-stretch">
				<button
					type="button"
					onclick={logout}
					class="flex min-h-11 items-center gap-2 rounded-md px-3 py-2 text-sm font-medium text-muted-foreground transition-colors hover:bg-accent hover:text-accent-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
				>
					<LogOut class="size-4" aria-hidden="true" />
					Log out
				</button>
				<div class="px-1"><ThemeToggle /></div>
			</div>
		</nav>

		<div class="min-w-0">
			{@render children?.()}
		</div>
	</div>
{/if}

<Toaster />
