<script lang="ts">
	import { page } from '$app/state';
	import { Button } from '$lib/components/ui/button/index.js';
	import * as Table from '$lib/components/ui/table/index.js';
	import { api } from '#lib/api.ts';
	import Badge from '#lib/components/Badge.svelte';
	import Card from '#lib/components/Card.svelte';
	import Copy from '#lib/components/Copy.svelte';
	import ErrorBox from '#lib/components/ErrorBox.svelte';
	import Loading from '#lib/components/Loading.svelte';
	import PageHead from '#lib/components/PageHead.svelte';
	import Stat from '#lib/components/Stat.svelte';
	import { bytes, date, download, duration, short } from '#lib/format.ts';
	import { Task } from '#lib/task.svelte.ts';
	import type { Artifact, Manifest } from '#lib/types.ts';
	import Download from '@lucide/svelte/icons/download';

	const id = $derived(page.params.id ?? '');
	const run = new Task<{ id: string; manifest: Manifest }>();
	const artifact = new Task<{ artifact: Artifact; text: string }>();
	let open = $state<string | null>(null);

	$effect(() => {
		const i = id;
		run.run(() => api('evidence.show', { id: i }));
	});
	$effect(() => {
		if (run.value && open === null) open = '-';
	});
	$effect(() => {
		const path = open;
		const i = id;
		if (path !== null) artifact.run(() => api('evidence.artifact', { id: i, path }));
	});
</script>

<PageHead title="Run {short(id, 12)}" guide="environments">A manifest named by the sha256 of its own contents; each artifact under its own sha256.</PageHead>

{#if run.error}
	<ErrorBox error={run.error} />
{:else if !run.value}
	<Loading />
{:else}
	{@const m = run.value.manifest}
	<div class="grid grid-cols-2 gap-3 md:grid-cols-4">
		<Stat label="Exit code" value={m.exitCode} tone={m.exitCode === 0 ? 'add' : 'del'} />
		<Stat label="Tests" value={m.tests?.tests ?? '–'} hint={m.tests ? `${m.tests.failures} failures, ${m.tests.errors} errors, ${m.tests.skipped} skipped` : 'no JUnit results'} />
		<Stat label="Duration" value={duration(m.finishedAt - m.startedAt)} hint={date(m.startedAt)} />
		<Stat label="Artifacts" value={m.artifacts.length} />
	</div>
	<div class="grid gap-4 xl:grid-cols-2">
		<Card title="What ran">
			<dl class="grid grid-cols-[max-content_minmax(0,1fr)] gap-x-4 gap-y-2 text-sm">
				<dt class="text-muted-foreground">Command</dt><dd class="font-mono text-xs break-all">{m.command}</dd>
				<dt class="text-muted-foreground">Commit</dt><dd class="font-mono text-xs break-all">{m.commit}</dd>
				<dt class="text-muted-foreground">Run id</dt><dd class="flex items-center gap-2"><span class="font-mono text-xs break-all">{run.value.id}</span><Copy text={run.value.id} /></dd>
			</dl>
		</Card>
		<Card title="Where it ran">
			<dl class="grid grid-cols-[max-content_minmax(0,1fr)] gap-x-4 gap-y-2 text-sm">
				<dt class="text-muted-foreground">Environment</dt><dd>{m.environment.name}</dd>
				<dt class="text-muted-foreground">Image</dt><dd class="font-mono text-xs">{m.environment.warmImage ?? m.environment.image}</dd>
				<dt class="text-muted-foreground">Setup</dt><dd class="font-mono text-xs">{m.environment.setup ?? '–'}</dd>
				<dt class="text-muted-foreground">Seed</dt><dd class="font-mono text-xs break-all">{m.environment.seed ?? '–'}</dd>
				<dt class="text-muted-foreground">Network</dt><dd>{m.environment.hosts.length ? m.environment.hosts.join(', ') : 'none'}</dd>
				<dt class="text-muted-foreground">Secrets</dt><dd class="font-mono text-xs">{m.environment.secrets.join(', ') || 'none'}</dd>
			</dl>
		</Card>
	</div>
	<div class="grid gap-4 xl:grid-cols-[minmax(0,1fr)_minmax(0,2fr)]">
		<Card title="Artifacts" pad={false} class="self-start">
			<Table.Root>
				<Table.Body>
					{#each m.artifacts as a (a.path)}
						<Table.Row class="cursor-pointer {open === a.path ? 'bg-muted' : ''}" onclick={() => (open = a.path)}>
							<Table.Cell class="pl-4"><Badge tone={a.kind === 'log' ? 'neutral' : a.kind === 'junit' ? 'info' : 'faint'}>{a.kind}</Badge></Table.Cell>
							<Table.Cell class="font-mono text-xs">{a.path === '-' ? 'output log' : a.path}<div class="text-muted-foreground">{short(a.sha256, 16)}</div></Table.Cell>
							<Table.Cell class="pr-4 text-right text-xs text-muted-foreground">{bytes(a.size)}</Table.Cell>
						</Table.Row>
					{/each}
				</Table.Body>
			</Table.Root>
		</Card>
		{#if open !== null}
			<Card title={open === '-' ? 'Output log' : open}>
				{#snippet actions()}
					{#if artifact.value}
						<Button variant="outline" size="sm" onclick={() => download(open === '-' ? 'output.log' : ((open ?? 'artifact').split('/').pop() ?? 'artifact'), artifact.value?.text ?? '', 'text/plain')}><Download />Download</Button>
					{/if}
				{/snippet}
				{#if artifact.running}<Loading />{:else if artifact.error}<ErrorBox error={artifact.error} />{:else if artifact.value}
					{#if artifact.value.artifact.kind === 'trace'}<p class="mb-2 text-xs text-muted-foreground">Add these calls to the map with <code>onus map . --traces {open}</code> after downloading it.</p>{/if}
					<pre class="max-h-[60vh] whitespace-pre-wrap">{artifact.value.text || '(empty)'}</pre>
				{/if}
			</Card>
		{/if}
	</div>
{/if}
