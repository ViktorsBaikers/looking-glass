<script lang="ts">
	import Download from '~icons/material-symbols/download';
	import Upload from '~icons/material-symbols/upload';
	import { Button } from '$lib/components/ui/button/index.js';
	import CopyButton from '$lib/components/ui/copy-button.svelte';
	import { lineStyle } from '$lib/lines.js';
	import { downloadUrl } from './api.js';
	import { barWidths, runSpeedTest } from './speedtest.js';
	import {
		speedSection,
		speedTitle,
		speedGrid,
		speedCard,
		cardTitleRow,
		iperfTitle,
		speedCardTitle,
		endpointBlock,
		endpointName,
		endpointHost,
		cmdGroup,
		cmdLabel,
		cmdRow,
		cmdCode,
		readouts,
		readoutLabel,
		readoutValueRow,
		readoutValue,
		readoutUnit,
		barTrack,
		barDownload,
		barUpload,
		noFilesNote,
		panelError,
		fileLinks,
		fileLink,
		fileSize
	} from './styles.js';
	import type { LocationDetail } from '$lib/admin/types.js';

	let { location }: { location: LocationDetail } = $props();

	let running = $state(false);
	let measured = $state(false);
	let failed = $state(false);
	let downloadMbps = $state(0);
	let uploadMbps = $state(0);
	let progress = $state(0);

	const hasFiles = $derived(location.files.length > 0);

	// Measurements belong to one location; switching tabs (or unmounting)
	// cancels the in-flight test and resets the readouts. `runId` fences stale
	// progress callbacks from a cancelled run out of the fresh state.
	let runId = 0;
	let abort: AbortController | null = null;

	function cancelRun() {
		runId++;
		abort?.abort();
		abort = null;
	}

	$effect(() => {
		if (location.id) {
			running = false;
			measured = false;
			failed = false;
			downloadMbps = 0;
			uploadMbps = 0;
			progress = 0;
		}
		return cancelRun;
	});

	async function start() {
		const controller = new AbortController();
		abort = controller;
		const id = ++runId;
		running = true;
		failed = false;
		try {
			const result = await runSpeedTest(
				location,
				(sample) => {
					if (id !== runId) return;
					downloadMbps = sample.downloadMbps;
					uploadMbps = sample.uploadMbps;
					progress = sample.progress;
				},
				undefined,
				controller.signal
			);
			if (id !== runId) return;
			failed = result.failed;
			measured = !result.failed;
			if (!result.failed) progress = 100;
		} finally {
			if (id === runId) {
				running = false;
				if (abort === controller) abort = null;
			}
		}
	}

	// Finished runs split the bar proportionally to the two measured speeds;
	// while running each phase fills its own half (download 0–50% of the
	// track, upload 50–100%).
	const doneSplit = $derived(
		downloadMbps + uploadMbps > 0
			? Math.round((downloadMbps / (downloadMbps + uploadMbps)) * 100)
			: 0
	);
	const doneUploadSplit = $derived(doneSplit > 0 ? 100 - doneSplit : 0);
	const liveWidths = $derived(barWidths(progress));
	const downloadWidth = $derived(measured ? doneSplit : liveWidths.download);
	const uploadWidth = $derived(measured ? doneUploadSplit : liveWidths.upload);
</script>

<section class={speedSection} aria-label="Speed Tests" data-line style={lineStyle(location.id)}>
	<h2 class={speedTitle}>Speed Tests</h2>
	<div class={speedGrid}>
		{#if location.iperf.length > 0}
			<div class={speedCard}>
				<div class={cardTitleRow}>
					<span class={iperfTitle}>iperf3 Client</span>
				</div>
				{#each location.iperf as endpoint (endpoint.id)}
					<div class={endpointBlock}>
						<div class={endpointName}>
							<span>{endpoint.label}</span>
							<span class={endpointHost}>{endpoint.host}:{endpoint.port}</span>
						</div>
						{#each [
							{ dir: 'Standard Test', cmd: endpoint.cmd_outgoing },
							{ dir: 'Reverse Test', cmd: endpoint.cmd_incoming }
						] as row (row.dir)}
							<div class={cmdGroup}>
								<span class={cmdLabel}>{row.dir}</span>
								<div class={cmdRow}>
									<code class={cmdCode}>{row.cmd}</code>
								<CopyButton
									text={row.cmd}
									label={`${endpoint.host} ${row.dir === 'Standard Test' ? 'standard' : 'reverse'} command`}
								/>
								</div>
							</div>
						{/each}
					</div>
				{/each}
			</div>
		{/if}

		<div class={speedCard}>
			<div class={cardTitleRow}>
				<span class={speedCardTitle}>Speed Test</span>
				{#if hasFiles}
					<Button variant="secondary" size="sm" disabled={running} onclick={start}>
						{running ? 'Testing…' : 'Start Speed Test'}
					</Button>
				{:else}
					<Button variant="secondary" size="sm" disabled>Start Speed Test</Button>
				{/if}
			</div>

			{#if !hasFiles}
				<p class={noFilesNote}>No test files configured</p>
			{/if}

			{#if failed}
				<p class={panelError} role="alert">Speed test failed — the test file could not be downloaded.</p>
			{/if}

			<div class={readouts} role="group" aria-label="Speed test results">
				<div>
					<div class={readoutLabel}><Download aria-hidden="true" /> Download</div>
					<div class={readoutValueRow}>
						<span class={readoutValue}>{running || measured ? downloadMbps : '—'}</span>
						<span class={readoutUnit}>Mbps</span>
					</div>
				</div>
				<div>
					<div class={readoutLabel}><Upload aria-hidden="true" /> Upload</div>
					<div class={readoutValueRow}>
						<span class={readoutValue}>{running || measured ? uploadMbps : '—'}</span>
						<span class={readoutUnit}>Mbps</span>
					</div>
				</div>
			</div>
			<div
				class={barTrack}
				role="progressbar"
				aria-label="Speed test progress"
				aria-valuemin={0}
				aria-valuemax={100}
				aria-valuenow={Math.round(progress)}
			>
				<div class={barDownload} style="width: {downloadWidth}%"></div>
				<div class={barUpload} style="width: {uploadWidth}%"></div>
			</div>

			{#if hasFiles}
				<div class={fileLinks}>
					{#each location.files as file (file.id)}
						<a
							href={downloadUrl(location, file)}
							download={file.label}
							class={fileLink}
						>
							<Download aria-hidden="true" />
							{file.label}
							<span class={fileSize}>{file.declared_size}</span>
						</a>
					{/each}
				</div>
			{/if}
		</div>
	</div>
</section>
