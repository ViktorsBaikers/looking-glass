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
		password.length > 0 && password.length < MIN_PASSWORD ? `At least ${MIN_PASSWORD} characters.` : ''
	);
	const confirmError = $derived(confirm.length > 0 && confirm !== password ? 'Passwords do not match.' : '');
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

<div class={authPage}>
	<Card class={authCard}>
		<CardHeader>
			<CardTitle>Create the admin account</CardTitle>
			<CardDescription>
				This one-time step creates the single administrator for this Looking Glass.
			</CardDescription>
		</CardHeader>
		<CardContent>
			<form class={formStack} onsubmit={submit} novalidate>
				<Field
					label="Setup token"
					for="setup-token"
					hint="Read it from the setup-token file beside the database, or set LG_SETUP_TOKEN before first start."
				>
					<Input
						id="setup-token"
						name="setup_token"
						autocomplete="off"
						bind:value={setupToken}
						disabled={submitting}
						required
					/>
				</Field>

				<Field label="Username" for="username" error={usernameError}>
					<Input
						id="username"
						name="username"
						autocomplete="username"
						bind:value={username}
						disabled={submitting}
						invalid={!!usernameError}
						required
					/>
				</Field>

				<Field label="Password" for="password" error={passwordError}>
					<Input
						id="password"
						name="password"
						type="password"
						autocomplete="new-password"
						bind:value={password}
						disabled={submitting}
						invalid={!!passwordError}
						required
					/>
				</Field>

				<Field label="Confirm password" for="confirm" error={confirmError}>
					<Input
						id="confirm"
						name="confirm"
						type="password"
						autocomplete="new-password"
						bind:value={confirm}
						disabled={submitting}
						invalid={!!confirmError}
						required
					/>
				</Field>

				{#if formError}
					<p class={formErrorText} role="alert">{formError}</p>
				{/if}

				<Button type="submit" class={fullWidth} loading={submitting} disabled={!canSubmit}>
					{submitting ? 'Creating account' : 'Create account'}
				</Button>
			</form>
		</CardContent>
	</Card>
</div>
