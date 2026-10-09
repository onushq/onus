<script lang="ts">
	import { Button } from '$lib/components/ui/button/index.js';
	import { Input } from '$lib/components/ui/input/index.js';
	import { Textarea } from '$lib/components/ui/textarea/index.js';
	import { api } from '#lib/api.ts';
	import Badge from '#lib/components/Badge.svelte';
	import Card from '#lib/components/Card.svelte';
	import Copy from '#lib/components/Copy.svelte';
	import ErrorBox from '#lib/components/ErrorBox.svelte';
	import Field from '#lib/components/Field.svelte';
	import Loading from '#lib/components/Loading.svelte';
	import PageHead from '#lib/components/PageHead.svelte';
	import { date } from '#lib/format.ts';
	import { Task } from '#lib/task.svelte.ts';
	import type { KeysStatus, TokenInfo } from '#lib/types.ts';
	import BadgeCheck from '@lucide/svelte/icons/badge-check';
	import Funnel from '@lucide/svelte/icons/funnel';
	import KeyRound from '@lucide/svelte/icons/key-round';
	import Lightbulb from '@lucide/svelte/icons/lightbulb';
	import ScanSearch from '@lucide/svelte/icons/scan-search';
	import ShieldCheck from '@lucide/svelte/icons/shield-check';
	import { onMount } from 'svelte';
	import { toast } from 'svelte-sonner';

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
			toast.success(`Token minted for ${m.inspect.task}`, { description: 'Written to the audit log.' });
		}
	}

	async function generate() {
		const k = await keys.run(() => api<KeysStatus>('keys.generate'));
		if (k) toast.success('Root key pair created', { description: k.dir });
	}
</script>

<PageHead title="Tokens & scopes" guide="scopes">
	A task gets exactly the paths, hosts and secrets it needs, as a signed token anyone can narrow and nobody can widen without the root key. The git gateway and the map server enforce it.
</PageHead>

<Card>
	{#if keys.value}
		<div class="flex flex-wrap items-center gap-4">
			<div class="flex size-10 items-center justify-center rounded-lg bg-muted"><KeyRound class="size-5" /></div>
			<div class="grid min-w-0 flex-1 gap-1">
				<div class="flex flex-wrap items-center gap-2">
					<span class="font-medium">Root key</span>
					<Badge tone={keys.value.private ? 'add' : 'faint'}>{keys.value.private ? 'private key present' : 'no private key'}</Badge>
					<Badge tone={keys.value.public ? 'add' : 'faint'}>{keys.value.public ? 'public key present' : 'no public key'}</Badge>
				</div>
				<span class="truncate font-mono text-xs text-muted-foreground">{keys.value.publicKey ?? keys.value.dir}</span>
			</div>
			{#if !keys.value.private && !keys.value.public}
				<Button onclick={generate}><KeyRound />Generate a key pair</Button>
			{:else}
				<span class="text-xs text-muted-foreground">Keep the private key with whoever approves tasks. <code>onus ui --keys &lt;dir&gt;</code> uses another pair.</span>
			{/if}
		</div>
	{:else if keys.error}<ErrorBox error={keys.error} />{:else}<Loading />{/if}
</Card>

<div class="grid gap-4 xl:grid-cols-2">
	<Card title="Plan a task" subtitle="Writes are exactly what the plan says; components and contracts resolve through the map.">
		<div class="grid gap-3">
			<Textarea rows={12} class="font-mono text-xs" bind:value={plan} spellcheck={false} />
			<div class="flex flex-wrap gap-2">
				<Button variant="outline" onclick={() => suggestion.run(() => api('scope.suggest', { plan }))} disabled={suggestion.running}><Lightbulb />Suggest reads</Button>
				<Button onclick={mint} disabled={minted.running || !keys.value?.private}><BadgeCheck />Mint the token</Button>
			</div>
			{#if suggestion.error}<ErrorBox error={suggestion.error} />{/if}
			{#if suggestion.value}
				<div class="grid gap-2 rounded-lg border p-3">
					<div class="flex items-center justify-between gap-2">
						<span class="text-sm font-medium">Suggested reads</span>
						{#if suggestion.value.reads.length}<Button variant="outline" size="xs" onclick={addReads}>Use them</Button>{/if}
					</div>
					<ul class="grid gap-0.5 font-mono text-xs">{#each suggestion.value.reads as r (r)}<li>{r}</li>{:else}<li class="text-muted-foreground">None</li>{/each}</ul>
					{#if suggestion.value.leftOut.length}
						<p class="text-xs text-muted-foreground">Left out as sensitive; ask for them explicitly: <span class="font-mono">{suggestion.value.leftOut.join(', ')}</span></p>
					{/if}
				</div>
			{/if}
			{#if minted.error}<ErrorBox error={minted.error} />{/if}
			{#if minted.value}
				<div class="grid gap-2 rounded-lg border border-success/40 bg-success-soft/40 p-3">
					<div class="flex items-center justify-between gap-2">
						<span class="text-sm font-medium">Token for {minted.value.inspect.task}</span>
						<Copy text={minted.value.token} />
					</div>
					<pre class="max-h-32 bg-background/60 break-all whitespace-pre-wrap">{minted.value.token}</pre>
					<span class="text-xs text-muted-foreground">Give it to the agent as <code>ONUS_TOKEN</code>.</span>
				</div>
			{/if}
		</div>
	</Card>

	<Card title="Inspect a token" subtitle="Verified against the root public key.">
		<div class="grid gap-3">
			<Textarea rows={4} class="font-mono text-xs" bind:value={token} placeholder="Paste a token" spellcheck={false} />
			<div><Button variant="outline" onclick={() => inspected.run(() => api('token.inspect', { token }))} disabled={!token.trim()}><ScanSearch />Inspect</Button></div>
			{#if inspected.error}<ErrorBox error={inspected.error} />{/if}
			{#if inspected.value}
				{@const t = inspected.value}
				<dl class="grid grid-cols-[max-content_minmax(0,1fr)] gap-x-4 gap-y-2 text-sm">
					<dt class="text-muted-foreground">Task</dt><dd class="font-medium">{t.task}</dd>
					<dt class="text-muted-foreground">Expires</dt><dd class="flex items-center gap-2">{date(t.expires)} {#if t.expires * 1000 < Date.now()}<Badge tone="del">expired</Badge>{/if}</dd>
					<dt class="text-muted-foreground">Rights</dt>
					<dd class="flex flex-wrap gap-1">{#each t.rights as r (r)}<code class="text-xs">{r}</code>{/each}</dd>
					{#if t.attenuations.length}
						<dt class="text-muted-foreground">Narrowed by</dt>
						<dd>{#each t.attenuations as a, i (i)}<pre class="text-[11px]">{a}</pre>{/each}</dd>
					{/if}
				</dl>
				<div class="grid gap-2 border-t pt-3">
					<span class="flex items-center gap-1.5 text-sm font-medium"><ShieldCheck class="size-4" />Check a right</span>
					<div class="flex gap-2">
						<Input class="font-mono text-xs" bind:value={right} />
						<Button variant="outline" onclick={() => checked.run(() => api('token.check', { token, right }))}>Check</Button>
					</div>
					{#if checked.value}
						<div class="flex items-center gap-2 text-sm">
							<Badge tone={checked.value.allowed ? 'add' : 'del'}>{checked.value.allowed ? 'allowed' : 'refused'}</Badge>
							<span class="text-muted-foreground">{checked.value.reason ?? checked.value.right}</span>
						</div>
					{/if}
					{#if checked.error}<ErrorBox error={checked.error} />{/if}
				</div>
				<div class="grid gap-2 border-t pt-3">
					<span class="flex items-center gap-1.5 text-sm font-medium"><Funnel class="size-4" />Narrow it for a sub-agent</span>
					<Textarea rows={3} class="font-mono text-xs" bind:value={only} placeholder="write:path:services/notifications/src/sms/**" />
					<div><Button variant="outline" onclick={() => narrowed.run(() => api('token.attenuate', { token, only: only.split('\n').map((s) => s.trim()).filter(Boolean) }))} disabled={!only.trim()}>Attenuate</Button></div>
					{#if narrowed.error}<ErrorBox error={narrowed.error} />{/if}
					{#if narrowed.value}
						<div class="flex items-center justify-between"><span class="text-xs text-muted-foreground">The narrower token</span><Copy text={narrowed.value.token} /></div>
						<pre class="max-h-32 break-all whitespace-pre-wrap">{narrowed.value.token}</pre>
					{/if}
				</div>
			{/if}
		</div>
	</Card>
</div>

<Card title="Where tokens are enforced">
	<div class="grid gap-4 text-sm md:grid-cols-3">
		<div class="grid gap-1"><span class="font-medium">Git gateway</span><code class="w-fit text-xs">onus gateway serve --repo . --remote origin</code><span class="text-muted-foreground">A mirror per task holding only what it may read; pushes outside its writes are refused.</span></div>
		<div class="grid gap-1"><span class="font-medium">The map</span><code class="w-fit text-xs">onus mcp --token "$ONUS_TOKEN"</code><span class="text-muted-foreground">Answers only about files the task may read.</span></div>
		<div class="grid gap-1"><span class="font-medium">Environments</span><span class="text-muted-foreground">The token's hosts and secrets are all an environment gets.</span></div>
	</div>
</Card>
