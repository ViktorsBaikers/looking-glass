<script lang="ts">
	import { afterNavigate, goto } from '$app/navigation';
	import { page } from '$app/state';
	import { onDestroy } from 'svelte';
	import { Dialog, Portal } from '@ark-ui/svelte';
	import { getJson, postJson } from '$lib/api.js';
	import { Button } from '$lib/components/ui/button/index.js';
	import { endStuckClose } from '$lib/components/ui/dialog.svelte';
	import { errorCard } from '$lib/admin/list-styles.js';
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
	// once a session is confirmed. A signed-out session goes to login; a server
	// that cannot be reached (or answers anything else) offers a retry.
	let gate: 'checking' | 'ready' | 'unreachable' = $state('checking');
	// Only a network error means the server was not reached.
	let unreachableMessage = $state('');
	let logoutError = $state('');
	let drawerOpen = $state(false);
	// The drawer gets the shared dialog's fixes: its closing backdrop lets
	// clicks through (F-196) and a stuck close is ended (F-145/F-264, F-329).
	let drawerBackdropEl = $state<Element | null>(null);
	let drawerContentEl = $state<Element | null>(null);
	endStuckClose(() => drawerOpen, () => [drawerBackdropEl, drawerContentEl]);
	// After a retry, or a route-change check that fails (the panel and its
	// focused link go away), the alert's button takes focus so a keyboard user
	// is not dropped on <body>. A failed first load leaves focus alone like any
	// page load. Only from <body>: focus the user moved during the check stays
	// put (no native autofocus, which WebKit honours even then).
	let retried = $state(false);
	const refocus = (button: HTMLElement) => {
		if (retried && (document.activeElement ?? document.body) === document.body) button.focus();
	};
	// Only the newest session check's answer counts; a slower, older one is dropped.
	let checks = 0;
	// An answer arriving after the admin area unmounted is dropped too.
	onDestroy(() => checks++);

	// Fail-closed gate: runs on mount and again on every admin route change
	// (SvelteKit keeps this layout mounted across sidebar navigation), so an
	// expired or revoked session is bounced to sign-in on the next move.
	// Query-only navigations (the editor's ?tab= deep links) keep the gate.
	afterNavigate((navigation) => {
		// A failed logout's alert does not follow the admin to the next page.
		logoutError = '';
		if (navigation.type !== 'enter' && navigation.from?.url.pathname === navigation.to?.url.pathname) {
			return;
		}
		void revalidate();
	});

	async function revalidate() {
		const check = ++checks;
		const me = await getJson<{ id: number; username: string }>('/api/admin/me');
		if (check !== checks) return;
		if (me.ok) {
			gate = 'ready';
		} else if (me.error === 'unauthorized') {
			goto('/login');
		} else {
			unreachableMessage =
				me.error === 'network'
					? 'Could not reach the server to check your session.'
					: 'The server could not check your session. Wait a moment and try again.';
			if (gate === 'ready') retried = true;
			gate = 'unreachable';
		}
	}

	function retry() {
		retried = true;
		gate = 'checking';
		void revalidate();
	}

	// A logout the server did not confirm leaves the session alive: stay and say so.
	async function logout() {
		logoutError = '';
		const result = await postJson('/api/auth/logout', {});
		if (result.ok) goto('/login');
		else logoutError = `Could not log out. ${result.message}`;
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

<p class={srOnly} role="status">
	{gate === 'checking' ? 'Checking your session…' : gate === 'ready' && retried ? 'Session confirmed.' : ''}
</p>
{#if gate === 'checking'}
	<div class={checking}>
		<Spinner class={spin} aria-hidden="true" />
	</div>
{:else if gate === 'unreachable'}
	<div class={checking}>
		<div class={errorCard} role="alert">
			<p>{unreachableMessage}</p>
			<Button variant="secondary" onclick={retry} {@attach refocus}>Try again</Button>
		</div>
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
				{#if logoutError}
					<!-- Log out sits in a sticky sidebar, so the page may be scrolled. -->
					<div class={errorCard} role="alert" {@attach (el) => el.scrollIntoView({ block: 'nearest' })}>
						{logoutError}
					</div>
				{/if}
				{@render children?.()}
			</div>
		</div>
	</div>

	<Dialog.Root bind:open={drawerOpen}>
		<Portal>
			<Dialog.Backdrop
				bind:ref={drawerBackdropEl}
				class={drawerBackdrop}
				style={drawerOpen ? undefined : 'pointer-events: none'}
			/>
			<Dialog.Positioner class={drawerPositioner}>
				<Dialog.Content bind:ref={drawerContentEl} class={drawerContent}>
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
