<script lang="ts">
	import { Button } from '$lib/components/ui/button/index.js';
	import { api } from '#lib/api.ts';
	import Badge from '#lib/components/Badge.svelte';
	import Card from '#lib/components/Card.svelte';
	import Copy from '#lib/components/Copy.svelte';
	import ErrorBox from '#lib/components/ErrorBox.svelte';
	import Loading from '#lib/components/Loading.svelte';
	import PageHead from '#lib/components/PageHead.svelte';
	import Tabs from '#lib/components/Tabs.svelte';
	import { download } from '#lib/format.ts';
	import { Task } from '#lib/task.svelte.ts';
	import CircleCheck from '@lucide/svelte/icons/circle-check';
	import Circle from '@lucide/svelte/icons/circle';
	import Download from '@lucide/svelte/icons/download';
	import { onMount } from 'svelte';

	const config = new Task<{ path: string; exists: boolean; text: string | null; parsed: Record<string, unknown> | null; error: string | null }>();
	const init = new Task<{ text: string }>();
	let tab = $state('written');

	onMount(() => {
		config.run(() => api('config'));
		init.run(() => api('config.init'));
	});

	$effect(() => {
		if (config.value && !config.value.exists) tab = 'suggested';
	});

	const count = (v: unknown) => (Array.isArray(v) ? v.length : v && typeof v === 'object' ? Object.keys(v).length : 0);
	const sections = [
		['components', 'Components, owners and labels', 'configuration'],
		['contracts', 'Shared contracts and their invariants', 'configuration'],
		['rules', 'Boundary rules', 'configuration'],
		['labels', 'Sensitivity labels', 'configuration'],
		['extractors', 'Events, external services, SDK packs, Prisma', 'configuration'],
		['tests', 'Where the tests are', 'configuration'],
		['lanes', 'The lane policy', 'lanes'],
		['environment', 'The environment for runs', 'environments']
	] as const;
</script>

<PageHead title="onus.yaml" guide="configuration">
	What Onus cannot infer: which components are sensitive, which boundaries must hold, how events are published. Unknown keys are an error, so a typo never silently does nothing.
</PageHead>

{#if config.error}
	<ErrorBox error={config.error} />
{:else if !config.value}
	<Loading />
{:else}
	{@const c = config.value}
	{#if c.error}<ErrorBox error={c.error} title="onus.yaml does not load" />{/if}
	<div class="grid gap-4 xl:grid-cols-[360px_minmax(0,1fr)]">
		<Card title={c.exists ? 'Declared' : 'No onus.yaml yet'} subtitle={c.path} class="self-start">
			{#if c.parsed}
				<ul class="grid gap-2.5">
					{#each sections as [key, label, guide] (key)}
						{@const n = count(c.parsed[key])}
						<li class="flex items-start gap-2 text-sm">
							{#if n}<CircleCheck class="mt-0.5 size-4 shrink-0 text-success" />{:else}<Circle class="mt-0.5 size-4 shrink-0 text-muted-foreground" />{/if}
							<div class="grid flex-1 gap-0.5">
								<span class="flex items-center gap-2"><code class="text-xs">{key}</code>{#if n}<Badge tone="faint">{n}</Badge>{/if}</span>
								<span class="text-xs text-muted-foreground">{label} · <a class="underline" href="/guide/{guide}">guide</a></span>
							</div>
						</li>
					{/each}
				</ul>
			{:else if !c.exists}
				<p class="text-sm text-muted-foreground">Onus works without one. The suggestion is inferred from your workspaces and CODEOWNERS; save it as <code>onus.yaml</code> at the repository root, or run <code>onus init</code>.</p>
			{/if}
		</Card>
		<Card pad={false}>
			<div class="flex flex-wrap items-center justify-between gap-3 border-b px-4 py-3">
				<Tabs bind:value={tab} tabs={[{ id: 'written', label: 'As written' }, { id: 'suggested', label: 'Suggested by onus init' }]} />
				<div class="flex gap-2">
					{#if tab === 'written' && c.text}
						<Copy text={c.text} />
					{:else if tab === 'suggested' && init.value}
						<Copy text={init.value.text} />
						<Button variant="outline" size="sm" onclick={() => download('onus.yaml', init.value?.text ?? '', 'text/yaml')}><Download />Download</Button>
					{/if}
				</div>
			</div>
			<div class="p-4">
				{#if tab === 'written'}
					{#if c.text}<pre class="max-h-[70vh]">{c.text}</pre>{:else}<p class="text-sm text-muted-foreground">There is no onus.yaml in this repository.</p>{/if}
				{:else if init.value}
					<p class="mb-3 text-xs text-muted-foreground">Inferred from workspaces and CODEOWNERS; suggested labels are commented out until you confirm them.</p>
					<pre class="max-h-[70vh]">{init.value.text}</pre>
				{:else if init.error}<ErrorBox error={init.error} />{:else}<Loading />{/if}
			</div>
		</Card>
	</div>
{/if}
