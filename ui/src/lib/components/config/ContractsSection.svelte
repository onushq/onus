<script lang="ts">
	import { Button } from '$lib/components/ui/button/index.js';
	import Card from '#lib/components/Card.svelte';
	import Field from '#lib/components/Field.svelte';
	import type { Draft } from '#lib/config/draft.svelte.ts';
	import Plus from '@lucide/svelte/icons/plus';
	import Trash from '@lucide/svelte/icons/trash-2';
	import ListInput from './ListInput.svelte';
	import TextInput from './TextInput.svelte';

	let { draft }: { draft: Draft } = $props();

	const contracts = $derived(Object.entries(draft.data.contracts ?? {}));
	let name = $state('');
	let symbol = $state('');
</script>

<Card title="Contracts" subtitle="Shared types and APIs, with the rules they must keep. Changes to them are reported with these invariants beside them.">
	<div class="grid gap-2">
		{#each contracts as [id, c] (id)}
			<div class="grid gap-3 rounded-lg border p-3 md:grid-cols-[minmax(0,10rem)_minmax(0,1fr)_auto]">
				<Field label="Name"><TextInput mono value={id} onchange={(v) => draft.rename(['contracts', id], v)} /></Field>
				<Field label="Symbol" hint="component:path#Name"><TextInput mono value={c?.symbol ?? ''} onchange={(v) => draft.set(['contracts', id, 'symbol'], v)} /></Field>
				<div class="flex items-start justify-end pt-5">
					<Button variant="ghost" size="icon-sm" aria-label="Remove {id}" onclick={() => draft.set(['contracts', id], undefined)}><Trash /></Button>
				</div>
				<Field label="Invariants" class="md:col-span-3">
					<ListInput value={c?.invariants ?? []} onchange={(v) => draft.set(['contracts', id, 'invariants'], v)} placeholder="phone numbers are stored in E.164" />
				</Field>
			</div>
		{/each}
		<form
			class="grid gap-2 sm:grid-cols-[minmax(0,10rem)_minmax(0,1fr)_auto]"
			onsubmit={(e) => {
				e.preventDefault();
				if (name.trim() && symbol.trim() && !draft.data.contracts?.[name.trim()]) draft.set(['contracts', name.trim()], { symbol: symbol.trim() });
				name = '';
				symbol = '';
			}}
		>
			<input class="h-8 rounded-lg border border-input bg-transparent px-2.5 font-mono text-xs dark:bg-input/30" bind:value={name} placeholder="UserPreferences" aria-label="New contract name" />
			<input class="h-8 rounded-lg border border-input bg-transparent px-2.5 font-mono text-xs dark:bg-input/30" bind:value={symbol} placeholder="user-preferences:src/types.ts#UserPreferences" aria-label="Its symbol" />
			<Button type="submit" variant="outline" size="sm" disabled={!name.trim() || !symbol.trim()}><Plus />Add contract</Button>
		</form>
	</div>
</Card>
