<script lang="ts">
	import Info from '~icons/material-symbols/info';
	import Tooltip from '$lib/components/ui/tooltip.svelte';
	import { srOnly } from '$lib/styles.js';
	import {
		metricsGrid,
		metricCard,
		metricLabelRow,
		metricInfo,
		metricValueRow,
		metricValue,
		metricUnit,
		metricCaption
	} from '$lib/public/styles.js';
	import type { Metric } from './metrics.js';

	let { metrics }: { metrics: Metric[] } = $props();
</script>

<div class={metricsGrid} role="group" aria-label="Run metrics">
	{#each metrics as metric (metric.label)}
		<div class={metricCard}>
			<div class={metricLabelRow}>
				<span>{metric.label}</span>
				<Tooltip content={metric.tooltip}>
					<span class={metricInfo}>
						<Info aria-hidden="true" />
						<span class={srOnly}>About {metric.label}</span>
					</span>
				</Tooltip>
			</div>
			<div class={metricValueRow}>
				<span class={metricValue}>{metric.value}</span>
				{#if metric.unit}<span class={metricUnit}>{metric.unit}</span>{/if}
			</div>
			{#if metric.caption}<div class={metricCaption}>{metric.caption}</div>{/if}
		</div>
	{/each}
</div>
