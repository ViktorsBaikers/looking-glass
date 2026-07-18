<script lang="ts">
	import { onMount } from 'svelte';
	import { goto } from '$app/navigation';
	import LoaderCircle from '@lucide/svelte/icons/loader-circle';
	import { Button } from '$lib/components/ui/button/index.js';
	import { Input } from '$lib/components/ui/input/index.js';
	import { Label } from '$lib/components/ui/label/index.js';
	import {
		Card,
		CardContent,
		CardDescription,
		CardHeader,
		CardTitle
	} from '$lib/components/ui/card/index.js';
	import { fetchSetupStatus, postJson } from '$lib/api.js';

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

<div class="mx-auto flex max-w-md flex-col justify-center">
	<Card>
		<CardHeader>
			<CardTitle>Sign in</CardTitle>
			<CardDescription>Sign in to manage locations and settings.</CardDescription>
		</CardHeader>
		<CardContent>
			<form class="space-y-4" onsubmit={submit} novalidate>
				<div class="space-y-2">
					<Label for="username">Username</Label>
					<Input
						id="username"
						name="username"
						autocomplete="username"
						bind:value={username}
						disabled={submitting}
						required
					/>
				</div>

				<div class="space-y-2">
					<Label for="password">Password</Label>
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
				</div>

				{#if formError}
					<p id="login-error" class="text-sm text-destructive" role="alert">{formError}</p>
				{/if}

				<Button type="submit" class="w-full" disabled={!canSubmit}>
					{#if submitting}
						<LoaderCircle class="animate-spin" aria-hidden="true" />
						Signing in
					{:else}
						Sign in
					{/if}
				</Button>
			</form>
		</CardContent>
	</Card>
</div>
