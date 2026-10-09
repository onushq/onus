<script lang="ts">
	import { Input } from '$lib/components/ui/input/index.js';
	import * as Table from '$lib/components/ui/table/index.js';
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
	import Link2 from '@lucide/svelte/icons/link-2';
	import Link2Off from '@lucide/svelte/icons/link-2-off';
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
	Every decision as one line, each carrying the hash of the line before it, so a removed or edited line breaks the chain.
</PageHead>

{#if log.error}
	<ErrorBox error={log.error} />
{:else if !log.value}
	<Loading />
{:else}
	{#if log.value.error}<ErrorBox error={log.value.error} title="The chain does not verify" />{/if}
	<Card pad={false}>
		<div class="flex flex-wrap items-center gap-3 border-b px-4 py-3">
			{#if !log.value.exists}
				<Badge tone="faint">no log yet</Badge>
			{:else if log.value.intact}
				<Badge tone="add"><Link2 />chain intact</Badge>
			{:else}
				<Badge tone="del"><Link2Off />chain broken</Badge>
			{/if}
			<span class="truncate font-mono text-xs text-muted-foreground">{log.value.file}</span>
			<Input placeholder="Filter" bind:value={filter} class="ml-auto w-56" />
		</div>
		{#if shown.length}
			<Table.Root>
				<Table.Header><Table.Row><Table.Head class="pl-4 text-right">#</Table.Head><Table.Head>When</Table.Head><Table.Head>Actor</Table.Head><Table.Head>Action</Table.Head><Table.Head>Subject</Table.Head><Table.Head>Decision</Table.Head><Table.Head class="pr-4">Reason</Table.Head></Table.Row></Table.Header>
				<Table.Body>
					{#each shown as e (e.seq)}
						<Table.Row class="cursor-pointer" onclick={() => (open = open === e.seq ? null : e.seq)}>
							<Table.Cell class="pl-4 text-right text-xs text-muted-foreground tabular-nums">{e.seq}</Table.Cell>
							<Table.Cell class="text-xs whitespace-nowrap">{date(e.at)}</Table.Cell>
							<Table.Cell class="text-xs">{e.actor}</Table.Cell>
							<Table.Cell class="font-medium">{e.action}</Table.Cell>
							<Table.Cell class="font-mono text-xs break-all">{e.subject}</Table.Cell>
							<Table.Cell><Badge tone={tone(e.decision)}>{e.decision}</Badge></Table.Cell>
							<Table.Cell class="pr-4 text-xs">{e.reason}</Table.Cell>
						</Table.Row>
						{#if open === e.seq}
							<Table.Row class="hover:bg-transparent"><Table.Cell colspan={7} class="px-4"><pre class="max-h-80">{JSON.stringify(e.details, null, 2)}</pre></Table.Cell></Table.Row>
						{/if}
					{/each}
				</Table.Body>
			</Table.Root>
		{:else}
			<div class="p-4"><Empty title="Nothing logged">Mints, escalations, grants and denials made from here are logged to this file. The gateway keeps its own log in its state folder.</Empty></div>
		{/if}
	</Card>
{/if}
