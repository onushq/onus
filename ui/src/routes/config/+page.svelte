<script lang="ts">
	import { api } from '#lib/api.ts';
	import Badge from '#lib/components/Badge.svelte';
	import Card from '#lib/components/Card.svelte';
	import Copy from '#lib/components/Copy.svelte';
	import ErrorBox from '#lib/components/ErrorBox.svelte';
	import Loading from '#lib/components/Loading.svelte';
	import PageHead from '#lib/components/PageHead.svelte';
	import { download } from '#lib/format.ts';
	import { Task } from '#lib/task.svelte.ts';
	import { onMount } from 'svelte';

	const config = new Task<{ path: string; exists: boolean; text: string | null; parsed: Record<string, unknown> | null; error: string | null }>();
	const init = new Task<{ text: string }>();

	onMount(() => {
		config.run(() => api('config'));
		init.run(() => api('config.init'));
	});

	const count = (v: unknown) => (Array.isArray(v) ? v.length : v && typeof v === 'object' ? Object.keys(v).length : 0);
	const sections = [
		['components', 'Components and their owners and labels'],
		['contracts', 'Shared contracts and their invariants'],
		['rules', 'Boundary rules'],
		['labels', 'Sensitivity labels'],
		['extractors', 'Events, external services, SDK packs, Prisma'],
		['tests', 'Where the tests are'],
		['lanes', 'The lane policy'],
		['environment', 'The environment for runs']
	] as const;
</script>

<PageHead title="onus.yaml" guide="configuration">
	What Onus cannot infer: which components are sensitive, which boundaries must hold, how events are published.
	Unknown keys are an error, so a typo never silently does nothing.
</PageHead>

{#if config.error}
	<ErrorBox error={config.error} />
{:else if !config.value}
	<Loading />
{:else}
	{@const c = config.value}
	<div class="stack">
		{#if c.error}<ErrorBox error={c.error} title="onus.yaml does not load" />{/if}
		<div class="grid-2">
			<Card title={c.exists ? 'Declared' : 'No onus.yaml yet'} subtitle={c.path}>
				{#if c.parsed}
					<ul class="plain">
						{#each sections as [key, label] (key)}
							<li>
								<Badge tone={c.parsed[key] && count(c.parsed[key]) ? 'add' : 'faint'}>{count(c.parsed[key]) || '–'}</Badge>
								<code>{key}</code> <span class="muted small">{label}</span>
							</li>
						{/each}
					</ul>
				{:else if !c.exists}
					<p class="muted">Onus works without one. The suggestion beside this is inferred from your workspaces and CODEOWNERS; save it as <code>onus.yaml</code> at the repository root (or run <code>onus init</code>).</p>
				{/if}
			</Card>
			<Card title="Suggested by onus init" subtitle="Inferred from workspaces and CODEOWNERS; labels are suggested as comments.">
				{#snippet actions()}
					{#if init.value}
						<Copy text={init.value.text} />
						<button class="small" onclick={() => download('onus.yaml', init.value?.text ?? '', 'text/yaml')}>Download</button>
					{/if}
				{/snippet}
				{#if init.value}<pre class="yaml">{init.value.text}</pre>{:else if init.error}<ErrorBox error={init.error} />{:else}<Loading />{/if}
			</Card>
		</div>
		{#if c.text}
			<Card title="As written">
				{#snippet actions()}<Copy text={c.text ?? ''} />{/snippet}
				<pre class="yaml">{c.text}</pre>
			</Card>
		{/if}
	</div>
{/if}

<style>
	.plain {
		list-style: none;
		margin: 0;
		padding: 0;
		display: grid;
		gap: 8px;
	}
	.yaml {
		max-height: 60vh;
	}
</style>
