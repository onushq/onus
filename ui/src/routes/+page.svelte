<script lang="ts">
	import { api } from '#lib/api.ts';
	import { app } from '#lib/app.svelte.ts';
	import Card from '#lib/components/Card.svelte';
	import Copy from '#lib/components/Copy.svelte';
	import Graph from '#lib/components/Graph.svelte';
	import Loading from '#lib/components/Loading.svelte';
	import PageHead from '#lib/components/PageHead.svelte';
	import Stat from '#lib/components/Stat.svelte';
	import { ago, short } from '#lib/format.ts';
	import { Task } from '#lib/task.svelte.ts';
	import type { Graph as G, KeysStatus } from '#lib/types.ts';
	import { onMount } from 'svelte';

	const graph = new Task<G>();
	const keys = new Task<KeysStatus>();

	onMount(() => {
		graph.run(() => api<G>('map.graph'));
		keys.run(() => api<KeysStatus>('keys.status'));
		app.loadRefs();
	});

	const s = $derived(app.status);
	const setup = $derived(
		s
			? [
					{ done: s.config.exists, label: 'onus.yaml declares components, labels and rules', href: '/config', how: 'onus init' },
					{ done: s.config.lanes, label: 'A lane policy decides what merges itself', href: '/lanes', how: 'lanes: in onus.yaml' },
					{ done: s.config.environment, label: 'An environment runs tests and records evidence', href: '/environments', how: 'environment: in onus.yaml' },
					{ done: !!keys.value?.public, label: 'A root key signs task tokens', href: '/scopes', how: 'onus token keygen' }
				]
			: []
	);
</script>

<PageHead title="Overview">
	What Onus knows about this repository, and where to go next.
</PageHead>

{#if !s}
	{#if app.error}<p class="muted">{app.error}</p>{:else}<Loading label="Building the map of the repository…" />{/if}
{:else}
	<div class="stack">
		<div class="stats">
			<Stat label="Components" value={s.map.components} href="/map" />
			<Stat label="Files" value={s.map.files} href="/map?tab=files" />
			<Stat label="Symbols" value={s.map.symbols} href="/map" />
			<Stat label="Relationships" value={s.map.edges} />
			<Stat label="Tests" value={s.map.tests} />
			<Stat
				label="Uncommitted files"
				value={s.dirty.length}
				href="/changes"
				hint={s.dirty.length ? 'Check what they mean' : 'Working tree is clean'}
				tone={s.dirty.length ? 'signal' : undefined}
			/>
		</div>

		<div class="grid-2">
			<Card title="Components" subtitle="Who depends on whom. Click a component to open it.">
				{#snippet actions()}<a class="button small" href="/map">Open the map</a>{/snippet}
				{#if graph.value}
					{#if graph.value.components.length}
						<Graph graph={graph.value} limit={18} />
					{:else}
						<p class="muted">The map has no components yet. Onus reads TypeScript and JavaScript; other languages come in through plugins (see the guide).</p>
					{/if}
				{:else if graph.error}
					<p class="muted">{graph.error}</p>
				{:else}
					<Loading />
				{/if}
			</Card>

			<div class="stack">
				<Card title="Set up">
					<ul class="setup">
						{#each setup as item (item.label)}
							<li class:done={item.done}>
								<span class="tick">{item.done ? '✓' : '○'}</span>
								<a href={item.href}>{item.label}</a>
								{#if !item.done}<code class="small">{item.how}</code>{/if}
							</li>
						{/each}
					</ul>
				</Card>

				<Card title="Recent commits" subtitle="Report what any commit meant.">
					<ul class="commits">
						{#each (app.refs?.commits ?? []).slice(0, 8) as c (c.sha)}
							<li>
								<a class="mono sha" href="/changes?base={c.sha}~1&head={c.sha}">{short(c.sha, 7)}</a>
								<span class="subject" title={c.subject}>{c.subject}</span>
								<span class="faint small nowrap">{ago(c.at)}</span>
							</li>
						{:else}
							<li class="muted">No commits yet.</li>
						{/each}
					</ul>
				</Card>

				<Card title="Connect an agent" subtitle="Agents get the same map over MCP: check their work, see what a change would break.">
					<div class="row">
						<code class="grow">claude mcp add onus -- onus mcp</code>
						<Copy text="claude mcp add onus -- onus mcp" />
					</div>
					<p class="small muted" style="margin-top: 8px">Any MCP client works: <code>onus mcp</code> on stdio, or <code>onus mcp --http 127.0.0.1:8765</code>. See <a href="/guide/agents">agents</a>.</p>
				</Card>
			</div>
		</div>
	</div>
{/if}

<style>
	.setup,
	.commits {
		list-style: none;
		margin: 0;
		padding: 0;
		display: grid;
		gap: var(--space-2);
	}
	.setup li {
		display: flex;
		gap: var(--space-2);
		align-items: baseline;
		flex-wrap: wrap;
	}
	.tick {
		width: 14px;
		color: var(--ink-faint);
	}
	.done .tick {
		color: var(--add);
	}
	.done a {
		color: var(--ink-muted);
	}
	.commits li {
		display: grid;
		grid-template-columns: auto 1fr auto;
		gap: var(--space-3);
		align-items: baseline;
	}
	.subject {
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	.grow {
		flex: 1;
		padding: 6px 10px !important;
	}
</style>
