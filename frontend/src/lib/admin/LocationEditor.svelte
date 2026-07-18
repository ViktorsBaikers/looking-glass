<script lang="ts">
	import ArrowLeft from '@lucide/svelte/icons/arrow-left';
	import LoaderCircle from '@lucide/svelte/icons/loader-circle';
	import { Button } from '$lib/components/ui/button/index.js';
	import { Input } from '$lib/components/ui/input/index.js';
	import { Label } from '$lib/components/ui/label/index.js';
	import Tabs from '$lib/components/ui/tabs.svelte';
	import CrudSection from './CrudSection.svelte';
	import { toaster } from '$lib/toast.svelte.js';
	import { OFFERED_METHODS } from './types.js';
	import type { LocationDetail, OfferedMethod } from './types.js';
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

	let { locationId, onclose }: { locationId: string; onclose: () => void } = $props();

	let phase = $state<'loading' | 'ready' | 'error'>('loading');
	let detail = $state<LocationDetail | null>(null);
	let form = $state<LocationInput>(blankForm());
	let active = $state('settings');
	let savingLocation = $state(false);
	let locationError = $state('');

	const tabs = [
		{ id: 'settings', label: 'Settings' },
		{ id: 'methods', label: 'Methods' },
		{ id: 'ips', label: 'Test IPs' },
		{ id: 'iperf', label: 'iperf' },
		{ id: 'files', label: 'Files' }
	];

	function blankForm(): LocationInput {
		return {
			name: '',
			geo_label: '',
			map_query: null,
			facility: null,
			facility_url: null,
			data_plane_origin: null,
			kind: 'local',
			offered_methods: []
		};
	}

	$effect(() => {
		void load(locationId);
	});

	async function load(id: string) {
		phase = 'loading';
		const result = await getLocation(id);
		if (result.ok) {
			detail = result.data;
			form = {
				name: result.data.name,
				geo_label: result.data.geo_label,
				map_query: result.data.map_query,
				facility: result.data.facility,
				facility_url: result.data.facility_url,
				data_plane_origin: result.data.data_plane_origin,
				kind: result.data.kind,
				offered_methods: [...result.data.offered_methods]
			};
			phase = 'ready';
		} else {
			phase = 'error';
		}
	}

	function toggleMethod(method: OfferedMethod, on: boolean) {
		form.offered_methods = on
			? [...form.offered_methods, method]
			: form.offered_methods.filter((m) => m !== method);
	}

	async function saveLocation(event: SubmitEvent) {
		event.preventDefault();
		if (savingLocation) return;
		savingLocation = true;
		locationError = '';
		const result = await updateLocation(locationId, form);
		savingLocation = false;
		if (result.ok) {
			toaster.success('Location saved.');
			await load(locationId);
		} else {
			locationError = result.message;
			active = 'settings';
		}
	}

	const reload = () => {
		toaster.success('Saved.');
		void load(locationId);
	};
</script>

<div class="space-y-6">
	<Button variant="ghost" size="sm" onclick={onclose} class="-ml-2">
		<ArrowLeft class="size-4" aria-hidden="true" />
		Back to locations
	</Button>

	{#if phase === 'loading'}
		<div class="flex items-center gap-2 text-muted-foreground">
			<LoaderCircle class="size-5 animate-spin" aria-hidden="true" />
			Loading location…
		</div>
	{:else if phase === 'error'}
		<p class="text-sm text-destructive" role="alert">This location could not be loaded.</p>
	{:else if detail}
		<div>
			<h1 class="text-xl font-semibold tracking-tight">{detail.name}</h1>
			<p class="text-sm text-muted-foreground">{detail.kind} node · {detail.status}</p>
		</div>

		<Tabs {tabs} bind:active />

		{#if active === 'settings'}
			<form
				id="panel-settings"
				class="max-w-xl space-y-4"
				onsubmit={saveLocation}
				novalidate
			>
				<div class="space-y-2">
					<Label for="loc-name">Display name</Label>
					<Input id="loc-name" bind:value={form.name} disabled={savingLocation} required />
				</div>
				<div class="space-y-2">
					<Label for="loc-geo">Geographic label</Label>
					<Input
						id="loc-geo"
						bind:value={form.geo_label}
						placeholder="Frankfurt, DE"
						disabled={savingLocation}
					/>
				</div>
				<div class="space-y-2">
					<Label for="loc-kind">Node kind</Label>
					<select
						id="loc-kind"
						bind:value={form.kind}
						disabled={savingLocation}
						class="h-11 w-full rounded-md border border-input bg-background px-3 text-sm focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
					>
						<option value="local">Local (built-in node)</option>
						<option value="remote">Remote (enrolled agent)</option>
					</select>
				</div>
				<div class="grid gap-4 sm:grid-cols-2">
					<div class="space-y-2">
						<Label for="loc-facility">Facility (optional)</Label>
						<Input id="loc-facility" bind:value={form.facility} disabled={savingLocation} />
					</div>
					<div class="space-y-2">
						<Label for="loc-facility-url">Facility link (optional)</Label>
						<Input id="loc-facility-url" bind:value={form.facility_url} disabled={savingLocation} />
					</div>
				</div>
				<div class="space-y-2">
					<Label for="loc-map">Map query (optional)</Label>
					<Input
						id="loc-map"
						bind:value={form.map_query}
						placeholder="50.11,8.68"
						disabled={savingLocation}
					/>
				</div>
				{#if form.kind === 'remote'}
					<div class="space-y-2">
						<Label for="loc-data-plane">Data-plane origin (optional)</Label>
						<Input
							id="loc-data-plane"
							bind:value={form.data_plane_origin}
							placeholder="https://remote.example.net:9443"
							disabled={savingLocation}
						/>
					</div>
				{/if}

				{#if locationError}
					<p class="text-sm text-destructive" role="alert">{locationError}</p>
				{/if}

				<Button type="submit" disabled={savingLocation}>
					{savingLocation ? 'Saving…' : 'Save location'}
				</Button>
			</form>
		{:else if active === 'methods'}
			<form
				id="panel-methods"
				class="max-w-xl space-y-4"
				onsubmit={saveLocation}
				novalidate
			>
				<p class="text-sm text-muted-foreground">
					A method that is not enabled here cannot be run at this location.
				</p>
				<fieldset class="grid grid-cols-2 gap-2 sm:grid-cols-4">
					<legend class="sr-only">Offered methods</legend>
					{#each OFFERED_METHODS as method (method)}
						<label class="flex min-h-11 items-center gap-2 rounded-md border border-border px-3 py-2 text-sm">
							<input
								type="checkbox"
								checked={form.offered_methods.includes(method)}
								disabled={savingLocation}
								onchange={(e) => toggleMethod(method, e.currentTarget.checked)}
							/>
							<span class="font-mono">{method}</span>
						</label>
					{/each}
				</fieldset>
				{#if locationError}
					<p class="text-sm text-destructive" role="alert">{locationError}</p>
				{/if}
				<Button type="submit" disabled={savingLocation}>
					{savingLocation ? 'Saving…' : 'Save methods'}
				</Button>
			</form>
		{:else if active === 'ips'}
			<div id="panel-ips" role="tabpanel" aria-labelledby="tab-ips">
				<CrudSection
					title="Test IP addresses"
					addLabel="Add IP"
					items={detail.test_ips}
					fields={[
						{ key: 'address', label: 'Address', placeholder: '203.0.113.10' },
						{
							key: 'family',
							label: 'Family',
							type: 'select',
							options: [
								{ value: 'v4', label: 'IPv4' },
								{ value: 'v6', label: 'IPv6' }
							]
						},
						{ key: 'label', label: 'Label (optional)', optional: true }
					]}
					summarize={(ip) =>
						`${ip.address} · ${ip.family}${ip.label ? ' · ' + ip.label : ''}`}
					create={(d) => createTestIp(locationId, d as unknown as TestIpInput)}
					update={(id, d) => updateTestIp(id, d as unknown as TestIpInput)}
					remove={deleteTestIp}
					onchanged={reload}
				/>
			</div>
		{:else if active === 'iperf'}
			<div id="panel-iperf" role="tabpanel" aria-labelledby="tab-iperf">
				<CrudSection
					title="iperf endpoints"
					addLabel="Add endpoint"
					items={detail.iperf}
					fields={[
						{ key: 'label', label: 'Label' },
						{ key: 'host', label: 'Host' },
						{ key: 'port', label: 'Port', type: 'number', placeholder: '5201' },
						{ key: 'cmd_incoming', label: 'Incoming command' },
						{ key: 'cmd_outgoing', label: 'Outgoing command' }
					]}
					summarize={(ep) => `${ep.label} · ${ep.host}:${ep.port}`}
					create={(d) => createIperf(locationId, d as unknown as IperfInput)}
					update={(id, d) => updateIperf(id, d as unknown as IperfInput)}
					remove={deleteIperf}
					onchanged={reload}
				/>
			</div>
		{:else if active === 'files'}
			<div id="panel-files" role="tabpanel" aria-labelledby="tab-files">
				<CrudSection
					title="Downloadable test files"
					addLabel="Add file"
					items={detail.files}
					fields={[
						{ key: 'label', label: 'Label' },
						{ key: 'declared_size', label: 'Declared size', placeholder: '1 GB' },
						{ key: 'source_ref', label: 'Source on node', placeholder: '/files/1g.bin' }
					]}
					summarize={(file) => `${file.label} · ${file.declared_size}`}
					create={(d) => createTestFile(locationId, d as unknown as TestFileInput)}
					update={(id, d) => updateTestFile(id, d as unknown as TestFileInput)}
					remove={deleteTestFile}
					onchanged={reload}
				/>
			</div>
		{/if}
	{/if}
</div>
