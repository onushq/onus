<script lang="ts">
	import { api } from '#lib/api.ts';
	import Badge from '#lib/components/Badge.svelte';
	import Card from '#lib/components/Card.svelte';
	import Copy from '#lib/components/Copy.svelte';
	import ErrorBox from '#lib/components/ErrorBox.svelte';
	import Loading from '#lib/components/Loading.svelte';
	import PageHead from '#lib/components/PageHead.svelte';
	import { date } from '#lib/format.ts';
	import { Task } from '#lib/task.svelte.ts';
	import type { KeysStatus, TokenInfo } from '#lib/types.ts';
	import { onMount } from 'svelte';

	const keys = new Task<KeysStatus>();
	const suggestion = new Task<{ reads: string[]; leftOut: string[] }>();
	const minted = new Task<{ token: string; inspect: TokenInfo }>();
	const inspected = new Task<TokenInfo>();
	const checked = new Task<{ allowed: boolean; right: string; reason?: string }>();
	const narrowed = new Task<{ token: string }>();

	let plan = $state(`task: sms-alerts
writes: ["services/notifications/**"]
# reads: ["services/orders/events/**"]
# writeComponents: [notifications.internal]
# readContracts: [UserPreferences]
# escalateBefore: ["contract:*"]
# hosts: ["sms.test.example"]
# secrets: ["SMS_TEST_KEY"]
ttl: 8h
`);
	let token = $state('');
	let right = $state('write:path:services/notifications/src/sms.ts');
	let only = $state('');

	onMount(() => keys.run(() => api<KeysStatus>('keys.status')));

	function addReads() {
		const reads = suggestion.value?.reads ?? [];
		if (!reads.length) return;
		const line = `reads: [${reads.map((r) => JSON.stringify(r)).join(', ')}]`;
		plan = /^#?\s*reads:.*$/m.test(plan) ? plan.replace(/^#?\s*reads:.*$/m, line) : `${plan.trimEnd()}\n${line}\n`;
	}

	async function mint() {
		const m = await minted.run(() => api('token.mint', { plan }));
		if (m) {
			token = m.token;
			inspected.value = m.inspect;
		}
	}
</script>

<PageHead title="Tokens & scopes" guide="scopes">
	A task gets exactly the paths, hosts and secrets it needs, as a signed token anyone can narrow and nobody can
	widen without the root key. The git gateway and the map server enforce it.
</PageHead>

<div class="stack">
	<Card title="Root key">
		{#if keys.value}
			<div class="spread">
				<div class="stack tight">
					<span class="mono small">{keys.value.dir}</span>
					<div class="row">
						<Badge tone={keys.value.private ? 'add' : 'faint'}>{keys.value.private ? 'private key present' : 'no private key'}</Badge>
						<Badge tone={keys.value.public ? 'add' : 'faint'}>{keys.value.public ? 'public key present' : 'no public key'}</Badge>
					</div>
					{#if keys.value.publicKey}<span class="mono small faint break">{keys.value.publicKey}</span>{/if}
				</div>
				{#if !keys.value.private && !keys.value.public}
					<button class="primary" onclick={() => keys.run(() => api<KeysStatus>('keys.generate'))}>Generate a key pair</button>
				{/if}
			</div>
			<p class="small muted" style="margin-top: 8px">Keep the private key with whoever approves tasks. Start <code>onus ui --keys &lt;dir&gt;</code> to use another pair.</p>
		{:else if keys.error}<ErrorBox error={keys.error} />{:else}<Loading />{/if}
	</Card>

	<div class="grid-2">
		<Card title="Plan a task" subtitle="Writes are exactly what the plan says; components and contracts resolve through the map.">
			<div class="stack">
				<textarea rows="12" bind:value={plan} spellcheck="false"></textarea>
				<div class="row">
					<button onclick={() => suggestion.run(() => api('scope.suggest', { plan }))} disabled={suggestion.running}>Suggest reads</button>
					<button class="primary" onclick={mint} disabled={minted.running || !keys.value?.private}>Mint the token</button>
				</div>
				{#if suggestion.error}<ErrorBox error={suggestion.error} />{/if}
				{#if suggestion.value}
					<div class="stack tight">
						<div class="spread"><strong class="small">Suggested reads</strong>{#if suggestion.value.reads.length}<button class="small" onclick={addReads}>Use them</button>{/if}</div>
						<ul class="mono small plain">{#each suggestion.value.reads as r (r)}<li>{r}</li>{:else}<li class="muted">None</li>{/each}</ul>
						{#if suggestion.value.leftOut.length}
							<span class="small muted">Left out, sensitive (ask for them explicitly): <span class="mono">{suggestion.value.leftOut.join(', ')}</span></span>
						{/if}
					</div>
				{/if}
				{#if minted.error}<ErrorBox error={minted.error} />{/if}
				{#if minted.value}
					<div class="stack tight">
						<div class="spread"><strong class="small">Token for {minted.value.inspect.task}</strong><Copy text={minted.value.token} /></div>
						<pre class="token">{minted.value.token}</pre>
						<span class="small muted">Minted and written to the audit log. Give it to the agent as <code>ONUS_TOKEN</code>.</span>
					</div>
				{/if}
			</div>
		</Card>

		<Card title="Inspect a token" subtitle="Verified against the root public key.">
			<div class="stack">
				<textarea rows="4" class="mono" bind:value={token} placeholder="Paste a token" spellcheck="false"></textarea>
				<div class="row"><button onclick={() => inspected.run(() => api('token.inspect', { token }))} disabled={!token.trim()}>Inspect</button></div>
				{#if inspected.error}<ErrorBox error={inspected.error} />{/if}
				{#if inspected.value}
					{@const t = inspected.value}
					<dl>
						<dt>Task</dt><dd>{t.task}</dd>
						<dt>Expires</dt><dd>{date(t.expires)} {#if t.expires * 1000 < Date.now()}<Badge tone="del">expired</Badge>{/if}</dd>
						<dt>Rights</dt>
						<dd><ul class="plain mono small">{#each t.rights as r (r)}<li>{r}</li>{/each}</ul></dd>
						{#if t.attenuations.length}
							<dt>Narrowed by</dt>
							<dd>{#each t.attenuations as a, i (i)}<pre class="small">{a}</pre>{/each}</dd>
						{/if}
					</dl>
					<div class="stack tight">
						<strong class="small">Check a right</strong>
						<div class="row">
							<input class="mono grow" bind:value={right} />
							<button onclick={() => checked.run(() => api('token.check', { token, right }))}>Check</button>
						</div>
						{#if checked.value}
							<div class="row">
								<Badge tone={checked.value.allowed ? 'add' : 'del'}>{checked.value.allowed ? 'allowed' : 'refused'}</Badge>
								<span class="small">{checked.value.reason ?? checked.value.right}</span>
							</div>
						{/if}
						{#if checked.error}<ErrorBox error={checked.error} />{/if}
					</div>
					<div class="stack tight">
						<strong class="small">Narrow it for a sub-agent</strong>
						<textarea rows="3" class="mono" bind:value={only} placeholder="write:path:services/notifications/src/sms/**"></textarea>
						<div class="row"><button onclick={() => narrowed.run(() => api('token.attenuate', { token, only: only.split('\n').map((s) => s.trim()).filter(Boolean) }))} disabled={!only.trim()}>Attenuate</button></div>
						{#if narrowed.error}<ErrorBox error={narrowed.error} />{/if}
						{#if narrowed.value}
							<div class="spread"><span class="small muted">The narrower token</span><Copy text={narrowed.value.token} /></div>
							<pre class="token">{narrowed.value.token}</pre>
						{/if}
					</div>
				{/if}
			</div>
		</Card>
	</div>

	<Card title="Enforcement">
		<ul class="small">
			<li><strong>Git gateway:</strong> <code>onus gateway serve --repo . --remote origin</code> gives each task a mirror holding only what it may read and refuses pushes outside its writes.</li>
			<li><strong>The map:</strong> <code>onus mcp --token "$ONUS_TOKEN" --key-public root.pub</code> answers only about files the task may read.</li>
			<li><strong>Environments:</strong> the token's hosts and secrets are all an environment gets.</li>
		</ul>
	</Card>
</div>

<style>
	.plain {
		list-style: none;
		margin: 0;
		padding: 0;
		display: grid;
		gap: 2px;
	}
	.token {
		white-space: pre-wrap;
		word-break: break-all;
		max-height: 160px;
	}
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
		min-width: 0;
	}
	.grow {
		flex: 1;
	}
</style>
