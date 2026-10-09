<script lang="ts">
	import { api } from '#lib/api.ts';
	import Badge from '#lib/components/Badge.svelte';
	import Card from '#lib/components/Card.svelte';
	import Empty from '#lib/components/Empty.svelte';
	import ErrorBox from '#lib/components/ErrorBox.svelte';
	import Loading from '#lib/components/Loading.svelte';
	import PageHead from '#lib/components/PageHead.svelte';
	import RefPicker from '#lib/components/RefPicker.svelte';
	import { ago, short } from '#lib/format.ts';
	import { Task } from '#lib/task.svelte.ts';
	import type { Env, EnvSpec, Manifest, Run, TestRun } from '#lib/types.ts';
	import { onMount } from 'svelte';

	const engine = new Task<{ engine: string | null; engineError: string | null; spec: EnvSpec | null; specError: string | null }>();
	const envs = new Task<Env[]>();
	const runs = new Task<{ store: string; runs: Run[] }>();
	const creating = new Task<Env>();
	const running = new Task<{ id: string; manifest: Manifest; run: TestRun }>();
	const oneOff = new Task<TestRun>();
	const destroying = new Task<unknown>();

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
			refresh();
		}
	}

	async function run(e: SubmitEvent) {
		e.preventDefault();
		const r = await running.run(() => api('envs.run', { name: target, command }));
		if (r) runs.run(() => api('evidence.list'));
	}

	async function destroy(n: string) {
		if (!confirm(`Remove environment ${n}, its proxy and its network?`)) return;
		await destroying.run(() => api('envs.destroy', { name: n }));
		if (target === n) target = '';
		refresh();
	}

	function runOnce(e: SubmitEvent) {
		e.preventDefault();
		oneOff.run(() => api<TestRun>('runTest', { ...test, setup: test.setup || undefined }));
	}
</script>

<PageHead title="Environments" guide="environments">
	Containers built from a commit that run your tests and keep what they recorded, so approval rests on evidence
	an environment produced, not on what an agent says it ran.
</PageHead>

<div class="stack">
	{#if engine.value}
		{#if !engine.value.engine}
			<Card tone="signal" title="No container engine">
				<p class="muted">{engine.value.engineError} Environments, the test runner and the judge need Docker or Podman.</p>
			</Card>
		{/if}
		{#if engine.value.specError}
			<Card tone="signal" title="No environment declared">
				<p class="muted" style="margin-bottom: 12px">{engine.value.specError}</p>
				<pre>{`environment:
  image: node:22-alpine
  setup: npm ci                      # with network, once per lockfile
  seed: node scripts/seed-test-data.mjs
  evidence: ["reports/**/*.xml"]     # JUnit XML the tests write
  traces: ["otel/*.json"]            # OTLP JSON trace files`}</pre>
			</Card>
		{:else if engine.value.spec}
			{@const s = engine.value.spec}
			<Card title="The environment" subtitle="From onus.yaml in your working tree (or the devcontainer), never from the commit under test.">
				<dl>
					<dt>Engine</dt><dd>{engine.value.engine}</dd>
					<dt>Image</dt><dd class="mono">{s.image}</dd>
					<dt>Setup</dt><dd class="mono">{s.setup ?? '–'}</dd>
					<dt>Seed</dt><dd class="mono">{s.seed ?? '–'}</dd>
					<dt>Evidence</dt><dd class="mono">{s.evidence.join(', ') || '–'}</dd>
					<dt>Traces</dt><dd class="mono">{s.traces.join(', ') || '–'}</dd>
				</dl>
			</Card>
		{/if}
	{:else if engine.running}
		<Loading label="Looking for a container engine…" />
	{/if}

	<div class="grid-2">
		<Card title="Create">
			<form class="stack" onsubmit={create}>
				<RefPicker id="env-ref" label="Commit" bind:value={ref} />
				<label class="field">Name (default: the token's task, else the short commit)<input bind:value={name} placeholder="sms-alerts" /></label>
				<label class="field">Task token (optional: its hosts open the egress proxy, its secrets come from ONUS_SECRET_*)<input class="mono" bind:value={token} /></label>
				<div class="row">
					<button class="primary" type="submit" disabled={creating.running || !engine.value?.spec}>{creating.running ? 'Building…' : 'Create'}</button>
					{#if creating.running}<span class="muted small">The first build of a lockfile runs setup; later ones start from the warm image.</span>{/if}
				</div>
				{#if creating.error}<ErrorBox error={creating.error} />{/if}
			</form>
		</Card>
		<Card title="Run a command" subtitle="Each run leaves a manifest, its log, JUnit results and traces in the evidence store.">
			<form class="stack" onsubmit={run}>
				<label class="field">
					Environment
					<select bind:value={target}>
						{#each envs.value ?? [] as e (e.name)}<option value={e.name}>{e.name} ({short(e.commit)})</option>{/each}
					</select>
				</label>
				<label class="field"><span>Command (run with <code>sh -c</code> at the repository root)</span><input class="mono" bind:value={command} /></label>
				<div class="row"><button class="primary" type="submit" disabled={running.running || !target || !command.trim()}>{running.running ? 'Running…' : 'Run'}</button></div>
			</form>
			{#if running.error}<ErrorBox error={running.error} />{/if}
			{#if running.value}
				{@const r = running.value}
				<div class="stack tight" style="margin-top: 12px">
					<div class="row">
						<Badge tone={r.manifest.exitCode === 0 ? 'add' : 'del'}>exit {r.manifest.exitCode}</Badge>
						{#if r.manifest.tests}<span class="small">{r.manifest.tests.tests} tests, {r.manifest.tests.failures + r.manifest.tests.errors} failed</span>{/if}
						<a class="small" href="/evidence/{r.id}">Run {short(r.id, 10)}</a>
					</div>
					<pre class="log">{r.run.outputTail}</pre>
				</div>
			{/if}
		</Card>
	</div>

	<Card title="Environments" pad={false}>
		{#snippet actions()}<button class="small" onclick={refresh}>Refresh</button>{/snippet}
		{#if envs.error}
			<div style="padding: 16px"><ErrorBox error={envs.error} /></div>
		{:else if envs.value?.length}
			<table class="data">
				<thead><tr><th>Name</th><th>Commit</th><th>State</th><th>Image</th><th>Network</th><th>Secrets</th><th>Created</th><th></th></tr></thead>
				<tbody>
					{#each envs.value as e (e.name)}
						<tr>
							<td><strong>{e.name}</strong></td>
							<td class="mono">{short(e.commit)}</td>
							<td><Badge tone={e.state === 'running' ? 'add' : 'faint'}>{e.state}</Badge></td>
							<td class="mono small">{e.warmImage ?? e.image}</td>
							<td class="small">{e.hosts.length ? e.hosts.join(', ') : 'none'}</td>
							<td class="small mono">{e.secrets.join(', ') || '–'}</td>
							<td class="small faint">{ago(e.createdAt)}</td>
							<td class="num"><button class="small danger" onclick={() => destroy(e.name)}>Destroy</button></td>
						</tr>
					{/each}
				</tbody>
			</table>
		{:else if envs.value}
			<div style="padding: 16px"><Empty title="No environments" /></div>
		{:else}
			<div style="padding: 16px"><Loading /></div>
		{/if}
	</Card>

	<Card title="Evidence" subtitle={runs.value ? `Content-addressed, in ${runs.value.store}` : undefined} pad={false}>
		{#if runs.error}
			<div style="padding: 16px"><ErrorBox error={runs.error} /></div>
		{:else if runs.value?.runs.length}
			<table class="data">
				<thead><tr><th>Run</th><th>Commit</th><th>Environment</th><th>Command</th><th>Result</th><th>Tests</th><th>When</th></tr></thead>
				<tbody>
					{#each runs.value.runs as r (r.id)}
						<tr>
							<td><a class="mono" href="/evidence/{r.id}">{short(r.id, 10)}</a></td>
							<td class="mono">{short(r.manifest.commit)}</td>
							<td>{r.manifest.environment.name}</td>
							<td class="mono small">{r.manifest.command}</td>
							<td><Badge tone={r.manifest.exitCode === 0 ? 'add' : 'del'}>exit {r.manifest.exitCode}</Badge></td>
							<td class="small">{r.manifest.tests ? `${r.manifest.tests.tests} (${r.manifest.tests.failures + r.manifest.tests.errors} failed)` : ''}</td>
							<td class="small faint">{ago(r.manifest.finishedAt)}</td>
						</tr>
					{/each}
				</tbody>
			</table>
		{:else if runs.value}
			<div style="padding: 16px"><Empty title="No runs recorded yet" /></div>
		{:else}
			<div style="padding: 16px"><Loading /></div>
		{/if}
	</Card>

	<Card title="Run a test once" subtitle="onus run-test: a throwaway container with no network, for reproducing a failure. Nothing is recorded.">
		<form class="stack" onsubmit={runOnce}>
			<div class="grid-2">
				<RefPicker id="rt-ref" label="Commit" bind:value={test.ref} />
				<label class="field">Image<input class="mono" bind:value={test.image} /></label>
				<label class="field">Setup (with network)<input class="mono" bind:value={test.setup} placeholder="npm ci" /></label>
				<label class="field">Command<input class="mono" bind:value={test.command} /></label>
			</div>
			<div class="row"><button type="submit" disabled={oneOff.running}>{oneOff.running ? 'Running…' : 'Run'}</button></div>
		</form>
		{#if oneOff.error}<ErrorBox error={oneOff.error} />{/if}
		{#if oneOff.value}
			<div class="stack tight" style="margin-top: 12px">
				<Badge tone={oneOff.value.exitCode === 0 ? 'add' : 'del'}>exit {oneOff.value.exitCode}</Badge>
				<pre class="log">{oneOff.value.outputTail}</pre>
			</div>
		{/if}
	</Card>
</div>

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
		max-height: 320px;
		white-space: pre-wrap;
	}
</style>
