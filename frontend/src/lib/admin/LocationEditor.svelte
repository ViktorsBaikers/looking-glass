<script lang="ts">
	import { untrack } from 'svelte';
	import { Button } from '$lib/components/ui/button/index.js';
	import { Input } from '$lib/components/ui/input/index.js';
	import Field from '$lib/components/ui/field.svelte';
	import Select from '$lib/components/ui/select.svelte';
	import Tabs from '$lib/components/ui/tabs.svelte';
	import CheckboxCard from '$lib/components/ui/checkbox-card.svelte';
	import CrudSection from './CrudSection.svelte';
	import EnrollmentTab from './EnrollmentTab.svelte';
	import { toast } from '$lib/toast.svelte.js';
	import {
		backLink,
		pageHead,
		pageTitle,
		pageSub,
		headRoundel,
		panelCard,
		panelTitle,
		methodsIntro,
		familyHead,
		familyGroup,
		methodsGrid,
		formStack,
		formRow2,
		saveRow,
		methodsSave,
		loadingRow,
		errorText,
		retryBtn,
		spinIcon,
		tdMono
	} from './editor-styles.js';
	import {
		getLocation,
		updateLocation,
		createTestIp,
		updateTestIp,
		deleteTestIp,
		createIperf,
		updateIperf,
		deleteIperf,
		createTestFile,
		updateTestFile,
		deleteTestFile,
		createEnrollment,
		type LocationInput
	} from './api.js';
	import { asnError, asnValue, methodsByFamily, nullIfBlank } from './editor.js';
	import {
		OFFERED_METHODS,
		type LocationDetail,
		type OfferedMethod,
		type TestIp,
		type IperfEndpoint,
		type TestFile,
		type EnrollmentTicket
	} from './types.js';
	import StatusBadge from '$lib/components/ui/status-badge.svelte';
	import { lineCode, lineStyle } from '$lib/lines.js';
	import { locationState, STATE_LABEL } from './locationList.js';
	import ArrowBack from '~icons/material-symbols/arrow-back';
	import Spinner from '~icons/material-symbols/progress-activity';

	let {
		locationId,
		tab,
		ontab
	}: {
		locationId: string;
		tab: string;
		ontab: (id: string) => void;
	} = $props();

	const tabs = [
		{ id: 'settings', label: 'Settings' },
		{ id: 'methods', label: 'Methods' },
		{ id: 'test-ips', label: 'Test IPs' },
		{ id: 'iperf', label: 'iperf endpoints' },
		{ id: 'speedtest', label: 'Test files' },
		{ id: 'enrollment', label: 'Enrollment' }
	];

	const kindItems = [
		{ label: 'Local (built-in node)', value: 'local' },
		{ label: 'Remote (enrolled agent)', value: 'remote' }
	];
	const familyItems = [
		{ label: 'IPv4', value: 'v4' },
		{ label: 'IPv6', value: 'v6' }
	];
	const families = methodsByFamily();

	const ipColumns = [
		{ label: 'Name', value: (ip: TestIp) => ip.label ?? '—' },
		{ label: 'IP address', value: (ip: TestIp) => ip.address, mono: true },
		{
			label: 'Type',
			value: (ip: TestIp) => (ip.family === 'v4' ? 'IPv4' : 'IPv6'),
			muted: true
		}
	];
	const iperfColumns = [
		{ label: 'Name', value: (ep: IperfEndpoint) => ep.label },
		{ label: 'Host', value: (ep: IperfEndpoint) => ep.host, mono: true },
		{
			label: 'Port',
			value: (ep: IperfEndpoint) => String(ep.port),
			mono: true,
			muted: true
		}
	];
	const fileColumns = [
		{ label: 'Label', value: (file: TestFile) => file.label },
		{ label: 'Declared size', value: (file: TestFile) => file.declared_size, muted: true },
		{ label: 'Source on node', value: (file: TestFile) => file.source_ref, mono: true }
	];

	let phase = $state<'loading' | 'ready' | 'error'>('loading');
	// A refresh of the editor on screen failed: it stays, with a retry.
	let refreshFailed = $state(false);
	let retrying = $state(false);
	// Bumped per failed refresh: the alert is re-inserted, so a repeat is announced.
	let refreshFailures = $state(0);
	let heading = $state<HTMLElement>();
	let detail = $state<LocationDetail | null>(null);
	let form = $state({
		name: '',
		geo_label: '',
		asn: '',
		map_query: '',
		facility: '',
		facility_url: '',
		data_plane_origin: '',
		kind: 'local'
	});
	let offered = $state<Record<OfferedMethod, boolean>>(blankOffered());
	let saving = $state(false);
	let formError = $state('');
	// A save refused for an invalid ASN: the Methods tab, where the ASN field is
	// not shown, says why until the ASN is fixed or saving is tried again.
	let asnBlocked = $state(false);
	let active = $state('');
	// Enrollment tickets minted this session, per location, and the mint in
	// flight for each: the tab remounts on every visit, and each POST stores
	// another live token on central, so a remounted tab joins the pending mint.
	const tickets = $state<Record<string, EnrollmentTicket | undefined>>({});
	const minting = new Map<string, Promise<string | null>>();

	function mint(id: string): Promise<string | null> {
		let pending = minting.get(id);
		if (!pending) {
			// Drop the shown ticket at once: after a revoke it is already purged,
			// and a tab remounted mid-mint sees no ticket, so it joins this mint.
			tickets[id] = undefined;
			pending = createEnrollment(id)
				.then((result) => {
					if (!result.ok) return result.message;
					tickets[id] = result.data;
					return null;
				})
				.finally(() => minting.delete(id));
			minting.set(id, pending);
		}
		return pending;
	}

	// The active tab lives in the URL: local clicks push out through `ontab`
	// (the route rewrites ?tab= with replaceState), deep links and reloads flow
	// back in through the `tab` prop.
	$effect(() => {
		active = tab;
	});
	$effect(() => {
		if (active !== '' && active !== tab) ontab(active);
	});

	$effect(() => {
		const id = locationId;
		// load() reads `detail`; only a route id change may re-run this.
		untrack(() => void load(id));
	});

	function blankOffered(): Record<OfferedMethod, boolean> {
		return Object.fromEntries(OFFERED_METHODS.map((method) => [method, false])) as Record<
			OfferedMethod,
			boolean
		>;
	}

	let loadGeneration = 0;
	// The server's values from the last load. A refresh after a sub-resource
	// change or the enrollment poll updates only the fields still equal to
	// them, so unsaved Settings and Methods edits survive it.
	let loaded = { form: { ...form }, offered: blankOffered() };

	async function load(id: string, replace = false) {
		const generation = ++loadGeneration;
		// Only a first load (or a new route id) swaps the editor for the loading
		// row; a refresh after a save or poll updates it in place, so focus,
		// scroll and the mounted tab survive.
		const fresh = detail?.id !== id;
		if (fresh) phase = 'loading';
		const result = await getLocation(id);
		// A newer load started (route id changed mid-fetch): drop the stale result
		// so it cannot overwrite the editor now bound to the new id.
		if (generation !== loadGeneration) return;
		if (!result.ok) {
			// Only a first load has no editor to keep; a failed refresh keeps it
			// (and its unsaved edits) and offers a retry.
			if (fresh) phase = 'error';
			else {
				refreshFailed = true;
				refreshFailures++;
			}
			return;
		}
		refreshFailed = false;
		detail = result.data;
		const next = {
			name: result.data.name,
			geo_label: result.data.geo_label,
			asn: result.data.asn == null ? '' : String(result.data.asn),
			map_query: result.data.map_query ?? '',
			facility: result.data.facility ?? '',
			facility_url: result.data.facility_url ?? '',
			data_plane_origin: result.data.data_plane_origin ?? '',
			kind: result.data.kind
		};
		const nextOffered = blankOffered();
		for (const method of result.data.offered_methods) nextOffered[method] = true;
		const keep = !fresh && !replace;
		for (const key of Object.keys(next) as (keyof typeof next)[]) {
			if (!keep || form[key] === loaded.form[key]) form[key] = next[key];
		}
		for (const method of OFFERED_METHODS) {
			if (!keep || offered[method] === loaded.offered[method]) offered[method] = nextOffered[method];
		}
		loaded = { form: next, offered: nextOffered };
		phase = 'ready';
	}

	const reload = () => void load(locationId);

	// The retry button leaves with the alert: put focus on the heading, not <body>.
	async function retryRefresh() {
		retrying = true;
		await load(locationId);
		retrying = false;
		if (!refreshFailed && (document.activeElement ?? document.body) === document.body) heading?.focus();
	}

	const asnFieldError = $derived(asnError(form.asn));

	// The data-plane certificate the node's agent obtains itself.
	const certificateHint = $derived.by(() => {
		const certificate = detail?.certificate;
		const utc = (seconds: number) =>
			`${new Date(seconds * 1000).toISOString().slice(0, 16).replace('T', ' ')} UTC`;
		const parts: string[] = [];
		if (certificate?.issued_at != null && certificate.expires_at != null) {
			parts.push(
				`HTTPS certificate issued ${utc(certificate.issued_at)}, expires ${utc(certificate.expires_at)}.`
			);
		}
		if (certificate?.last_error) parts.push(`Last certificate error: ${certificate.last_error}`);
		return (
			parts.join(' ') ||
			'The node gets its HTTPS certificate itself; it must be reachable from the internet on TCP 443 at this origin.'
		);
	});

	function buildInput(): LocationInput {
		return {
			name: form.name,
			geo_label: form.geo_label,
			map_query: nullIfBlank(form.map_query),
			facility: nullIfBlank(form.facility),
			facility_url: nullIfBlank(form.facility_url),
			data_plane_origin: form.kind === 'remote' ? nullIfBlank(form.data_plane_origin) : null,
			kind: form.kind === 'remote' ? 'remote' : 'local',
			asn: asnValue(form.asn),
			offered_methods: OFFERED_METHODS.filter((method) => offered[method])
		};
	}

	async function save(event: SubmitEvent, message: string) {
		event.preventDefault();
		if (saving) return;
		asnBlocked = asnFieldError !== null;
		if (asnBlocked) return;
		saving = true;
		formError = '';
		const result = await updateLocation(locationId, buildInput());
		if (result.ok) {
			// The refresh overwrites every field: stay busy (read-only) until it
			// lands, so nothing typed after the toast is lost to it.
			await load(locationId, true);
			toast.success(message);
		} else {
			formError = result.message;
			toast.error(result.message);
		}
		saving = false;
	}

	// While saving, the controls refuse changes without `disabled`, which would
	// drop a focused control's focus to <body>. Capturing at the form keeps the
	// Ark machines from seeing the event; cancelling the click undoes a checkbox
	// toggle and keeps the select closed.
	function holdWhileSaving(event: Event) {
		if (!saving) return;
		event.stopPropagation();
		if (event.type === 'click') event.preventDefault();
	}
</script>

<a href="/admin" class={backLink}>
	<ArrowBack aria-hidden="true" />
	Back to locations
</a>

{#if phase === 'loading'}
	<p class={loadingRow}>
		<Spinner class={spinIcon} aria-hidden="true" />
		Loading location…
	</p>
{:else if phase === 'error' || !detail}
	<p class={errorText} role="alert">This location could not be loaded.</p>
{:else}
	{@const state = locationState(detail)}
	<header class={pageHead} data-line style={lineStyle(detail.id)}>
		<span class={headRoundel} aria-hidden="true">{lineCode(detail.name)}</span>
		<div>
			<h1 class={pageTitle} tabindex="-1" bind:this={heading}>{detail.name}</h1>
			<p class={pageSub}>
				<StatusBadge
					tone={state === 'online' ? 'success' : state === 'offline' ? 'danger' : 'neutral'}
					size="lg">{STATE_LABEL[state]}</StatusBadge
				>
				<span>{detail.kind === 'remote' ? 'Remote node' : 'Local node'}</span>
				{#if detail.asn}<span class={tdMono}>AS{detail.asn}</span>{/if}
			</p>
		</div>
	</header>

	{#if refreshFailed}
		<!-- The Enrollment tab's retry pattern; the margin keeps the button's focus
		     ring off the tab rail. -->
		<div class={familyGroup}>
			{#key refreshFailures}
				<p class={errorText} role="alert">This location could not be refreshed. It may be out of date.</p>
			{/key}
			<Button variant="secondary" size="sm" onclick={retryRefresh} class={retryBtn} loading={retrying}
				>Try again</Button
			>
		</div>
	{/if}

	<Tabs {tabs} bind:active label="Location sections" contentClass={panelCard}>
		{#snippet panel(item)}
		<!-- Branch on the panel's own tab, not `active`: the outgoing panel stays
		     mounted for one pass after a switch and must not render the new tab. -->
		{#if !detail}
			<!-- unreachable: the outer branch guarantees detail -->
		{:else if item.id === 'settings'}
			<form
				class={formStack}
				onsubmit={(event) => save(event, 'Location saved.')}
				onclickcapture={holdWhileSaving}
				onkeydowncapture={holdWhileSaving}
				novalidate
				aria-busy={saving || undefined}
			>
				<Field label="Display name" for="loc-name">
					<Input
						id="loc-name"
						bind:value={form.name}
						required
						readonly={saving}
						aria-disabled={saving || undefined}
					/>
				</Field>
				<Field label="Geographic label" for="loc-geo">
					<Input
						id="loc-geo"
						bind:value={form.geo_label}
						placeholder="Frankfurt, DE"
						readonly={saving}
						aria-disabled={saving || undefined}
					/>
				</Field>
				<Field label="Node kind" for="loc-kind">
					<!-- Select takes no aria-disabled; the group exposes it to the trigger. -->
					<div class="locked" role="group" aria-disabled={saving || undefined}>
						<Select
							id="loc-kind"
							items={kindItems}
							bind:value={form.kind}
							portaled={false}
						/>
					</div>
				</Field>
				<Field label="ASN (optional)" for="loc-asn" error={asnFieldError ?? undefined}>
					<Input
						id="loc-asn"
						bind:value={form.asn}
						mono
						invalid={asnFieldError !== null}
						placeholder="64501"
						readonly={saving}
						aria-disabled={saving || undefined}
					/>
				</Field>
				<div class={formRow2}>
					<Field label="Facility (optional)" for="loc-facility">
						<Input
							id="loc-facility"
							bind:value={form.facility}
							readonly={saving}
							aria-disabled={saving || undefined}
						/>
					</Field>
					<Field label="Facility link (optional)" for="loc-facility-url">
						<Input
							id="loc-facility-url"
							bind:value={form.facility_url}
							readonly={saving}
							aria-disabled={saving || undefined}
						/>
					</Field>
				</div>
				<Field label="Map query (optional)" for="loc-map">
					<Input
						id="loc-map"
						bind:value={form.map_query}
						placeholder="50.11,8.68"
						readonly={saving}
						aria-disabled={saving || undefined}
					/>
				</Field>
				{#if form.kind === 'remote'}
					<Field label="Data-plane origin (optional)" for="loc-data-plane" hint={certificateHint}>
						<Input
							id="loc-data-plane"
							bind:value={form.data_plane_origin}
							placeholder="https://remote.example.net"
							readonly={saving}
							aria-disabled={saving || undefined}
						/>
					</Field>
				{/if}

				{#if formError}
					<p class={errorText} role="alert">{formError}</p>
				{/if}

				<div class={saveRow}>
					<!-- Busy via aria-busy/aria-disabled (the recipe's dimmed look), not
					     `disabled`, which blurs the focused button. The inline style keeps
					     it hit-testable, so a second click lands on it (swallowed by
					     `saving`) instead of falling through and moving focus away. -->
					<Button
						type="submit"
						style="pointer-events: auto"
						aria-busy={saving || undefined}
						aria-disabled={saving || undefined}
					>
						{#if saving}<Spinner class={spinIcon} aria-hidden="true" />{/if}
						Save location
					</Button>
				</div>
			</form>
		{:else if item.id === 'methods'}
			<form
				onsubmit={(event) => save(event, 'Methods saved.')}
				onclickcapture={holdWhileSaving}
				novalidate
				aria-busy={saving || undefined}
			>
				<h3 class={panelTitle}>Offered methods</h3>
				<p class={methodsIntro}>Visitors can run only the methods offered at this location.</p>
				<div
					class="{familyGroup} locked"
					role="group"
					aria-labelledby="methods-v4"
					aria-disabled={saving || undefined}
				>
					<h4 class={familyHead} id="methods-v4">IPv4 methods</h4>
					<div class={methodsGrid}>
						{#each families.v4 as method (method)}
							<CheckboxCard
								label={method}
								bind:checked={offered[method]}
								name="methods"
								value={method}
							/>
						{/each}
					</div>
				</div>
				<div
					class="{familyGroup} locked"
					role="group"
					aria-labelledby="methods-v6"
					aria-disabled={saving || undefined}
				>
					<h4 class={familyHead} id="methods-v6">IPv6 methods</h4>
					<div class={methodsGrid}>
						{#each families.v6 as method (method)}
							<CheckboxCard
								label={method}
								bind:checked={offered[method]}
								name="methods"
								value={method}
							/>
						{/each}
					</div>
				</div>

				{#if asnBlocked && asnFieldError}
					<!-- The Methods save sends the Settings fields too. -->
					<p class={errorText} role="alert">
						Not saved: fix the ASN on the Settings tab. {asnFieldError}
					</p>
				{/if}
				{#if formError}
					<p class={errorText} role="alert">{formError}</p>
				{/if}

				<div class={methodsSave}>
					<Button
						type="submit"
						style="pointer-events: auto"
						aria-busy={saving || undefined}
						aria-disabled={saving || undefined}
					>
						{#if saving}<Spinner class={spinIcon} aria-hidden="true" />{/if}
						Save methods
					</Button>
				</div>
			</form>
		{:else if item.id === 'test-ips'}
			<CrudSection
				title="Test IPs"
				description="Addresses visitors can test toward. They're listed with this location on the public page."
				addLabel="Add test IP"
				scope={locationId}
				itemLabel="test IP"
				items={detail.test_ips}
				columns={ipColumns}
				rowName={(ip) => ip.label ?? ip.address}
				savedMessage="Test IP saved."
				deletedMessage="Test IP deleted."
				fields={[
					{ key: 'label', label: 'Name', optional: true },
					{ key: 'address', label: 'IP address', mono: true, placeholder: '203.0.113.10' },
					{ key: 'family', label: 'Type', type: 'select', options: familyItems }
				]}
				create={(draft) => createTestIp(locationId, draft)}
				update={updateTestIp}
				remove={deleteTestIp}
				onchanged={reload}
			/>
		{:else if item.id === 'iperf'}
			<CrudSection
				title="iperf endpoints"
				description="Servers visitors can point their own iperf3 client at. Looking Glass shows the commands; it never runs them."
				addLabel="Add iperf endpoint"
				scope={locationId}
				itemLabel="iperf endpoint"
				items={detail.iperf}
				columns={iperfColumns}
				rowName={(ep) => ep.label}
				savedMessage="iperf endpoint saved."
				deletedMessage="iperf endpoint deleted."
				fields={[
					{ key: 'label', label: 'Name' },
					{ key: 'host', label: 'Host', mono: true, placeholder: 'iperf.example.net' },
					{ key: 'port', label: 'Port', type: 'number', mono: true, placeholder: '5201' },
					{
						key: 'cmd_outgoing',
						label: 'Standard command',
						mono: true,
						placeholder: 'iperf3 -c host'
					},
					{
						key: 'cmd_incoming',
						label: 'Reverse command',
						mono: true,
						placeholder: 'iperf3 -c host -R'
					}
				]}
				create={(draft) => createIperf(locationId, draft)}
				update={updateIperf}
				remove={deleteIperf}
				onchanged={reload}
			/>
		{:else if item.id === 'speedtest'}
			<CrudSection
				title="Test files"
				description="Files the browser speed test streams and visitors can download."
				addLabel="Add test file"
				scope={locationId}
				itemLabel="test file"
				items={detail.files}
				columns={fileColumns}
				rowName={(file) => file.label}
				savedMessage="Test file saved."
				deletedMessage="Test file deleted."
				fields={[
					{ key: 'label', label: 'Label' },
					{ key: 'declared_size', label: 'Declared size', placeholder: '100 MB' },
					{ key: 'source_ref', label: 'Source on node', mono: true, placeholder: '/files/100mb.bin' }
				]}
				create={(draft) => createTestFile(locationId, draft)}
				update={updateTestFile}
				remove={deleteTestFile}
				onchanged={reload}
			/>
		{:else if item.id === 'enrollment'}
			<EnrollmentTab
				{locationId}
				locationName={detail.name}
				kind={detail.kind}
				online={detail.status === 'online'}
				onchanged={reload}
				ticket={tickets[locationId]}
				{mint}
			/>
		{/if}
		{/snippet}
	</Tabs>
{/if}

<style>
	/* While saving, a group's aria-disabled gives the locked Ark controls their
	   recipes' disabled look. Panda does not extract css() from .svelte files. */
	.locked[aria-disabled='true'] :global([data-part='trigger']) {
		opacity: 0.55;
		cursor: not-allowed;
		background: var(--colors-sunk);
	}
	.locked[aria-disabled='true'] :global([data-scope='checkbox'][data-part='root']) {
		opacity: 0.55;
		cursor: not-allowed;
	}
</style>
