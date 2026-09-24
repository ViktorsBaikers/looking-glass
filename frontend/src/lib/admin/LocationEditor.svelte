<script lang="ts">
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
		spinIcon
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
		type LocationInput,
		type TestIpInput,
		type IperfInput,
		type TestFileInput
	} from './api.js';
	import { asnError, asnValue, methodsByFamily, nullIfBlank } from './editor.js';
	import {
		OFFERED_METHODS,
		type LocationDetail,
		type OfferedMethod,
		type TestIp,
		type IperfEndpoint,
		type TestFile
	} from './types.js';
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
		{ id: 'iperf', label: 'iperf' },
		{ id: 'speedtest', label: 'Speedtest' },
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
	let active = $state('');

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
		void load(locationId);
	});

	function blankOffered(): Record<OfferedMethod, boolean> {
		return Object.fromEntries(OFFERED_METHODS.map((method) => [method, false])) as Record<
			OfferedMethod,
			boolean
		>;
	}

	async function load(id: string) {
		phase = 'loading';
		const result = await getLocation(id);
		if (!result.ok) {
			phase = 'error';
			return;
		}
		detail = result.data;
		form = {
			name: result.data.name,
			geo_label: result.data.geo_label,
			asn: result.data.asn == null ? '' : String(result.data.asn),
			map_query: result.data.map_query ?? '',
			facility: result.data.facility ?? '',
			facility_url: result.data.facility_url ?? '',
			data_plane_origin: result.data.data_plane_origin ?? '',
			kind: result.data.kind
		};
		offered = blankOffered();
		for (const method of result.data.offered_methods) offered[method] = true;
		phase = 'ready';
	}

	const reload = () => void load(locationId);

	const asnFieldError = $derived(asnError(form.asn));

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
		if (saving || asnFieldError) return;
		saving = true;
		formError = '';
		const result = await updateLocation(locationId, buildInput());
		saving = false;
		if (result.ok) {
			toast.success(message);
			await load(locationId);
		} else {
			formError = result.message;
		}
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
	<header class={pageHead}>
		<h1 class={pageTitle}>{detail.name}</h1>
		<p class={pageSub}>{detail.kind} node · {detail.status}</p>
	</header>

	<Tabs {tabs} bind:active label="Location sections" />

	<div class={panelCard}>
		{#if active === 'settings'}
			<form class={formStack} onsubmit={(event) => save(event, 'Location saved.')} novalidate>
				<Field label="Display name" for="loc-name">
					<Input id="loc-name" bind:value={form.name} required />
				</Field>
				<Field label="Geographic label" for="loc-geo">
					<Input id="loc-geo" bind:value={form.geo_label} placeholder="Frankfurt, DE" />
				</Field>
				<Field label="Node kind" for="loc-kind">
					<Select id="loc-kind" items={kindItems} bind:value={form.kind} portaled={false} />
				</Field>
				<Field label="ASN (optional)" for="loc-asn" error={asnFieldError ?? undefined}>
					<Input
						id="loc-asn"
						bind:value={form.asn}
						mono
						invalid={asnFieldError !== null}
						placeholder="64501"
					/>
				</Field>
				<div class={formRow2}>
					<Field label="Facility (optional)" for="loc-facility">
						<Input id="loc-facility" bind:value={form.facility} />
					</Field>
					<Field label="Facility link (optional)" for="loc-facility-url">
						<Input id="loc-facility-url" bind:value={form.facility_url} />
					</Field>
				</div>
				<Field label="Map query (optional)" for="loc-map">
					<Input id="loc-map" bind:value={form.map_query} placeholder="50.11,8.68" />
				</Field>
				{#if form.kind === 'remote'}
					<Field label="Data-plane origin (optional)" for="loc-data-plane">
						<Input
							id="loc-data-plane"
							bind:value={form.data_plane_origin}
							placeholder="https://remote.example.net:9443"
						/>
					</Field>
				{/if}

				{#if formError}
					<p class={errorText} role="alert">{formError}</p>
				{/if}

				<div class={saveRow}>
					<Button type="submit" loading={saving}>Save location</Button>
				</div>
			</form>
		{:else if active === 'methods'}
			<form onsubmit={(event) => save(event, 'Methods saved.')} novalidate>
				<h3 class={panelTitle}>Diagnostic Methods</h3>
				<p class={methodsIntro}>
					A method that is not enabled here cannot be run at this location.
				</p>
				<div class={familyGroup}>
					<h4 class={familyHead}>IPv4 Tools</h4>
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
				<div class={familyGroup}>
					<h4 class={familyHead}>IPv6 Tools</h4>
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

				{#if formError}
					<p class={errorText} role="alert">{formError}</p>
				{/if}

				<div class={methodsSave}>
					<Button type="submit" loading={saving}>Save methods</Button>
				</div>
			</form>
		{:else if active === 'test-ips'}
			<CrudSection
				title="Test IP addresses"
				description="Configure target IP addresses for diagnostic testing from this location."
				addLabel="Add IP"
				itemLabel="IP"
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
				create={(draft) => createTestIp(locationId, draft as unknown as TestIpInput)}
				update={(id, draft) => updateTestIp(id, draft as unknown as TestIpInput)}
				remove={deleteTestIp}
				onchanged={reload}
			/>
		{:else if active === 'iperf'}
			<CrudSection
				title="Known iperf3 Endpoints"
				description="Endpoints visitors can point their own iperf3 client at."
				addLabel="Add endpoint"
				itemLabel="endpoint"
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
						key: 'cmd_incoming',
						label: 'Incoming command',
						mono: true,
						placeholder: 'iperf3 -c host -R'
					},
					{
						key: 'cmd_outgoing',
						label: 'Outgoing command',
						mono: true,
						placeholder: 'iperf3 -c host'
					}
				]}
				create={(draft) => createIperf(locationId, draft as unknown as IperfInput)}
				update={(id, draft) => updateIperf(id, draft as unknown as IperfInput)}
				remove={deleteIperf}
				onchanged={reload}
			/>
		{:else if active === 'speedtest'}
			<CrudSection
				title="Test files"
				description="Files the browser Speed test streams and the download links serve."
				addLabel="Add file"
				itemLabel="file"
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
				create={(draft) => createTestFile(locationId, draft as unknown as TestFileInput)}
				update={(id, draft) => updateTestFile(id, draft as unknown as TestFileInput)}
				remove={deleteTestFile}
				onchanged={reload}
			/>
		{:else if active === 'enrollment'}
			<EnrollmentTab
				{locationId}
				locationName={detail.name}
				kind={detail.kind}
				online={detail.status === 'online'}
				onchanged={reload}
			/>
		{/if}
	</div>
{/if}
