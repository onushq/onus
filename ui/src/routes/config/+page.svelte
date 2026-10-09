<script lang="ts">
	import { beforeNavigate } from '$app/navigation';
	import { Button } from '$lib/components/ui/button/index.js';
	import { ApiError, api } from '#lib/api.ts';
	import { app } from '#lib/app.svelte.ts';
	import Badge from '#lib/components/Badge.svelte';
	import Card from '#lib/components/Card.svelte';
	import ComponentsSection from '#lib/components/config/ComponentsSection.svelte';
	import ContractsSection from '#lib/components/config/ContractsSection.svelte';
	import DetectionSection from '#lib/components/config/DetectionSection.svelte';
	import EnvironmentSection from '#lib/components/config/EnvironmentSection.svelte';
	import LabelsSection from '#lib/components/config/LabelsSection.svelte';
	import LanesSection from '#lib/components/config/LanesSection.svelte';
	import RulesSection from '#lib/components/config/RulesSection.svelte';
	import Copy from '#lib/components/Copy.svelte';
	import ErrorBox from '#lib/components/ErrorBox.svelte';
	import Loading from '#lib/components/Loading.svelte';
	import PageHead from '#lib/components/PageHead.svelte';
	import Tabs from '#lib/components/Tabs.svelte';
	import { diffLines, hunks } from '#lib/config/diff.ts';
	import { Draft } from '#lib/config/draft.svelte.ts';
	import type { ConfigFile, ConfigSchema } from '#lib/config/types.ts';
	import { Task } from '#lib/task.svelte.ts';
	import FileCode from '@lucide/svelte/icons/file-code';
	import RotateCcw from '@lucide/svelte/icons/rotate-ccw';
	import Save from '@lucide/svelte/icons/save';
	import { onMount } from 'svelte';
	import { toast } from 'svelte-sonner';

	const file = new Task<ConfigFile>();
	const schema = new Task<ConfigSchema>();
	const init = new Task<{ text: string }>();
	const saving = new Task<unknown>();
	const draft = new Draft();
	let tab = $state('form');
	let reviewing = $state(false);
	let conflict = $state(false);
	let check = $state<{ state: 'idle' | 'checking' | 'ok' | 'error'; error?: string }>({ state: 'idle' });

	async function load() {
		const f = await file.run(() => api<ConfigFile>('config'));
		if (f) {
			draft.load(f.text, f.hash);
			conflict = false;
		}
	}

	onMount(() => {
		load();
		schema.run(() => api('config.schema'));
		init.run(() => api('config.init'));
	});

	// The server checks what the YAML means (unknown keys, rules naming
	// missing components, packs that do not exist) as you type.
	let seq = 0;
	$effect(() => {
		const text = draft.text;
		const started = draft.exists || draft.dirty;
		if (draft.syntax || !started) {
			check = { state: 'idle' };
			return;
		}
		const mine = ++seq;
		check = { state: 'checking' };
		const t = setTimeout(async () => {
			try {
				const r = await api<{ ok: boolean; error?: string }>('config.validate', { text });
				if (mine === seq) check = r.ok ? { state: 'ok' } : { state: 'error', error: r.error };
			} catch (e) {
				if (mine === seq) check = { state: 'error', error: e instanceof Error ? e.message : String(e) };
			}
		}, 250);
		return () => clearTimeout(t);
	});

	const blocked = $derived(!!draft.syntax || check.state === 'error' || check.state === 'checking');
	const diff = $derived(draft.dirty ? diffLines(draft.base, draft.text) : []);
	const changedLines = $derived(diff.filter((l) => l.op !== ' ').length);

	async function save() {
		if (!draft.dirty || blocked || saving.running) return;
		try {
			const r = await api<{ config: ConfigFile; map: { ok: boolean; components?: number; files?: number; buildMs?: number; error?: string } }>('config.save', {
				text: draft.text,
				baseHash: draft.baseHash
			});
			file.value = r.config;
			draft.load(r.config.text, r.config.hash);
			reviewing = false;
			if (r.map.ok) toast.success(`Saved onus.yaml. The map was rebuilt in ${r.map.buildMs} ms: ${r.map.components} components, ${r.map.files?.toLocaleString()} files.`);
			else toast.warning(`Saved onus.yaml, but the map did not rebuild: ${r.map.error}`);
			app.refresh();
			schema.run(() => api('config.schema'));
		} catch (e) {
			if (e instanceof ApiError && e.status === 409) conflict = true;
			else toast.error(e instanceof Error ? e.message : String(e));
		}
	}

	// Edited elsewhere (an editor, git pull) while this page was open.
	async function onFocus() {
		const f = await api<ConfigFile>('config').catch(() => undefined);
		if (!f || f.hash === draft.baseHash) return;
		if (draft.dirty) conflict = true;
		else {
			file.value = f;
			draft.load(f.text, f.hash);
			toast.info('onus.yaml changed on disk; showing the new version.');
		}
	}

	beforeNavigate((nav) => {
		if (draft.dirty && !confirm('onus.yaml has unsaved changes. Leave without saving?')) nav.cancel();
	});

	function keydown(e: KeyboardEvent) {
		if ((e.metaKey || e.ctrlKey) && e.key === 's') {
			e.preventDefault();
			save();
		}
	}

	function yamlKeydown(e: KeyboardEvent) {
		if (e.key !== 'Tab' || e.shiftKey) return;
		e.preventDefault();
		const el = e.currentTarget as HTMLTextAreaElement;
		const at = el.selectionStart;
		draft.text = draft.text.slice(0, at) + '  ' + draft.text.slice(el.selectionEnd);
		requestAnimationFrame(() => el.setSelectionRange(at + 2, at + 2));
	}

	const componentIds = $derived(
		[...new Set([...Object.keys(draft.data.components ?? {}), ...(schema.value?.components ?? []).map((c) => c.id)])].sort()
	);
	const count = (v: unknown) => (Array.isArray(v) ? v.length : v && typeof v === 'object' ? Object.keys(v).length : 0);
	const sections = $derived([
		{ id: 'components', label: 'Components', n: count(draft.data.components) },
		{ id: 'labels', label: 'Labels', n: count(draft.data.labels) },
		{ id: 'rules', label: 'Boundary rules', n: count(draft.data.rules) },
		{ id: 'contracts', label: 'Contracts', n: count(draft.data.contracts) },
		{ id: 'lanes', label: 'Risk lanes', n: draft.data.lanes ? count(draft.data.lanes.rules) || 'on' : 0 },
		{ id: 'detection', label: 'Detection', n: count(draft.data.extractors?.externals) + count(draft.data.extractors?.events?.publish) },
		{ id: 'environment', label: 'Environment', n: count(draft.data.environment) }
	]);
	const empty = $derived(!draft.exists && !draft.dirty);
</script>

<svelte:window onkeydown={keydown} onfocus={onFocus} onbeforeunload={(e) => draft.dirty && e.preventDefault()} />

<PageHead title="onus.yaml" guide="configuration">
	What Onus cannot infer: which components are sensitive, which boundaries must hold, how changes are routed. Edits are checked as you make them and written to the file only when you save; the map is rebuilt with them straight away.
</PageHead>

{#if file.error}
	<ErrorBox error={file.error} />
{:else if !file.value}
	<Loading />
{:else if empty}
	<Card title="No onus.yaml yet">
		<div class="grid gap-4">
			<p class="text-sm text-muted-foreground">
				Onus works without one: components come from your workspaces and owners from CODEOWNERS. Add one to mark sensitive code, set boundaries and route changes into lanes.
			</p>
			<div class="flex flex-wrap gap-2">
				<Button disabled={!init.value} onclick={() => (draft.text = init.value?.text ?? '')}>Start from what Onus infers</Button>
				<Button variant="outline" onclick={() => (draft.text = 'version: 1\n')}>Start empty</Button>
			</div>
			{#if init.value}<pre class="max-h-80 text-xs">{init.value.text}</pre>{/if}
		</div>
	</Card>
{:else}
	<div class="sticky top-(--header-height) z-[5] -mx-1 grid gap-2 bg-background/85 px-1 pt-2 pb-2 backdrop-blur">
		<div class="flex flex-wrap items-center gap-3 rounded-xl border bg-card px-4 py-2.5 shadow-xs">
			<FileCode class="size-4 text-muted-foreground" />
			<code class="text-xs" title={file.value.path}>onus.yaml</code>
			{#if draft.syntax}<Badge tone="del">YAML error</Badge>
			{:else if check.state === 'error'}<Badge tone="del">does not load</Badge>
			{:else if draft.dirty}<Badge tone="signal">unsaved · {changedLines} {changedLines === 1 ? 'line' : 'lines'}</Badge>
			{:else if draft.exists}<Badge tone="add">saved</Badge>
			{:else}<Badge tone="signal">new file</Badge>{/if}
			{#if check.state === 'checking'}<span class="text-xs text-muted-foreground">checking…</span>{/if}
			<div class="ml-auto flex gap-2">
				{#if draft.dirty}
					<Button variant="ghost" size="sm" onclick={() => (reviewing = !reviewing)}>{reviewing ? 'Hide changes' : 'Review changes'}</Button>
					<Button variant="outline" size="sm" onclick={() => draft.load(file.value?.text ?? null, file.value?.hash ?? null)}><RotateCcw />Discard</Button>
				{/if}
				<Button size="sm" disabled={!draft.dirty || blocked || saving.running} onclick={save} title="Save (⌘S)"><Save />Save</Button>
			</div>
		</div>
		{#if conflict}
			<div class="flex flex-wrap items-center gap-3 rounded-xl border border-signal/50 bg-signal-soft px-4 py-2.5 text-sm">
				<span>onus.yaml changed on disk since you opened it. Saving now would overwrite that change, so it is refused.</span>
				<div class="ml-auto flex gap-2">
					<Copy text={draft.text} />
					<Button variant="outline" size="sm" onclick={load}>Load the version on disk</Button>
				</div>
			</div>
		{/if}
		{#if draft.syntax}<ErrorBox title="The YAML does not parse" error={draft.syntax} />
		{:else if check.state === 'error' && check.error}<ErrorBox title="Onus cannot load this onus.yaml" error={check.error} />{/if}
	</div>

	{#if reviewing && draft.dirty}
		<Card title="Changes to onus.yaml" subtitle="What saving will write." pad={false} class="mb-4">
			<pre class="overflow-x-auto p-0 font-mono text-xs leading-5">{#each hunks(diff) as l, k (k)}{#if l === null}<div class="bg-muted/50 px-4 text-muted-foreground">⋯</div>{:else}<div class="px-4 {l.op === '+' ? 'bg-success-soft text-success' : l.op === '-' ? 'bg-danger-soft text-destructive' : 'text-muted-foreground'}"><span class="mr-3 inline-block w-8 text-right opacity-60 select-none">{l.op === '-' ? l.a : l.b}</span>{l.op} {l.text}</div>{/if}{/each}</pre>
		</Card>
	{/if}

	<div class="mb-4"><Tabs bind:value={tab} tabs={[{ id: 'form', label: 'Form' }, { id: 'yaml', label: 'YAML' }, { id: 'suggested', label: 'Suggested by onus init' }]} /></div>

	{#if tab === 'form'}
		{#if draft.syntax}
			<p class="text-sm text-muted-foreground">The form needs YAML that parses. Fix it in the YAML tab, or discard your changes.</p>
		{:else}
			<div class="grid gap-6 xl:grid-cols-[180px_minmax(0,1fr)]">
				<nav class="hidden xl:block">
					<ul class="sticky top-40 grid gap-0.5 text-sm">
						{#each sections as s (s.id)}
							<li>
								<a class="flex items-center justify-between rounded-md px-2 py-1.5 text-muted-foreground hover:bg-muted hover:text-foreground" href="#{s.id}">
									{s.label}{#if s.n}<span class="text-xs tabular-nums">{s.n}</span>{/if}
								</a>
							</li>
						{/each}
					</ul>
				</nav>
				<div class="grid min-w-0 gap-4">
					<section id="components" class="scroll-mt-40"><ComponentsSection {draft} schema={schema.value} /></section>
					<section id="labels" class="scroll-mt-40"><LabelsSection {draft} sensitivities={schema.value?.sensitivities} /></section>
					<section id="rules" class="scroll-mt-40"><RulesSection {draft} {componentIds} /></section>
					<section id="contracts" class="scroll-mt-40"><ContractsSection {draft} /></section>
					<section id="lanes" class="scroll-mt-40"><LanesSection {draft} schema={schema.value} {componentIds} /></section>
					<section id="detection" class="scroll-mt-40"><DetectionSection {draft} /></section>
					<section id="environment" class="scroll-mt-40"><EnvironmentSection {draft} /></section>
				</div>
			</div>
		{/if}
	{:else if tab === 'yaml'}
		<Card pad={false}>
			<textarea
				class="block min-h-[65vh] w-full resize-y rounded-xl bg-transparent p-4 font-mono text-xs leading-5 outline-none"
				spellcheck="false"
				aria-label="onus.yaml"
				bind:value={draft.text}
				onkeydown={yamlKeydown}
			></textarea>
		</Card>
	{:else if init.value}
		<Card title="Inferred from workspaces and CODEOWNERS" subtitle="Suggested labels are commented out until you confirm them.">
			{#snippet actions()}
				<Copy text={init.value?.text ?? ''} />
				<Button
					variant="outline"
					size="sm"
					onclick={() => {
						if (!draft.text.trim() || confirm('Replace the editor contents with the suggestion? Nothing is written until you save.')) {
							draft.text = init.value?.text ?? '';
							tab = 'form';
						}
					}}>Use as a starting point</Button
				>
			{/snippet}
			<pre class="max-h-[65vh] text-xs">{init.value.text}</pre>
		</Card>
	{:else if init.error}<ErrorBox error={init.error} />{:else}<Loading />{/if}
{/if}
