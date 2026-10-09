<script lang="ts">
	import { page } from '$app/state';
	import { Button } from '$lib/components/ui/button/index.js';
	import { Checkbox } from '$lib/components/ui/checkbox/index.js';
	import * as Collapsible from '$lib/components/ui/collapsible/index.js';
	import { Input } from '$lib/components/ui/input/index.js';
	import * as Table from '$lib/components/ui/table/index.js';
	import { Textarea } from '$lib/components/ui/textarea/index.js';
	import { api } from '#lib/api.ts';
	import { app } from '#lib/app.svelte.ts';
	import Badge from '#lib/components/Badge.svelte';
	import Card from '#lib/components/Card.svelte';
	import ChangeRow from '#lib/components/ChangeRow.svelte';
	import ClassificationView from '#lib/components/ClassificationView.svelte';
	import Empty from '#lib/components/Empty.svelte';
	import ErrorBox from '#lib/components/ErrorBox.svelte';
	import Field from '#lib/components/Field.svelte';
	import Inline from '#lib/components/Inline.svelte';
	import LaneBadge from '#lib/components/LaneBadge.svelte';
	import Loading from '#lib/components/Loading.svelte';
	import PageHead from '#lib/components/PageHead.svelte';
	import RefPicker from '#lib/components/RefPicker.svelte';
	import { ago, download, short } from '#lib/format.ts';
	import { Task } from '#lib/task.svelte.ts';
	import type { Classification, Judgment, Lane, LanesPolicy, Run, Submission } from '#lib/types.ts';
	import ChevronDown from '@lucide/svelte/icons/chevron-down';
	import CircleCheck from '@lucide/svelte/icons/circle-check';
	import CircleDashed from '@lucide/svelte/icons/circle-dashed';
	import CircleX from '@lucide/svelte/icons/circle-x';
	import Download from '@lucide/svelte/icons/download';
	import Gavel from '@lucide/svelte/icons/gavel';
	import MessageCircleWarning from '@lucide/svelte/icons/message-circle-warning';
	import NotebookPen from '@lucide/svelte/icons/notebook-pen';
	import Send from '@lucide/svelte/icons/send';
	import { onMount } from 'svelte';

	const policy = new Task<LanesPolicy>();
	const runs = new Task<{ runs: Run[] }>();
	const submission = new Task<Submission>();
	const classification = new Task<Classification>();
	const judgment = new Task<{ classification: Classification; judgment: Judgment }>();
	const saved = new Task<{ dir: string; submissions: SavedSubmission[] }>();
	let savedId = $state('');

	interface SavedSubmission {
		id: string;
		at: number;
		base: string;
		head: string;
		agent: { tool: string; model: string; config: string };
		intent?: string | null;
		rows: number;
		needsPerson: number;
		evidence: number;
		task?: string | null;
		verdict?: string | null;
		lane?: Lane | null;
	}

	let base = $state(page.url.searchParams.get('base') ?? '');
	let head = $state(page.url.searchParams.get('head') ?? 'HEAD');
	let intent = $state('');
	let token = $state('');
	let evidence = $state<string[]>([]);
	let approvals = $state('');
	let escalations = $state('');
	let agent = $state({ tool: 'person', model: '', config: '', team: '' });

	onMount(() => {
		policy.run(() => api<LanesPolicy>('lanes.policy'));
		saved.run(() => api('submissions.list'));
		runs.run(() => api('evidence.list'));
		app.loadRefs().then((r) => {
			if (!base) base = r?.default ?? 'main';
		});
	});

	const lines = (s: string) => s.split('\n').map((l) => l.trim()).filter(Boolean);

	function toggle(id: string, on: boolean) {
		evidence = on ? [...evidence, id] : evidence.filter((e) => e !== id);
	}

	async function openSaved(id: string) {
		const r = await api<{ submission: Submission; judgment: { classification: Classification; judgment: Judgment } | null }>('submissions.show', { id });
		savedId = id;
		submission.value = r.submission;
		judgment.reset();
		if (r.judgment) judgment.value = r.judgment;
		classification.run(() => api<Classification>('lanes.classify', { submission: r.submission }));
		document.getElementById('submission')?.scrollIntoView({ behavior: 'smooth' });
	}

	async function submit(e: SubmitEvent) {
		e.preventDefault();
		savedId = '';
		classification.reset();
		judgment.reset();
		const sub = await submission.run(() =>
			api<Submission>('lanes.submit', {
				base,
				head,
				intent: intent.trim() || undefined,
				evidence,
				token: token.trim() || undefined,
				approvals: lines(approvals),
				escalations: lines(escalations),
				agent
			})
		);
		if (sub) classification.run(() => api<Classification>('lanes.classify', { submission: sub }));
	}

	async function judge() {
		const sub = submission.value;
		if (!sub) return;
		// A submission an agent handed in keeps its verdict beside it.
		const r = await judgment.run(() => (savedId ? api('submissions.judge', { id: savedId }) : api('lanes.judge', { submission: sub })));
		if (r && savedId) saved.run(() => api('submissions.list'));
	}

	const p = $derived(policy.value?.policy);
	const outcomeHref = $derived.by(() => {
		const j = judgment.value?.judgment;
		const sub = submission.value;
		if (!sub) return '/outcomes';
		const q = new URLSearchParams({
			change: short(sub.head, 12),
			agent: `${sub.agent.tool}/${sub.agent.model}/${sub.agent.config}`,
			lane: j?.lane ?? classification.value?.lane ?? 'judge',
			verdict: j?.verdict ?? '',
			judge: j?.judge ?? '',
			commit: sub.head
		});
		return `/outcomes?${q}`;
	});
	const verdictTone = (v: string) => (v === 'approve' ? 'add' : v === 'reject' ? 'del' : 'signal');
</script>

<PageHead title="Lanes & judge" guide="lanes">
	Where a change goes: merge on its own, the verifying judge, a person, or nowhere. Rules decide, and floors no rule can lower; nothing about a lane is generated.
</PageHead>

<Card title="Lane policy" subtitle={policy.value ? `From onus.yaml · judge configuration ${policy.value.judge}` : undefined}>
	{#if policy.error}
		<ErrorBox error={policy.error} />
	{:else if !p}
		<Loading />
	{:else}
		{#if !policy.value?.configured}
			<p class="mb-3 text-sm text-muted-foreground">onus.yaml has no <code>lanes:</code> yet, so every change goes to the judge. A starting point:</p>
			<pre>{`lanes:
  default: judge
  auditRate: 0.05
  minRecord: 10
  rules:
    - { lane: auto-merge, match: every, kinds: [internal], components: [docs] }
    - { lane: human, subkinds: [migration-changed] }
  heldOut: { command: "npm run test:held-out", image: "node:22", setup: "npm ci" }`}</pre>
		{:else}
			<div class="grid grid-cols-2 gap-4 text-sm md:grid-cols-5">
				<div class="grid gap-1"><span class="text-xs text-muted-foreground">Default lane</span><LaneBadge lane={p.default ?? 'judge'} /></div>
				<div class="grid gap-1"><span class="text-xs text-muted-foreground">Audited auto-merges</span><span class="font-medium">{((p.auditRate ?? 0) * 100).toFixed(1)}%</span></div>
				<div class="grid gap-1"><span class="text-xs text-muted-foreground">Record before auto-merge</span><span class="font-medium">{p.minRecord ?? 0} merged changes</span></div>
				<div class="grid gap-1"><span class="text-xs text-muted-foreground">Held-out checks</span><code class="w-fit text-xs">{p.heldOut?.command ?? 'none'}</code></div>
				<div class="grid gap-1"><span class="text-xs text-muted-foreground">Taste reviewer</span><code class="w-fit text-xs">{p.taste?.command.join(' ') ?? 'none'}</code></div>
			</div>
			{#if p.rules?.length}
				<div class="mt-4 overflow-hidden rounded-lg border">
					<Table.Root>
						<Table.Header><Table.Row><Table.Head class="pl-3">Lane</Table.Head><Table.Head>Match</Table.Head><Table.Head>Kinds</Table.Head><Table.Head>Subkinds</Table.Head><Table.Head>Components</Table.Head><Table.Head class="pr-3">Labels</Table.Head></Table.Row></Table.Header>
						<Table.Body>
							{#each p.rules as r, i (i)}
								<Table.Row>
									<Table.Cell class="pl-3"><LaneBadge lane={r.lane} /></Table.Cell>
									<Table.Cell>{r.match ?? 'any'}</Table.Cell>
									<Table.Cell class="font-mono text-xs">{r.kinds?.join(', ') || 'any'}</Table.Cell>
									<Table.Cell class="font-mono text-xs">{r.subkinds?.join(', ') || 'any'}</Table.Cell>
									<Table.Cell class="font-mono text-xs">{r.components?.join(', ') || 'any'}</Table.Cell>
									<Table.Cell class="pr-3 font-mono text-xs">{r.labels?.join(', ') || 'any'}</Table.Cell>
								</Table.Row>
							{/each}
						</Table.Body>
					</Table.Root>
				</div>
			{/if}
		{/if}
		<p class="mt-4 text-xs text-muted-foreground">
			Floors: secrets, writes outside the token's scope and unapproved weakened tests are <strong class="text-foreground">blocked</strong>; sensitive code, rules of the game and rows outside the intent go to <strong class="text-foreground">a person</strong>.
		</p>
	{/if}
</Card>

{#if saved.value?.submissions.length}
	<Card title="Submitted by agents" subtitle="Handed in with onus_submit over MCP; open one to classify and judge it here." pad={false}>
		<Table.Root>
			<Table.Header><Table.Row><Table.Head class="pl-4">Submission</Table.Head><Table.Head>Agent</Table.Head><Table.Head>Task</Table.Head><Table.Head>Change</Table.Head><Table.Head class="text-right">Rows</Table.Head><Table.Head class="text-right">Evidence</Table.Head><Table.Head>Verdict</Table.Head><Table.Head class="pr-4"></Table.Head></Table.Row></Table.Header>
			<Table.Body>
				{#each saved.value.submissions as s (s.id)}
					<Table.Row class={savedId === s.id ? 'bg-muted/60' : ''}>
						<Table.Cell class="pl-4"><span class="font-mono text-xs">{s.id}</span><div class="text-xs text-muted-foreground">{ago(s.at)}</div></Table.Cell>
						<Table.Cell class="font-mono text-xs">{s.agent.tool}{s.agent.model ? `/${s.agent.model}` : ''}</Table.Cell>
						<Table.Cell class="text-xs">{s.task ?? '–'}</Table.Cell>
						<Table.Cell class="font-mono text-xs">{short(s.base)} → {short(s.head)}</Table.Cell>
						<Table.Cell class="text-right tabular-nums">{s.rows}{#if s.needsPerson}<span class="ml-1 text-xs text-signal-foreground">({s.needsPerson} person)</span>{/if}</Table.Cell>
						<Table.Cell class="text-right tabular-nums">{s.evidence}</Table.Cell>
						<Table.Cell>{#if s.verdict}<Badge tone={verdictTone(s.verdict)}>{s.verdict}</Badge>{:else}<span class="text-xs text-muted-foreground">not judged</span>{/if}</Table.Cell>
						<Table.Cell class="pr-4 text-right"><Button variant="outline" size="xs" onclick={() => openSaved(s.id)}>Open</Button></Table.Cell>
					</Table.Row>
				{/each}
			</Table.Body>
		</Table.Root>
	</Card>
{/if}

<Card title="Submit a change" subtitle="What a reviewer needs and nothing of the author's reasoning.">
	<form class="grid gap-5" onsubmit={submit}>
		<div class="grid gap-3 md:grid-cols-2">
			<RefPicker id="lane-base" label="Base" bind:value={base} />
			<RefPicker id="lane-head" label="Head" bind:value={head} />
		</div>
		<Field hint="YAML, or Markdown with an onus-intent block.">
			{#snippet label()}Intent{/snippet}
			<Textarea rows={4} class="font-mono text-xs" bind:value={intent} placeholder={'summary: Redact digits in logs\ntouches: [logger]'} />
		</Field>
		<div class="grid gap-2">
			<span class="text-xs font-medium text-muted-foreground">Evidence: runs recorded in environments ({evidence.length} selected)</span>
			{#if runs.value?.runs.length}
				<div class="max-h-60 divide-y overflow-auto rounded-lg border">
					{#each runs.value.runs.slice(0, 40) as r (r.id)}
						<label class="flex cursor-pointer items-center gap-3 px-3 py-2 text-sm hover:bg-muted/40">
							<Checkbox checked={evidence.includes(r.id)} onCheckedChange={(v) => toggle(r.id, v === true)} />
							<Badge tone={r.manifest.exitCode === 0 ? 'add' : 'del'}>exit {r.manifest.exitCode}</Badge>
							<code class="min-w-0 flex-1 truncate bg-transparent p-0 text-xs">{r.manifest.command}</code>
							{#if r.manifest.tests}<span class="hidden text-xs text-muted-foreground sm:inline">{r.manifest.tests.tests} tests</span>{/if}
							<span class="hidden shrink-0 font-mono text-xs text-muted-foreground md:inline">{short(r.manifest.commit)} · {ago(r.manifest.finishedAt)}</span>
						</label>
					{/each}
				</div>
			{:else}
				<p class="rounded-lg border border-dashed p-3 text-sm text-muted-foreground">No runs yet. Record some in <a class="underline" href="/environments">Environments</a>; the judge rejects a change without evidence.</p>
			{/if}
		</div>
		<div class="grid gap-3 sm:grid-cols-2 lg:grid-cols-4">
			<Field label="Agent tool"><Input bind:value={agent.tool} placeholder="claude-code, codex, person…" /></Field>
			<Field label="Model"><Input bind:value={agent.model} /></Field>
			<Field label="Configuration"><Input bind:value={agent.config} /></Field>
			<Field label="Team"><Input bind:value={agent.team} /></Field>
		</div>
		<Collapsible.Root>
			<Collapsible.Trigger class="group flex items-center gap-1 text-sm text-muted-foreground hover:text-foreground">
				<ChevronDown class="size-4 transition-transform group-data-[state=open]:rotate-180" />Token, approvals and escalations
			</Collapsible.Trigger>
			<Collapsible.Content class="mt-3 grid gap-3">
				<Field label="Task token" hint="Its verified scope is checked against every path the change writes."><Input class="font-mono" bind:value={token} /></Field>
				<div class="grid gap-3 md:grid-cols-2">
					<Field label="Approvals, one row id=who per line"><Textarea rows={3} class="font-mono text-xs" bind:value={approvals} /></Field>
					<Field label="Granted escalations, one id per line"><Textarea rows={3} class="font-mono text-xs" bind:value={escalations} /></Field>
				</div>
			</Collapsible.Content>
		</Collapsible.Root>
		<div><Button type="submit" disabled={submission.running || !base || !head}><Send />{submission.running ? 'Reporting the change…' : 'Make the submission and classify'}</Button></div>
	</form>
</Card>

{#if submission.error}<ErrorBox error={submission.error} />{/if}

{#if submission.value}
	{@const sub = submission.value}
	<div id="submission" class="grid scroll-mt-16 gap-4 xl:grid-cols-[minmax(0,1fr)_minmax(0,1fr)]">
		<Card title="Lane">
			{#if classification.value}<ClassificationView classification={classification.value} />{:else if classification.error}<ErrorBox error={classification.error} />{:else}<Loading />{/if}
		</Card>
		<Card title="Submission">
			{#snippet actions()}
				<Button variant="outline" size="sm" onclick={() => download(`submission-${short(sub.head)}.json`, JSON.stringify(sub, null, 2))}><Download />Download</Button>
			{/snippet}
			<dl class="grid grid-cols-[max-content_minmax(0,1fr)] gap-x-4 gap-y-2 text-sm">
				<dt class="text-muted-foreground">Change</dt><dd class="font-mono text-xs">{short(sub.base)} → {short(sub.head)}</dd>
				<dt class="text-muted-foreground">Rows</dt><dd>{sub.report.changes.length} ({sub.report.summary.needsAttention} need a person)</dd>
				<dt class="text-muted-foreground">Files</dt><dd>{sub.changedFiles.length}</dd>
				<dt class="text-muted-foreground">Evidence</dt><dd>{sub.evidence.length} runs</dd>
				<dt class="text-muted-foreground">Scope</dt><dd>{sub.scope ? `${sub.scope.task}: ${sub.scope.rights.length} rights` : 'no token'}</dd>
				<dt class="text-muted-foreground">Agent</dt><dd class="font-mono text-xs">{sub.agent.tool}/{sub.agent.model}/{sub.agent.config}</dd>
			</dl>
		</Card>
	</div>

	<Card title="The judge" subtitle="Verification first, taste last: re-runs the evidence in fresh containers, checks intent, contracts and tests, runs held-out checks.">
		{#snippet actions()}
			<Button onclick={judge} disabled={judgment.running}><Gavel />{judgment.running ? 'Judging…' : 'Run the judge'}</Button>
		{/snippet}
		{#if judgment.running}
			<Loading label="Re-running evidence and held-out checks in containers; this can take a few minutes…" />
		{:else if judgment.error}
			<ErrorBox error={judgment.error} />
		{:else if judgment.value}
			{@const j = judgment.value.judgment}
			<div class="grid gap-5">
				<div class="flex flex-wrap items-center gap-3">
					<Badge tone={verdictTone(j.verdict)} class="h-7 px-3 text-sm uppercase tracking-wide">{j.verdict}</Badge>
					<span class="flex items-center gap-2 text-sm text-muted-foreground">in the <LaneBadge lane={j.lane} /> lane · judge {j.judge}</span>
					<Button variant="outline" size="sm" class="ml-auto" href={outcomeHref}><NotebookPen />Record the outcome</Button>
				</div>
				{#if j.reasons.length}
					<ul class="grid gap-1.5 rounded-lg bg-muted/50 px-4 py-3 text-sm">{#each j.reasons as r (r)}<li><Inline text={r} /></li>{/each}</ul>
				{/if}
				<ol class="grid gap-0">
					{#each j.steps as s, i (s.name)}
						<li class="relative flex gap-3 pb-4 last:pb-0">
							{#if i < j.steps.length - 1}<span class="absolute top-6 left-[9px] h-[calc(100%-1rem)] w-px bg-border"></span>{/if}
							{#if s.status === 'passed'}<CircleCheck class="size-5 shrink-0 text-success" />
							{:else if s.status === 'failed'}<CircleX class="size-5 shrink-0 text-destructive" />
							{:else if s.status === 'concern'}<MessageCircleWarning class="size-5 shrink-0 text-signal-foreground" />
							{:else}<CircleDashed class="size-5 shrink-0 text-muted-foreground" />{/if}
							<div class="grid gap-1">
								<span class="text-sm font-medium">{s.name[0].toUpperCase() + s.name.slice(1)} <span class="ml-1 text-xs font-normal text-muted-foreground normal-case">{s.status}</span></span>
								{#if s.details.length}<ul class="grid gap-0.5 text-xs text-muted-foreground">{#each s.details as d (d)}<li><Inline text={d} /></li>{/each}</ul>{/if}
							</div>
						</li>
					{/each}
				</ol>
			</div>
		{:else}
			<p class="text-sm text-muted-foreground">The judge needs a container engine (Docker or Podman) to re-run the evidence.</p>
		{/if}
	</Card>

	<Card title="Rows" subtitle="{sub.report.changes.length} changes in meaning" pad={false}>
		{#each sub.report.changes as c (c.id)}<ChangeRow change={c} />{:else}<div class="p-4"><Empty title="No changes in meaning" /></div>{/each}
	</Card>
{/if}
