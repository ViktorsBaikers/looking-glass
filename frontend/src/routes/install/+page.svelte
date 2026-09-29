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
		authTitle,
		formStack,
		formErrorText,
		fullWidth
	} from '$lib/auth/styles.js';

	const MIN_PASSWORD = 12;
	const MAX_PASSWORD = 512;
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
	// Central counts UTF-8 bytes, so a character beyond ASCII counts as 2 to 4.
	const passwordBytes = $derived(new TextEncoder().encode(password).length);
	const byteNote = $derived(passwordBytes > password.length ? ' Accented letters, other scripts and emoji count as 2 to 4 each.' : '');
	const passwordError = $derived(
		passwordBytes > MAX_PASSWORD
			? `At most ${MAX_PASSWORD} characters.${byteNote}`
			: passwordBytes > 0 && passwordBytes < MIN_PASSWORD
				? `At least ${MIN_PASSWORD} characters.${byteNote}`
				: ''
	);
	const confirmError = $derived(confirm.length > 0 && confirm !== password ? 'Passwords do not match.' : '');
	// A rule is a hint while its field is being typed in; leaving the field
	// shows it as an error (Create account stays disabled until the rules pass).
	// Editing Password makes the match rule a hint again until Password is left.
	// Typing resets in the capture phase: a delegated oninput runs after
	// bind:value has already rendered the alert (F-260).
	let shown = $state({ password: false, confirm: false });
	const canSubmit = $derived(
		setupToken.length > 0 &&
			username.length > 0 &&
			passwordBytes >= MIN_PASSWORD &&
			passwordBytes <= MAX_PASSWORD &&
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
			<CardTitle class={authTitle}>Create the first administrator</CardTitle>
			<CardDescription>
				This one-time step creates the first administrator for this Looking Glass. You can add more
				from Administrators later.
			</CardDescription>
		</CardHeader>
		<CardContent>
			<form class={formStack} onsubmit={submit} novalidate aria-busy={submitting || undefined}>
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
						readonly={submitting}
						aria-disabled={submitting || undefined}
						required
					/>
				</Field>

				<Field label="Username" for="username" error={usernameError}>
					<Input
						id="username"
						name="username"
						autocomplete="username"
						bind:value={username}
						readonly={submitting}
						aria-disabled={submitting || undefined}
						invalid={!!usernameError}
						required
					/>
				</Field>

				<Field
					label="Password"
					for="password"
					error={shown.password ? passwordError : undefined}
					hint={shown.password ? undefined : passwordError}
				>
					<Input
						id="password"
						name="password"
						type="password"
						autocomplete="new-password"
						bind:value={password}
						readonly={submitting}
						aria-disabled={submitting || undefined}
						invalid={shown.password && !!passwordError}
						oninputcapture={() => (shown = { password: false, confirm: false })}
						onblur={() => (shown = { password: true, confirm: shown.confirm || confirm.length > 0 })}
						required
					/>
				</Field>

				<Field
					label="Confirm password"
					for="confirm"
					error={shown.confirm ? confirmError : undefined}
					hint={shown.confirm ? undefined : confirmError}
				>
					<Input
						id="confirm"
						name="confirm"
						type="password"
						autocomplete="new-password"
						bind:value={confirm}
						readonly={submitting}
						aria-disabled={submitting || undefined}
						invalid={shown.confirm && !!confirmError}
						oninputcapture={() => (shown.confirm = false)}
						onblur={() => (shown.confirm = true)}
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
