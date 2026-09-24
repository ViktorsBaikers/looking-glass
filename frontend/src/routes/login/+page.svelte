<script lang="ts">
	import { onMount } from 'svelte';
	import { goto } from '$app/navigation';
	import { Button } from '$lib/components/ui/button/index.js';
	import { Input } from '$lib/components/ui/input/index.js';
	import Field from '$lib/components/ui/field.svelte';
	import {
		Card,
		CardContent,
		CardDescription,
		CardHeader,
		CardTitle
	} from '$lib/components/ui/card/index.js';
	import { fetchSetupStatus, postJson } from '$lib/api.js';
	import {
		authPage,
		authCard,
		formStack,
		formErrorText,
		fullWidth
	} from '$lib/auth/styles.js';

	let username = $state('');
	let password = $state('');
	let submitting = $state(false);
	let formError = $state('');

	const canSubmit = $derived(username.length > 0 && password.length > 0 && !submitting);

	onMount(async () => {
		const status = await fetchSetupStatus();
		if (status && !status.installed) goto('/install');
	});

	async function submit(event: SubmitEvent) {
		event.preventDefault();
		if (!canSubmit) return;
		submitting = true;
		formError = '';
		const result = await postJson('/api/auth/login', { username, password });
		submitting = false;
		if (result.ok) {
			goto('/admin');
			return;
		}
		formError = result.message;
	}
</script>

<div class={authPage}>
	<Card class={authCard}>
		<CardHeader>
			<CardTitle>Sign in</CardTitle>
			<CardDescription>Sign in to manage locations and settings.</CardDescription>
		</CardHeader>
		<CardContent>
			<form class={formStack} onsubmit={submit} novalidate>
				<Field label="Username" for="username">
					<Input
						id="username"
						name="username"
						autocomplete="username"
						bind:value={username}
						disabled={submitting}
						required
					/>
				</Field>

				<Field label="Password" for="password">
					<Input
						id="password"
						name="password"
						type="password"
						autocomplete="current-password"
						bind:value={password}
						disabled={submitting}
						aria-describedby={formError ? 'login-error' : undefined}
						required
					/>
				</Field>

				{#if formError}
					<p id="login-error" class={formErrorText} role="alert">{formError}</p>
				{/if}

				<Button type="submit" class={fullWidth} loading={submitting} disabled={!canSubmit}>
					{submitting ? 'Signing in' : 'Sign in'}
				</Button>
			</form>
		</CardContent>
	</Card>
</div>
