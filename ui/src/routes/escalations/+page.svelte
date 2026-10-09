<script lang="ts">
	import { api } from '#lib/api.ts';
	import Badge from '#lib/components/Badge.svelte';
	import Card from '#lib/components/Card.svelte';
	import Copy from '#lib/components/Copy.svelte';
	import Empty from '#lib/components/Empty.svelte';
	import ErrorBox from '#lib/components/ErrorBox.svelte';
	import Inline from '#lib/components/Inline.svelte';
	import Loading from '#lib/components/Loading.svelte';
	import PageHead from '#lib/components/PageHead.svelte';
	import { ago } from '#lib/format.ts';
	import { Task } from '#lib/task.svelte.ts';
	import type { EscalationItem } from '#lib/types.ts';
	import { onMount } from 'svelte';

	const list = new Task<{ dir: string; requests: EscalationItem[] }>();
	const created = new Task<unknown>();
	const acted = new Task<{ token?: string; decision?: string; reasons?: string[]; denied?: string }>();

	let form = $state({ task: '', kind: 'permission', scopes: '', evidence: '', reason: '' });
	let openId = $state<string | null>(null);
	let act = $state({ token: '', by: '', reason: '', reproduceAt: 'HEAD', image: 'node:22', setup: '', testCommand: 'npx vitest run {test}' });

	const refresh = () => list.run(() => api('escalations.list'));
	onMount(refresh);

	const lines = (s: string) => s.split('\n').map((l) => l.trim()).filter(Boolean);

	async function create(e: SubmitEvent) {
		e.preventDefault();
		const r = await created.run(() =>
			api('escalations.create', { task: form.task, kind: form.kind, scopes: lines(form.scopes), evidence: lines(form.evidence), reason: form.reason })
		);
		if (r) {
			form = { task: form.task, kind: 'permission', scopes: '', evidence: '', reason: '' };
			refresh();
		}
	}

	async function run(name: string, body: Record<string, unknown>) {
		const r = await acted.run(() => api(name, body));
		if (r) refresh();
	}

	const grade = (n: number) => ['', 'failing test', 'trace', 'map path', 'draft diff', 'rationale'][n] ?? `grade ${n}`;
	const decisionTone = (d: string) => (d === 'granted' ? 'add' : d === 'denied' ? 'del' : 'signal');
</script>

<PageHead title="Escalations" guide="scopes">
	When a task needs more than its token allows, it asks with evidence. Low-risk requests with a reproduced
	failing test are granted by policy; everything else goes to a person.
</PageHead>

<div class="stack">
	<Card title="Ask for more" subtitle="Evidence strongest first: failing-test:&lt;file&gt;, trace:&lt;file&gt;, map-path:&lt;path&gt;, draft-diff:&lt;file&gt;, rationale:&lt;text&gt;.">
		<form class="stack" onsubmit={create}>
			<div class="grid-3">
				<label class="field">Task<input bind:value={form.task} required /></label>
				<label class="field">Kind
					<select bind:value={form.kind}>
						<option value="permission">permission</option>
						<option value="broken-test">broken test</option>
						<option value="contradictory-spec">contradictory spec</option>
						<option value="impossible-task">impossible task</option>
					</select>
				</label>
				<label class="field">Why<input bind:value={form.reason} required /></label>
			</div>
			<div class="grid-2">
				<label class="field">Rights asked for, one per line<textarea rows="3" bind:value={form.scopes} placeholder="write:path:services/orders/src/**"></textarea></label>
				<label class="field">Evidence, one per line<textarea rows="3" bind:value={form.evidence} placeholder="failing-test:services/orders/src/ship.test.ts"></textarea></label>
			</div>
			<div class="row"><button class="primary" type="submit" disabled={created.running}>File the request</button></div>
			{#if created.error}<ErrorBox error={created.error} />{/if}
		</form>
	</Card>

	<Card title="Requests" subtitle={list.value?.dir} pad={false}>
		{#if list.error}
			<div style="padding: 16px"><ErrorBox error={list.error} /></div>
		{:else if !list.value}
			<div style="padding: 16px"><Loading /></div>
		{:else if !list.value.requests.length}
			<div style="padding: 16px"><Empty title="No requests" /></div>
		{:else}
			{#each list.value.requests as item (item.request.id)}
				{@const r = item.request}
				<div class="req">
					<button class="ghost head" onclick={() => (openId = openId === r.id ? null : r.id)}>
						<strong>{r.task}</strong>
						<span class="mono small">{r.scopes.join(', ') || r.kind}</span>
						<span class="row">
							{#if r.sensitive.length}<Badge tone="signal">{r.sensitive.join(', ')}</Badge>{/if}
							<Badge tone="faint">blast radius {r.blastRadius}</Badge>
							{#if item.decision}<Badge tone={decisionTone(item.decision.decision)}>{item.decision.decision}</Badge>{:else}<Badge tone="info">open</Badge>{/if}
						</span>
						<span class="faint small">{ago(r.at)}</span>
					</button>
					{#if openId === r.id}
						<div class="body stack">
							<p>{r.reason}</p>
							<ul class="small">
								{#each r.evidence as e, i (i)}
									<li>
										<Badge tone={e.grade === 1 ? 'add' : 'faint'}>{grade(e.grade)}</Badge>
										<span class="mono">{e.reference}</span>
										{#if e.reproduced !== undefined && e.reproduced !== null}<Badge tone={e.reproduced ? 'add' : 'del'}>{e.reproduced ? 'reproduced' : 'not reproduced'}</Badge>{/if}
									</li>
								{:else}<li class="muted">No evidence</li>{/each}
							</ul>
							{#if item.decision}
								<p class="small muted">{item.decision.decision} by {item.decision.by}: {item.decision.reason}</p>
							{/if}
							<div class="grid-2">
								<label class="field">The task's token<input class="mono" bind:value={act.token} /></label>
								<label class="field">You (for a person's decision)<input bind:value={act.by} placeholder="@team-orders" /></label>
							</div>
							<details>
								<summary class="small muted">How to reproduce failing tests</summary>
								<div class="grid-2" style="margin-top: 8px">
									<label class="field">At<input class="mono" bind:value={act.reproduceAt} /></label>
									<label class="field">Image<input class="mono" bind:value={act.image} /></label>
									<label class="field">Setup<input class="mono" bind:value={act.setup} placeholder="npm ci" /></label>
									<label class="field">Test command<input class="mono" bind:value={act.testCommand} /></label>
								</div>
							</details>
							<div class="row">
								<button disabled={!act.token || acted.running} onclick={() => run('escalations.decide', { id: r.id, token: act.token, reproduceAt: act.reproduceAt, image: act.image, setup: act.setup, testCommand: act.testCommand })}>Decide by policy</button>
								<button class="primary" disabled={!act.token || !act.by || acted.running} onclick={() => run('escalations.grant', { id: r.id, token: act.token, by: act.by })}>Grant</button>
								<input bind:value={act.reason} placeholder="Why not" />
								<button class="danger" disabled={!act.by || !act.reason || acted.running} onclick={() => run('escalations.deny', { id: r.id, by: act.by, reason: act.reason })}>Deny</button>
							</div>
							{#if acted.running}<Loading label="Deciding; reproducing a failing test runs a container…" />{/if}
							{#if acted.error}<ErrorBox error={acted.error} />{/if}
							{#if acted.value}
								{#if acted.value.decision}
									<p><Badge tone={decisionTone(acted.value.decision)}>{acted.value.decision}</Badge> {#each acted.value.reasons ?? [] as r (r)}<Inline text={r} /> {/each}</p>
								{/if}
								{#if acted.value.token}
									<div class="spread"><span class="small muted">The new token: the original rights plus exactly what was asked</span><Copy text={acted.value.token} /></div>
									<pre class="token">{acted.value.token}</pre>
								{/if}
								{#if acted.value.denied}<p class="small">Denied and logged.</p>{/if}
							{/if}
						</div>
					{/if}
				</div>
			{/each}
		{/if}
	</Card>
</div>

<style>
	.req {
		border-bottom: 1px solid var(--line);
	}
	.req:last-child {
		border-bottom: none;
	}
	.head {
		width: 100%;
		height: auto;
		padding: var(--space-3) var(--space-4);
		display: grid;
		grid-template-columns: auto 1fr auto auto;
		gap: var(--space-3);
		text-align: left;
		border-radius: 0;
		font-weight: 400;
	}
	.body {
		padding: 0 var(--space-4) var(--space-4);
	}
	.body ul {
		margin: 0;
		padding-left: 1.2em;
		display: grid;
		gap: 4px;
	}
	.token {
		white-space: pre-wrap;
		word-break: break-all;
	}
	details summary {
		cursor: pointer;
	}
</style>
