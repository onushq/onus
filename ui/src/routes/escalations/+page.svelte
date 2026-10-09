<script lang="ts">
	import { Button } from '$lib/components/ui/button/index.js';
	import * as Collapsible from '$lib/components/ui/collapsible/index.js';
	import { Input } from '$lib/components/ui/input/index.js';
	import { Textarea } from '$lib/components/ui/textarea/index.js';
	import { api } from '#lib/api.ts';
	import Badge from '#lib/components/Badge.svelte';
	import Card from '#lib/components/Card.svelte';
	import Copy from '#lib/components/Copy.svelte';
	import Empty from '#lib/components/Empty.svelte';
	import ErrorBox from '#lib/components/ErrorBox.svelte';
	import Field from '#lib/components/Field.svelte';
	import Inline from '#lib/components/Inline.svelte';
	import Loading from '#lib/components/Loading.svelte';
	import NativeSelect from '#lib/components/NativeSelect.svelte';
	import PageHead from '#lib/components/PageHead.svelte';
	import { ago } from '#lib/format.ts';
	import { Task } from '#lib/task.svelte.ts';
	import type { EscalationItem } from '#lib/types.ts';
	import Ban from '@lucide/svelte/icons/ban';
	import Check from '@lucide/svelte/icons/check';
	import ChevronDown from '@lucide/svelte/icons/chevron-down';
	import ChevronRight from '@lucide/svelte/icons/chevron-right';
	import Scale from '@lucide/svelte/icons/scale';
	import Send from '@lucide/svelte/icons/send';
	import { onMount } from 'svelte';
	import { toast } from 'svelte-sonner';

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
			toast.success('Request filed');
			refresh();
		}
	}

	async function run(name: string, body: Record<string, unknown>, done: string) {
		const r = await acted.run(() => api(name, body));
		if (r) {
			toast.success(done);
			refresh();
		}
	}

	const grade = (n: number) => ['', 'failing test', 'trace', 'map path', 'draft diff', 'rationale'][n] ?? `grade ${n}`;
	const decisionTone = (d: string) => (d === 'granted' ? 'add' : d === 'denied' ? 'del' : 'signal');
	const open = $derived((list.value?.requests ?? []).filter((r) => !r.decision || r.decision.decision === 'needs-person').length);
</script>

<PageHead title="Escalations" guide="scopes">
	When a task needs more than its token allows, it asks with evidence. Low-risk requests with a reproduced failing test are granted by policy; everything else goes to a person.
</PageHead>

<div class="grid gap-4 xl:grid-cols-[minmax(0,1fr)_400px]">
	<Card title="Requests" subtitle={list.value ? `${list.value.requests.length} on file, ${open} waiting · ${list.value.dir}` : undefined} pad={false} class="self-start">
		{#if list.error}
			<div class="p-4"><ErrorBox error={list.error} /></div>
		{:else if !list.value}
			<div class="p-4"><Loading /></div>
		{:else if !list.value.requests.length}
			<div class="p-4"><Empty title="No requests">Agents file them with <code>onus escalate</code>; you can file one here.</Empty></div>
		{:else}
			{#each list.value.requests as item (item.request.id)}
				{@const r = item.request}
				<div class="border-b last:border-b-0">
					<button class="flex w-full items-center gap-3 px-4 py-3 text-left hover:bg-muted/40" onclick={() => (openId = openId === r.id ? null : r.id)}>
						<ChevronRight class="size-4 shrink-0 text-muted-foreground transition-transform {openId === r.id ? 'rotate-90' : ''}" />
						<div class="grid min-w-0 flex-1 gap-0.5">
							<span class="font-medium">{r.task} <span class="font-normal text-muted-foreground">· {r.kind}</span></span>
							<span class="truncate font-mono text-xs text-muted-foreground">{r.scopes.join(', ') || r.reason}</span>
						</div>
						<div class="flex shrink-0 items-center gap-1.5">
							{#if r.sensitive.length}<Badge tone="signal">{r.sensitive.join(', ')}</Badge>{/if}
							<Badge tone="faint">blast {r.blastRadius}</Badge>
							{#if item.decision}<Badge tone={decisionTone(item.decision.decision)}>{item.decision.decision}</Badge>{:else}<Badge tone="info">open</Badge>{/if}
							<span class="hidden w-16 text-right text-xs text-muted-foreground sm:inline">{ago(r.at)}</span>
						</div>
					</button>
					{#if openId === r.id}
						<div class="grid gap-4 bg-muted/20 px-11 pt-1 pb-4">
							<p class="text-sm">{r.reason}</p>
							<ul class="grid gap-1.5 text-sm">
								{#each r.evidence as e, i (i)}
									<li class="flex flex-wrap items-center gap-2">
										<Badge tone={e.grade === 1 ? 'add' : 'faint'}>{grade(e.grade)}</Badge>
										<span class="font-mono text-xs">{e.reference}</span>
										{#if e.reproduced !== undefined && e.reproduced !== null}<Badge tone={e.reproduced ? 'add' : 'del'}>{e.reproduced ? 'reproduced' : 'not reproduced'}</Badge>{/if}
									</li>
								{:else}<li class="text-muted-foreground">No evidence</li>{/each}
							</ul>
							{#if item.decision}<p class="text-xs text-muted-foreground">{item.decision.decision} by {item.decision.by}: {item.decision.reason}</p>{/if}
							<div class="grid gap-3 md:grid-cols-2">
								<Field label="The task's token"><Input class="font-mono" bind:value={act.token} /></Field>
								<Field label="You (for a person's decision)"><Input bind:value={act.by} placeholder="@team-orders" /></Field>
							</div>
							<Collapsible.Root>
								<Collapsible.Trigger class="group flex items-center gap-1 text-xs text-muted-foreground hover:text-foreground"><ChevronDown class="size-3.5 transition-transform group-data-[state=open]:rotate-180" />How to reproduce failing tests</Collapsible.Trigger>
								<Collapsible.Content class="mt-2 grid gap-3 md:grid-cols-2">
									<Field label="At"><Input class="font-mono" bind:value={act.reproduceAt} /></Field>
									<Field label="Image"><Input class="font-mono" bind:value={act.image} /></Field>
									<Field label="Setup"><Input class="font-mono" bind:value={act.setup} placeholder="npm ci" /></Field>
									<Field label="Test command"><Input class="font-mono" bind:value={act.testCommand} /></Field>
								</Collapsible.Content>
							</Collapsible.Root>
							<div class="flex flex-wrap items-center gap-2">
								<Button variant="outline" disabled={!act.token || acted.running} onclick={() => run('escalations.decide', { id: r.id, token: act.token, reproduceAt: act.reproduceAt, image: act.image, setup: act.setup, testCommand: act.testCommand }, 'Decided by policy')}><Scale />Decide by policy</Button>
								<Button disabled={!act.token || !act.by || acted.running} onclick={() => run('escalations.grant', { id: r.id, token: act.token, by: act.by }, 'Granted')}><Check />Grant</Button>
								<Input class="w-48" bind:value={act.reason} placeholder="Why not" />
								<Button variant="destructive" disabled={!act.by || !act.reason || acted.running} onclick={() => run('escalations.deny', { id: r.id, by: act.by, reason: act.reason }, 'Denied and logged')}><Ban />Deny</Button>
							</div>
							{#if acted.running}<Loading label="Deciding; reproducing a failing test runs a container…" />{/if}
							{#if acted.error}<ErrorBox error={acted.error} />{/if}
							{#if acted.value}
								{#if acted.value.decision}
									<div class="flex flex-wrap items-center gap-2 text-sm"><Badge tone={decisionTone(acted.value.decision)}>{acted.value.decision}</Badge>{#each acted.value.reasons ?? [] as reason (reason)}<span class="text-muted-foreground"><Inline text={reason} /></span>{/each}</div>
								{/if}
								{#if acted.value.token}
									<div class="flex items-center justify-between"><span class="text-xs text-muted-foreground">The new token: the original rights plus exactly what was asked</span><Copy text={acted.value.token} /></div>
									<pre class="max-h-32 break-all whitespace-pre-wrap">{acted.value.token}</pre>
								{/if}
								{#if acted.value.denied}<p class="text-sm text-muted-foreground">Denied and logged.</p>{/if}
							{/if}
						</div>
					{/if}
				</div>
			{/each}
		{/if}
	</Card>

	<Card title="Ask for more" subtitle="Evidence, strongest first: failing-test:, trace:, map-path:, draft-diff:, rationale:" class="self-start">
		<form class="grid gap-3" onsubmit={create}>
			<div class="grid grid-cols-2 gap-3">
				<Field label="Task"><Input bind:value={form.task} required /></Field>
				<Field label="Kind">
					<NativeSelect bind:value={form.kind}>
						<option value="permission">permission</option>
						<option value="broken-test">broken test</option>
						<option value="contradictory-spec">contradictory spec</option>
						<option value="impossible-task">impossible task</option>
					</NativeSelect>
				</Field>
			</div>
			<Field label="Why"><Input bind:value={form.reason} required /></Field>
			<Field label="Rights asked for, one per line"><Textarea rows={3} class="font-mono text-xs" bind:value={form.scopes} placeholder="write:path:services/orders/src/**" /></Field>
			<Field label="Evidence, one per line"><Textarea rows={3} class="font-mono text-xs" bind:value={form.evidence} placeholder="failing-test:services/orders/src/ship.test.ts" /></Field>
			<div><Button type="submit" disabled={created.running}><Send />File the request</Button></div>
			{#if created.error}<ErrorBox error={created.error} />{/if}
		</form>
	</Card>
</div>
