<script lang="ts">
	import Download from '@lucide/svelte/icons/download';
	import {
		Card,
		CardHeader,
		CardTitle,
		CardDescription,
		CardContent
	} from '$lib/components/ui/card/index.js';
	import { buttonVariants } from '$lib/components/ui/button/index.js';
	import { cn } from '$lib/utils.js';
	import CopyButton from './CopyButton.svelte';
	import { downloadUrl } from './api.js';
	import type { LocationDetail } from '$lib/admin/types.js';

	let { location }: { location: LocationDetail } = $props();

	const hasContent = $derived(location.files.length > 0 || location.iperf.length > 0);
</script>

<Card>
	<CardHeader>
		<CardTitle>Speedtest</CardTitle>
		<CardDescription>Download a test file or run iperf3 directly against this node.</CardDescription>
	</CardHeader>

	<CardContent class="space-y-5">
		{#if !hasContent}
			<p class="text-sm text-muted-foreground">No speedtest resources configured for this location.</p>
		{/if}

		{#if location.files.length > 0}
			<div class="space-y-2">
				<h3 class="text-sm font-medium">Download test files</h3>
				<div class="flex flex-wrap gap-2">
					{#each location.files as file (file.id)}
						<a
							href={downloadUrl(location, file)}
							download={file.label}
							class={cn(buttonVariants({ variant: 'outline', size: 'sm' }), 'max-w-full gap-2 whitespace-normal text-left')}
						>
							<Download class="size-4" aria-hidden="true" />
							{file.label}
							<span class="text-xs text-muted-foreground">{file.declared_size}</span>
						</a>
					{/each}
				</div>
			</div>
		{/if}

		{#if location.iperf.length > 0}
			<div class="space-y-3">
				<h3 class="text-sm font-medium">iperf3 endpoints</h3>
				{#each location.iperf as endpoint (endpoint.id)}
					<div class="space-y-2 rounded-md border border-border p-3">
						<div class="flex items-baseline justify-between gap-2">
							<span class="text-sm font-medium">{endpoint.label}</span>
							<code class="font-mono text-xs text-muted-foreground">
								{endpoint.host}:{endpoint.port}
							</code>
						</div>
						{#each [{ dir: 'Incoming', cmd: endpoint.cmd_incoming }, { dir: 'Outgoing', cmd: endpoint.cmd_outgoing }] as row (row.dir)}
							<div class="space-y-1">
								<span class="text-xs text-muted-foreground">{row.dir}</span>
								<div class="flex items-center gap-2 rounded-md bg-muted px-3 py-2">
									<code class="min-w-0 flex-1 overflow-x-auto whitespace-nowrap font-mono text-xs">
										{row.cmd}
									</code>
									<CopyButton text={row.cmd} label={`${endpoint.label} ${row.dir} command`} />
								</div>
							</div>
						{/each}
					</div>
				{/each}
			</div>
		{/if}
	</CardContent>
</Card>
