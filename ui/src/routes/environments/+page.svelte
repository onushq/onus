<script lang="ts">
	import * as Alert from '$lib/components/ui/alert/index.js';
	import { Button } from '$lib/components/ui/button/index.js';
	import { Input } from '$lib/components/ui/input/index.js';
	import * as Table from '$lib/components/ui/table/index.js';
	import { api } from '#lib/api.ts';
	import Badge from '#lib/components/Badge.svelte';
	import Card from '#lib/components/Card.svelte';
	import Empty from '#lib/components/Empty.svelte';
	import ErrorBox from '#lib/components/ErrorBox.svelte';
	import Field from '#lib/components/Field.svelte';
	import Loading from '#lib/components/Loading.svelte';
	import NativeSelect from '#lib/components/NativeSelect.svelte';
	import PageHead from '#lib/components/PageHead.svelte';
	import RefPicker from '#lib/components/RefPicker.svelte';
	import Stat from '#lib/components/Stat.svelte';
	import { ago, short } from '#lib/format.ts';
	import { Task } from '#lib/task.svelte.ts';
	import type { Env, EnvSpec, Manifest, Run, TestRun } from '#lib/types.ts';
	import Boxes from '@lucide/svelte/icons/boxes';
	import CircleAlert from '@lucide/svelte/icons/circle-alert';
	import Container from '@lucide/svelte/icons/container';
	import FlaskConical from '@lucide/svelte/icons/flask-conical';
	import Play from '@lucide/svelte/icons/play';
	import Plus from '@lucide/svelte/icons/plus';
	import RefreshCw from '@lucide/svelte/icons/refresh-cw';
	import Trash2 from '@lucide/svelte/icons/trash-2';
	import { onMount } from 'svelte';
	import { toast } from 'svelte-sonner';

	const engine = new Task<{ engine: string | null; engineError: string | null; spec: EnvSpec | null; specError: string | null }>();
	const envs = new Task<Env[]>();
	const runs = new Task<{ store: string; runs: Run[] }>();
	const creating = new Task<Env>();
	const running = new Task<{ id: string; manifest: Manifest; run: TestRun }>();
	const oneOff = new Task<TestRun>();

	let ref = $state('HEAD');
	let name = $state('');
	let token = $state('');
	let target = $state('');
	let command = $state('npm test');
	let test = $state({ ref: 'HEAD', image: 'node:22', setup: '', command: 'npm test' });

	function refresh() {
		envs.run(() => api<Env[]>('envs.list'));
		runs.run(() => api('evidence.list'));
	}

	onMount(() => {
		engine.run(() => api('envs.engine'));
		refresh();
	});

	$effect(() => {
		if (!target && envs.value?.length) target = envs.value[0].name;
	});

	async function create(e: SubmitEvent) {
		e.preventDefault();
		const env = await creating.run(() => api<Env>('envs.create', { ref, name: name.trim() || undefined, token: token.trim() || undefined }));
		if (env) {
			target = env.name;
			name = '';
			toast.success(`Environment ${env.name} is ready`, { description: `${short(env.commit)} on ${env.warmImage ?? env.image}` });
			refresh();
		}
	}

	async function run(e: SubmitEvent) {
		e.preventDefault();
		const r = await running.run(() => api('envs.run', { name: target, command }));
		if (r) {
			(r.manifest.exitCode === 0 ? toast.success : toast.error)(`exit ${r.manifest.exitCode}`, { description: `Recorded as run ${short(r.id, 10)}` });
			runs.run(() => api('evidence.list'));
		}
	}

	async function destroy(n: string) {
		if (!confirm(`Remove environment ${n}, its proxy and its network?`)) return;
		try {
			await api('envs.destroy', { name: n });
			toast(`Environment ${n} removed`);
		} catch (err) {
			toast.error(err instanceof Error ? err.message : String(err));
		}
		if (target === n) target = '';
		refresh();
	}

	function runOnce(e: SubmitEvent) {
		e.preventDefault();
		oneOff.run(() => api<TestRun>('runTest', { ...test, setup: test.setup || undefined }));
	}

	const passing = $derived((runs.value?.runs ?? []).filter((r) => r.manifest.exitCode === 0).length);
</script>

<PageHead title="Environments" guide="environments">
	Containers built from a commit that run your tests and keep what they recorded, so approval rests on evidence an environment produced, not on what an agent says it ran.
</PageHead>

{#if engine.value && !engine.value.engine}
	<Alert.Root class="border-signal/60 bg-signal-soft/40">
		<CircleAlert />
		<Alert.Title>No container engine</Alert.Title>
		<Alert.Description>{engine.value.engineError} Environments, the test runner and the judge need Docker or Podman.</Alert.Description>
	</Alert.Root>
{/if}

<div class="grid grid-cols-2 gap-3 md:grid-cols-4">
	<Stat label="Engine" value={engine.value?.engine ?? (engine.running ? '…' : 'none')} icon={Container} />
	<Stat label="Environments" value={envs.value?.length ?? '…'} icon={Boxes} hint={envs.value ? `${envs.value.filter((e) => e.state === 'running').length} running` : undefined} />
	<Stat label="Recorded runs" value={runs.value?.runs.length ?? '…'} icon={FlaskConical} hint={runs.value ? `${passing} passed` : undefined} />
	<Stat label="Image" value={engine.value?.spec?.image ?? '–'} hint={engine.value?.spec?.setup ? `setup: ${engine.value.spec.setup}` : 'no setup'} />
</div>

{#if engine.value?.specError}
	<Card title="No environment declared" tone="signal">
		<p class="mb-3 text-sm text-muted-foreground">{engine.value.specError}</p>
		<pre>{`environment:
  image: node:22-alpine
  setup: npm ci                      # with network, once per lockfile
  seed: node scripts/seed-test-data.mjs
  evidence: ["reports/**/*.xml"]     # JUnit XML the tests write
  traces: ["otel/*.json"]            # OTLP JSON trace files`}</pre>
	</Card>
{/if}

<div class="grid gap-4 xl:grid-cols-2">
	<Card title="Create" subtitle="From onus.yaml in your working tree, never from the commit under test.">
		<form class="grid gap-3" onsubmit={create}>
			<RefPicker id="env-ref" label="Commit" bind:value={ref} />
			<Field label="Name" hint="Default: the token's task, else the short commit."><Input bind:value={name} placeholder="sms-alerts" /></Field>
			<Field label="Task token (optional)" hint="Its hosts open the egress proxy; its secrets come from ONUS_SECRET_<NAME>."><Input class="font-mono" bind:value={token} /></Field>
			<div class="flex items-center gap-3">
				<Button type="submit" disabled={creating.running || !engine.value?.spec}><Plus />{creating.running ? 'Building…' : 'Create'}</Button>
				{#if creating.running}<span class="text-xs text-muted-foreground">The first build of a lockfile runs setup; later ones start from the warm image.</span>{/if}
			</div>
			{#if creating.error}<ErrorBox error={creating.error} />{/if}
		</form>
	</Card>
	<Card title="Run a command" subtitle="Each run leaves a manifest, its log, JUnit results and traces in the evidence store.">
		<form class="grid gap-3" onsubmit={run}>
			<Field label="Environment">
				<NativeSelect bind:value={target} class="w-full">
					{#each envs.value ?? [] as e (e.name)}<option value={e.name}>{e.name} ({short(e.commit)})</option>{/each}
				</NativeSelect>
			</Field>
			<Field label="Command" hint="Run with sh -c at the repository root."><Input class="font-mono" bind:value={command} /></Field>
			<div><Button type="submit" disabled={running.running || !target || !command.trim()}><Play />{running.running ? 'Running…' : 'Run'}</Button></div>
		</form>
		{#if running.error}<div class="mt-3"><ErrorBox error={running.error} /></div>{/if}
		{#if running.value}
			{@const r = running.value}
			<div class="mt-4 grid gap-2">
				<div class="flex flex-wrap items-center gap-2 text-sm">
					<Badge tone={r.manifest.exitCode === 0 ? 'add' : 'del'}>exit {r.manifest.exitCode}</Badge>
					{#if r.manifest.tests}<span>{r.manifest.tests.tests} tests, {r.manifest.tests.failures + r.manifest.tests.errors} failed</span>{/if}
					<a class="ml-auto font-mono text-xs underline" href="/evidence/{r.id}">run {short(r.id, 10)}</a>
				</div>
				<pre class="max-h-72 whitespace-pre-wrap">{r.run.outputTail || '(no output)'}</pre>
			</div>
		{/if}
	</Card>
</div>

<Card title="Environments" pad={false}>
	{#snippet actions()}<Button variant="outline" size="sm" onclick={refresh}><RefreshCw />Refresh</Button>{/snippet}
	{#if envs.error}
		<div class="p-4"><ErrorBox error={envs.error} /></div>
	{:else if envs.value?.length}
		<Table.Root>
			<Table.Header><Table.Row><Table.Head class="pl-4">Name</Table.Head><Table.Head>Commit</Table.Head><Table.Head>State</Table.Head><Table.Head>Image</Table.Head><Table.Head>Network</Table.Head><Table.Head>Secrets</Table.Head><Table.Head>Created</Table.Head><Table.Head class="pr-4"></Table.Head></Table.Row></Table.Header>
			<Table.Body>
				{#each envs.value as e (e.name)}
					<Table.Row>
						<Table.Cell class="pl-4 font-medium">{e.name}</Table.Cell>
						<Table.Cell class="font-mono text-xs">{short(e.commit)}</Table.Cell>
						<Table.Cell><Badge tone={e.state === 'running' ? 'add' : 'faint'}>{e.state}</Badge></Table.Cell>
						<Table.Cell class="font-mono text-xs">{e.warmImage ?? e.image}</Table.Cell>
						<Table.Cell class="text-xs">{e.hosts.length ? e.hosts.join(', ') : 'none'}</Table.Cell>
						<Table.Cell class="font-mono text-xs">{e.secrets.join(', ') || '–'}</Table.Cell>
						<Table.Cell class="text-xs text-muted-foreground">{ago(e.createdAt)}</Table.Cell>
						<Table.Cell class="pr-4 text-right"><Button variant="ghost" size="icon-sm" aria-label="Destroy {e.name}" onclick={() => destroy(e.name)}><Trash2 class="text-destructive" /></Button></Table.Cell>
					</Table.Row>
				{/each}
			</Table.Body>
		</Table.Root>
	{:else if envs.value}
		<div class="p-4"><Empty title="No environments">Create one above, or with <code>onus env create</code>.</Empty></div>
	{:else}
		<div class="p-4"><Loading /></div>
	{/if}
</Card>

<Card title="Evidence" subtitle={runs.value ? `Content-addressed, in ${runs.value.store}` : undefined} pad={false}>
	{#if runs.error}
		<div class="p-4"><ErrorBox error={runs.error} /></div>
	{:else if runs.value?.runs.length}
		<Table.Root>
			<Table.Header><Table.Row><Table.Head class="pl-4">Run</Table.Head><Table.Head>Result</Table.Head><Table.Head>Command</Table.Head><Table.Head>Tests</Table.Head><Table.Head>Commit</Table.Head><Table.Head>Environment</Table.Head><Table.Head class="pr-4">When</Table.Head></Table.Row></Table.Header>
			<Table.Body>
				{#each runs.value.runs as r (r.id)}
					<Table.Row>
						<Table.Cell class="pl-4"><a class="font-mono text-xs underline" href="/evidence/{r.id}">{short(r.id, 10)}</a></Table.Cell>
						<Table.Cell><Badge tone={r.manifest.exitCode === 0 ? 'add' : 'del'}>exit {r.manifest.exitCode}</Badge></Table.Cell>
						<Table.Cell class="max-w-md truncate font-mono text-xs" title={r.manifest.command}>{r.manifest.command}</Table.Cell>
						<Table.Cell class="text-xs">{r.manifest.tests ? `${r.manifest.tests.tests} (${r.manifest.tests.failures + r.manifest.tests.errors} failed)` : ''}</Table.Cell>
						<Table.Cell class="font-mono text-xs">{short(r.manifest.commit)}</Table.Cell>
						<Table.Cell>{r.manifest.environment.name}</Table.Cell>
						<Table.Cell class="pr-4 text-xs text-muted-foreground">{ago(r.manifest.finishedAt)}</Table.Cell>
					</Table.Row>
				{/each}
			</Table.Body>
		</Table.Root>
	{:else if runs.value}
		<div class="p-4"><Empty title="No runs recorded yet" /></div>
	{:else}
		<div class="p-4"><Loading /></div>
	{/if}
</Card>

<Card title="Run a test once" subtitle="onus run-test: a throwaway container with no network, for reproducing a failure. Nothing is recorded.">
	<form class="grid gap-3" onsubmit={runOnce}>
		<div class="grid gap-3 md:grid-cols-2 xl:grid-cols-4">
			<RefPicker id="rt-ref" label="Commit" bind:value={test.ref} />
			<Field label="Image"><Input class="font-mono" bind:value={test.image} /></Field>
			<Field label="Setup (with network)"><Input class="font-mono" bind:value={test.setup} placeholder="npm ci" /></Field>
			<Field label="Command"><Input class="font-mono" bind:value={test.command} /></Field>
		</div>
		<div><Button variant="outline" type="submit" disabled={oneOff.running}><Play />{oneOff.running ? 'Running…' : 'Run once'}</Button></div>
	</form>
	{#if oneOff.error}<div class="mt-3"><ErrorBox error={oneOff.error} /></div>{/if}
	{#if oneOff.value}
		<div class="mt-4 grid gap-2">
			<Badge tone={oneOff.value.exitCode === 0 ? 'add' : 'del'}>exit {oneOff.value.exitCode}</Badge>
			<pre class="max-h-72 whitespace-pre-wrap">{oneOff.value.outputTail || '(no output)'}</pre>
		</div>
	{/if}
</Card>
