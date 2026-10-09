<script lang="ts">
	import { api } from '#lib/api.ts';
	import Badge from '#lib/components/Badge.svelte';
	import Card from '#lib/components/Card.svelte';
	import Empty from '#lib/components/Empty.svelte';
	import ErrorBox from '#lib/components/ErrorBox.svelte';
	import Loading from '#lib/components/Loading.svelte';
	import PageHead from '#lib/components/PageHead.svelte';
	import { date } from '#lib/format.ts';
	import { Task } from '#lib/task.svelte.ts';
	import type { AuditEntry } from '#lib/types.ts';
	import { onMount } from 'svelte';

	const log = new Task<{ file: string; exists: boolean; entries: AuditEntry[]; intact: boolean; error?: string }>();
	let open = $state<number | null>(null);
	let filter = $state('');

	onMount(() => log.run(() => api('audit')));

	const tone = (d: string) =>
		['granted', 'allowed', 'accepted'].includes(d) ? 'add' : ['refused', 'denied'].includes(d) ? 'del' : d === 'needs-person' ? 'signal' : 'faint';
	const shown = $derived(
		[...(log.value?.entries ?? [])]
			.reverse()
			.filter((e) => !filter || `${e.actor} ${e.action} ${e.subject} ${e.decision} ${e.reason}`.toLowerCase().includes(filter.toLowerCase()))
	);
</script>

<PageHead title="Audit log" guide="scopes">
	Every decision as one line, each carrying the hash of the line before it, so a removed or edited line breaks
	the chain.
</PageHead>

{#if log.error}
	<ErrorBox error={log.error} />
{:else if !log.value}
	<Loading />
{:else}
	<div class="stack">
		<div class="spread">
			<div class="row">
				{#if !log.value.exists}
					<Badge tone="faint">no log yet</Badge>
				{:else if log.value.intact}
					<Badge tone="add">chain intact</Badge>
				{:else}
					<Badge tone="del">chain broken</Badge>
				{/if}
				<span class="mono small muted">{log.value.file}</span>
			</div>
			<input placeholder="Filter" bind:value={filter} />
		</div>
		{#if log.value.error}<ErrorBox error={log.value.error} title="The chain does not verify" />{/if}
		<Card pad={false}>
			{#if shown.length}
				<table class="data">
					<thead><tr><th class="num">#</th><th>When</th><th>Actor</th><th>Action</th><th>Subject</th><th>Decision</th><th>Reason</th></tr></thead>
					<tbody>
						{#each shown as e (e.seq)}
							<tr onclick={() => (open = open === e.seq ? null : e.seq)} class="clickable">
								<td class="num faint">{e.seq}</td>
								<td class="small nowrap">{date(e.at)}</td>
								<td class="small">{e.actor}</td>
								<td>{e.action}</td>
								<td class="mono small break">{e.subject}</td>
								<td><Badge tone={tone(e.decision)}>{e.decision}</Badge></td>
								<td class="small">{e.reason}</td>
							</tr>
							{#if open === e.seq}
								<tr><td colspan="7"><pre>{JSON.stringify(e.details, null, 2)}</pre></td></tr>
							{/if}
						{/each}
					</tbody>
				</table>
			{:else}
				<div style="padding: 16px"><Empty title="Nothing logged">Mints, pushes, escalations, grants and denials made from here are logged to this file. The gateway keeps its own log in its state folder.</Empty></div>
			{/if}
		</Card>
	</div>
{/if}

<style>
	.clickable {
		cursor: pointer;
	}
</style>
