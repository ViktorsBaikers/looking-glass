<script lang="ts">
	import { afterNavigate, goto } from '$app/navigation';
	import { page } from '$app/state';
	import { Dialog, Portal } from '@ark-ui/svelte';
	import { getJson, postJson } from '$lib/api.js';
	import { cx } from 'styled-system/css';
	import {
		spin,
		checking,
		srOnly,
		adminShell,
		sidebar,
		heading,
		navList,
		navItem,
		navItemActive,
		logoutWrap,
		logoutBtn,
		contentCol,
		mobileBar,
		menuBtn,
		content,
		drawerBackdrop,
		drawerPositioner,
		drawerContent,
		drawerClose
	} from '$lib/styles.js';
	import Logout from '~icons/material-symbols/logout';
	import Menu from '~icons/material-symbols/menu';
	import Spinner from '~icons/material-symbols/progress-activity';
	import Close from '~icons/material-symbols/close';

	let { children } = $props();

	// 'checking' until the fail-closed admin gate answers; the panel renders only
	// once a session is confirmed, otherwise we route to login.
	let gate: 'checking' | 'ready' = $state('checking');
	let drawerOpen = $state(false);

	// Fail-closed gate: runs on mount and again on every admin route change
	// (SvelteKit keeps this layout mounted across sidebar navigation), so an
	// expired or revoked session is bounced to sign-in on the next move.
	// Query-only navigations (the editor's ?tab= deep links) keep the gate.
	afterNavigate((navigation) => {
		if (navigation.type !== 'enter' && navigation.from?.url.pathname === navigation.to?.url.pathname) {
			return;
		}
		void revalidate();
	});

	async function revalidate() {
		const me = await getJson<{ id: number; username: string }>('/api/admin/me');
		if (me.ok) {
			gate = 'ready';
		} else {
			goto('/login');
		}
	}

	async function logout() {
		await postJson('/api/auth/logout', {});
		goto('/login');
	}

	const path = $derived(page.url.pathname);
	const nav: { href: string; label: string; active: boolean }[] = $derived([
		{
			href: '/admin',
			label: 'Locations',
			active: path === '/admin' || path.startsWith('/admin/locations')
		},
		{
			href: '/admin/administrators',
			label: 'Administrators',
			active: path.startsWith('/admin/administrators')
		},
		{
			href: '/admin/settings',
			label: 'Settings',
			active: path.startsWith('/admin/settings')
		}
	]);

</script>

{#if gate === 'checking'}
	<div class={checking}>
		<Spinner class={spin} aria-hidden="true" />
		<span class={srOnly}>Checking your session…</span>
	</div>
{:else}
	<div class={adminShell}>
		<aside class={sidebar} aria-label="Administration">
			<div class={heading}>Administration</div>
			<nav class={navList}>
				{#each nav as item (item.href)}
					<a
						href={item.href}
						class={cx(navItem, item.active ? navItemActive : '')}
						aria-current={item.active ? 'page' : undefined}
					>
						{item.label}
					</a>
				{/each}
			</nav>
			<div class={logoutWrap}>
				<button type="button" class={logoutBtn} onclick={logout}>
					<Logout aria-hidden="true" />
					Log out
				</button>
			</div>
		</aside>

		<div class={contentCol}>
			<div class={mobileBar}>
				<button
					type="button"
					class={menuBtn}
					onclick={() => (drawerOpen = true)}
					aria-label="Open administration menu"
				>
					<Menu aria-hidden="true" />
				</button>
				Administration
			</div>
			<div class={content}>
				{@render children?.()}
			</div>
		</div>
	</div>

	<Dialog.Root bind:open={drawerOpen}>
		<Portal>
			<Dialog.Backdrop class={drawerBackdrop} />
			<Dialog.Positioner class={drawerPositioner}>
				<Dialog.Content class={drawerContent}>
					<Dialog.Title class={srOnly}>Administration</Dialog.Title>
					<Dialog.CloseTrigger class={drawerClose} aria-label="Close menu">
						<Close />
					</Dialog.CloseTrigger>
					<div class={heading}>Administration</div>
					<nav class={navList}>
						{#each nav as item (item.href)}
							<a
								href={item.href}
								class={cx(navItem, item.active ? navItemActive : '')}
								aria-current={item.active ? 'page' : undefined}
								onclick={() => (drawerOpen = false)}
							>
								{item.label}
							</a>
						{/each}
					</nav>
					<div class={logoutWrap}>
						<button
							type="button"
							class={logoutBtn}
							onclick={() => {
								drawerOpen = false;
								logout();
							}}
						>
							<Logout aria-hidden="true" />
							Log out
						</button>
					</div>
				</Dialog.Content>
			</Dialog.Positioner>
		</Portal>
	</Dialog.Root>
{/if}
