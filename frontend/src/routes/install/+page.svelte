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

	const MIN_PASSWORD = 12;
	const USERNAME_PATTERN = /^[A-Za-z0-9._-]+$/;

	let setupToken = $state('');
	let username = $state('');
	let password = $state('');
	let confirm = $state('');
	let submitting = $state(false);
	let formError = $state('');

	const usernameError = $derived(
		username.length > 0 && !USERNAME_PATTERN.test(username)
			? 'Use only letters, digits, and . _ -'
			: ''
	);
	const passwordError = $derived(
		password.length > 0 && password.length < MIN_PASSWORD
			? `At least ${MIN_PASSWORD} characters.`
			: ''
	);
	const confirmError = $derived(
		confirm.length > 0 && confirm !== password ? 'Passwords do not match.' : ''
	);
	const canSubmit = $derived(
		setupToken.length > 0 &&
			username.length > 0 &&
			password.length >= MIN_PASSWORD &&
			confirm === password &&
			!usernameError &&
			!submitting
	);

	onMount(async () => {
		const status = await fetchSetupStatus();
		if (status?.installed) goto('/login');
	});

	async function submit(event: SubmitEvent) {
		event.preventDefault();
		if (!canSubmit) return;
		submitting = true;
		formError = '';
		const result = await postJson('/api/setup', { setup_token: setupToken, username, password });
		submitting = false;
		if (result.ok) {
			goto('/login');
			return;
		}
		formError =
			result.error === 'already_installed'
				? 'Setup has already been completed. Redirecting to sign in.'
				: result.message;
		if (result.error === 'already_installed') goto('/login');
	}
</script>

<div class="mx-auto flex max-w-md flex-col justify-center">
	<Card>
		<CardHeader>
			<CardTitle>Create the admin account</CardTitle>
			<CardDescription>
				This one-time step creates the single administrator for this Looking Glass.
			</CardDescription>
		</CardHeader>
		<CardContent>
			<form class="space-y-4" onsubmit={submit} novalidate>
				<div class="space-y-2">
					<Label for="setup-token">Setup token</Label>
					<Input
						id="setup-token"
						name="setup_token"
						autocomplete="off"
						bind:value={setupToken}
						disabled={submitting}
						aria-describedby="setup-token-hint"
						required
					/>
					<p id="setup-token-hint" class="text-sm text-muted-foreground">
						Read it from the setup-token file beside the database, or set LG_SETUP_TOKEN before first start.
					</p>
				</div>

				<div class="space-y-2">
					<Label for="username">Username</Label>
					<Input
						id="username"
						name="username"
						autocomplete="username"
						bind:value={username}
						disabled={submitting}
						aria-invalid={usernameError ? 'true' : undefined}
						aria-describedby={usernameError ? 'username-error' : undefined}
						required
					/>
					{#if usernameError}
						<p id="username-error" class="text-sm text-destructive" role="alert">{usernameError}</p>
					{/if}
				</div>

				<div class="space-y-2">
					<Label for="password">Password</Label>
					<Input
						id="password"
						name="password"
						type="password"
						autocomplete="new-password"
						bind:value={password}
						disabled={submitting}
						aria-invalid={passwordError ? 'true' : undefined}
						aria-describedby={passwordError ? 'password-error' : undefined}
						required
					/>
					{#if passwordError}
						<p id="password-error" class="text-sm text-destructive" role="alert">{passwordError}</p>
					{/if}
				</div>

				<div class="space-y-2">
					<Label for="confirm">Confirm password</Label>
					<Input
						id="confirm"
						name="confirm"
						type="password"
						autocomplete="new-password"
						bind:value={confirm}
						disabled={submitting}
						aria-invalid={confirmError ? 'true' : undefined}
						aria-describedby={confirmError ? 'confirm-error' : undefined}
						required
					/>
					{#if confirmError}
						<p id="confirm-error" class="text-sm text-destructive" role="alert">{confirmError}</p>
					{/if}
				</div>

				{#if formError}
					<p class="text-sm text-destructive" role="alert">{formError}</p>
				{/if}

				<Button type="submit" class="w-full" disabled={!canSubmit}>
					{#if submitting}
						<LoaderCircle class="animate-spin" aria-hidden="true" />
						Creating account
					{:else}
						Create account
					{/if}
				</Button>
			</form>
		</CardContent>
	</Card>
</div>
