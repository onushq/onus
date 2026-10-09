<script lang="ts">
	import { Button } from '$lib/components/ui/button/index.js';
	import Card from '#lib/components/Card.svelte';
	import Field from '#lib/components/Field.svelte';
	import type { Draft } from '#lib/config/draft.svelte.ts';
	import ArrowRight from '@lucide/svelte/icons/arrow-right';
	import Plus from '@lucide/svelte/icons/plus';
	import Trash from '@lucide/svelte/icons/trash-2';
	import TextInput from './TextInput.svelte';

	let { draft, componentIds }: { draft: Draft; componentIds: string[] } = $props();

	const rules = $derived(draft.data.rules ?? []);
	const listId = 'rule-targets';
</script>

<Card title="Boundary rules" subtitle="Dependencies that must not exist. A change that adds one is reported as breaking a rule. “billing.internal” means billing’s files outside its public surface.">
	<datalist id={listId}>
		{#each componentIds as c (c)}<option value={c}></option><option value="{c}.internal"></option>{/each}
	</datalist>
	<div class="grid gap-2">
		{#each rules as r, i (i)}
			<div class="grid items-end gap-2 rounded-lg border p-2 pl-3 sm:grid-cols-[minmax(0,1fr)_auto_minmax(0,1fr)_auto]">
				<Field label="Deny from"><TextInput mono list={listId} value={r?.deny?.from ?? ''} onchange={(v) => draft.set(['rules', i, 'deny', 'from'], v)} /></Field>
				<ArrowRight class="mb-2 hidden size-4 text-muted-foreground sm:block" />
				<Field label="to"><TextInput mono list={listId} value={r?.deny?.to ?? ''} onchange={(v) => draft.set(['rules', i, 'deny', 'to'], v)} /></Field>
				<Button variant="ghost" size="icon-sm" aria-label="Remove rule" onclick={() => draft.set(['rules', i], undefined)}><Trash /></Button>
			</div>
		{/each}
		<div>
			<Button
				variant="outline"
				size="sm"
				disabled={componentIds.length < 2}
				onclick={() => draft.set(['rules', rules.length], { deny: { from: componentIds[0], to: `${componentIds[1]}.internal` } })}><Plus />Add rule</Button
			>
		</div>
	</div>
</Card>
