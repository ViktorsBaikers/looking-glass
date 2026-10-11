<script lang="ts">
	import { onMount } from 'svelte';
	import { goto } from '$app/navigation';
	import { page } from '$app/state';
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
	import { activate, getActivation } from '$lib/admin/api.js';
	import {
		authPage,
		authCard,
		authTitle,
		formStack,
		formErrorText,
		fullWidth,
		activateWho,
		activateMessage
	} from '$lib/auth/styles.js';

	const MIN_PASSWORD = 12;
	const MAX_PASSWORD = 512;

	const token = $derived(page.params.token ?? '');

	let phase = $state<'loading' | 'ready' | 'invalid'>('loading');
	let username = $state('');
	let message = $state('');
	let password = $state('');
	let confirm = $state('');
	let submitting = $state(false);
	let formError = $state('');

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
	// shows it as an error (Set password stays disabled until the rules pass).
	// Editing Password makes the match rule a hint again until Password is left.
	// Typing resets in the capture phase: a delegated oninput runs after
	// bind:value has already rendered the alert (F-260).
	let shown = $state({ password: false, confirm: false });
	const canSubmit = $derived(
		passwordBytes >= MIN_PASSWORD && passwordBytes <= MAX_PASSWORD && confirm === password && !submitting
	);

	onMount(async () => {
		const result = await getActivation(token);
		if (result.ok) {
			username = result.data.username;
			phase = 'ready';
		} else {
			message = result.message;
			phase = 'invalid';
		}
	});

	async function submit(event: SubmitEvent) {
		event.preventDefault();
		if (!canSubmit) return;
		submitting = true;
		formError = '';
		const result = await activate(token, password);
		submitting = false;
		if (result.ok) {
			goto('/login');
			return;
		}
		if (result.error === 'activation_invalid') {
			message = result.message;
			phase = 'invalid';
			return;
		}
		formError = result.message;
	}
</script>

<div class={authPage}>
	<Card class={authCard}>
		<CardHeader>
			<CardTitle class={authTitle}>Activate your account</CardTitle>
			<CardDescription>
				{phase === 'ready'
					? 'Choose the password you will sign in with.'
					: 'One-time activation for a pending administrator.'}
			</CardDescription>
		</CardHeader>
		<CardContent>
			{#if phase === 'loading'}
				<p class={activateMessage}>Checking your activation link…</p>
			{:else if phase === 'invalid'}
				<p class={activateMessage} role="alert">{message}</p>
			{:else}
				<form class={formStack} onsubmit={submit} novalidate aria-busy={submitting || undefined}>
					<p class={activateWho}>{username}</p>

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
						{submitting ? 'Setting password' : 'Set password'}
					</Button>
				</form>
			{/if}
		</CardContent>
	</Card>
</div>
