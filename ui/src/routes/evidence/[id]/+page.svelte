<script lang="ts">
	import { page } from '$app/state';
	import { api } from '#lib/api.ts';
	import Badge from '#lib/components/Badge.svelte';
	import Card from '#lib/components/Card.svelte';
	import Copy from '#lib/components/Copy.svelte';
	import ErrorBox from '#lib/components/ErrorBox.svelte';
	import Loading from '#lib/components/Loading.svelte';
	import PageHead from '#lib/components/PageHead.svelte';
	import { bytes, date, download, duration, short } from '#lib/format.ts';
	import { Task } from '#lib/task.svelte.ts';
	import type { Artifact, Manifest } from '#lib/types.ts';

	const id = $derived(page.params.id ?? '');
	const run = new Task<{ id: string; manifest: Manifest }>();
	const artifact = new Task<{ artifact: Artifact; text: string }>();
	let open = $state<string | null>(null);

	$effect(() => {
		const i = id;
		run.run(() => api('evidence.show', { id: i }));
	});

	$effect(() => {
		const m = run.value;
		if (m && open === null) open = '-';
	});

	$effect(() => {
		const path = open;
		const i = id;
		if (path !== null) artifact.run(() => api('evidence.artifact', { id: i, path }));
	});
</script>

<PageHead title="Run {short(id, 12)}" guide="environments">
	A manifest named by the sha256 of its own contents; each artifact under its own sha256.
</PageHead>

{#if run.error}
	<ErrorBox error={run.error} />
{:else if !run.value}
	<Loading />
{:else}
	{@const m = run.value.manifest}
	<div class="stack">
		<div class="grid-2">
			<Card title="What ran">
				<dl>
					<dt>Command</dt><dd class="mono">{m.command}</dd>
					<dt>Result</dt><dd><Badge tone={m.exitCode === 0 ? 'add' : 'del'}>exit {m.exitCode}</Badge></dd>
					{#if m.tests}<dt>Tests</dt><dd>{m.tests.tests} tests, {m.tests.failures} failures, {m.tests.errors} errors, {m.tests.skipped} skipped</dd>{/if}
					<dt>Commit</dt><dd class="mono">{m.commit}</dd>
					<dt>When</dt><dd>{date(m.startedAt)} · {duration(m.finishedAt - m.startedAt)}</dd>
					<dt>Run id</dt><dd class="mono small break">{run.value.id} <Copy text={run.value.id} /></dd>
				</dl>
			</Card>
			<Card title="Where it ran">
				<dl>
					<dt>Environment</dt><dd>{m.environment.name}</dd>
					<dt>Image</dt><dd class="mono">{m.environment.image}</dd>
					{#if m.environment.warmImage}<dt>Warm image</dt><dd class="mono">{m.environment.warmImage}</dd>{/if}
					<dt>Setup</dt><dd class="mono">{m.environment.setup ?? '–'}</dd>
					<dt>Seed</dt><dd class="mono">{m.environment.seed ?? '–'}</dd>
					<dt>Network</dt><dd>{m.environment.hosts.length ? m.environment.hosts.join(', ') : 'none'}</dd>
					<dt>Secrets</dt><dd class="mono">{m.environment.secrets.join(', ') || 'none'}</dd>
				</dl>
			</Card>
		</div>

		<Card title="Artifacts" pad={false}>
			<table class="data">
				<thead><tr><th>Kind</th><th>Path</th><th class="num">Size</th><th>sha256</th></tr></thead>
				<tbody>
					{#each m.artifacts as a (a.path)}
						<tr class:sel={open === a.path}>
							<td><Badge tone={a.kind === 'log' ? 'neutral' : a.kind === 'junit' ? 'info' : 'faint'}>{a.kind}</Badge></td>
							<td><button class="ghost small mono" onclick={() => (open = a.path)}>{a.path === '-' ? 'output log' : a.path}</button></td>
							<td class="num small">{bytes(a.size)}</td>
							<td class="mono small faint">{short(a.sha256, 16)}</td>
						</tr>
					{/each}
				</tbody>
			</table>
		</Card>

		{#if open !== null}
			<Card title={open === '-' ? 'Output log' : open}>
				{#snippet actions()}
					{#if artifact.value}
						<button class="small" onclick={() => download(open === '-' ? 'output.log' : (open ?? 'artifact').split('/').pop() ?? 'artifact', artifact.value?.text ?? '', 'text/plain')}>Download</button>
					{/if}
				{/snippet}
				{#if artifact.running}<Loading />{:else if artifact.error}<ErrorBox error={artifact.error} />{:else if artifact.value}
					{#if artifact.value.artifact.kind === 'trace'}
						<p class="small muted" style="margin-bottom: 8px">Add these calls to the map with <code>onus map . --traces {open}</code> after downloading it.</p>
					{/if}
					<pre class="log">{artifact.value.text || '(empty)'}</pre>
				{/if}
			</Card>
		{/if}
	</div>
{/if}

<style>
	dl {
		display: grid;
		grid-template-columns: max-content 1fr;
		gap: 6px var(--space-4);
		margin: 0;
	}
	dt {
		color: var(--ink-faint);
	}
	dd {
		margin: 0;
		overflow-wrap: anywhere;
	}
	.log {
		max-height: 60vh;
		white-space: pre-wrap;
	}
	tr.sel td {
		background: var(--sunken);
	}
</style>
