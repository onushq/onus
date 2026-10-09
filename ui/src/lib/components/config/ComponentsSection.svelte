<script lang="ts">
	import { Button } from '$lib/components/ui/button/index.js';
	import Card from '#lib/components/Card.svelte';
	import Field from '#lib/components/Field.svelte';
	import NativeSelect from '#lib/components/NativeSelect.svelte';
	import type { Draft } from '#lib/config/draft.svelte.ts';
	import type { ConfigSchema } from '#lib/config/types.ts';
	import Plus from '@lucide/svelte/icons/plus';
	import Trash from '@lucide/svelte/icons/trash-2';
	import ListInput from './ListInput.svelte';
	import TextInput from './TextInput.svelte';

	let { draft, schema }: { draft: Draft; schema?: ConfigSchema } = $props();

	const components = $derived(Object.entries(draft.data.components ?? {}));
	const labels = $derived(Object.keys(draft.data.labels ?? {}).map((l) => ({ value: l, hint: draft.data.labels?.[l]?.sensitivity })));
	const undeclared = $derived((schema?.components ?? []).filter((c) => !draft.data.components?.[c.id]));
	const paths = (p: string | string[] | undefined) => (Array.isArray(p) ? p : p ? [p] : []);
	const onePath = (list: string[]) => (list.length === 1 ? list[0] : list);

	let newId = $state('');
	let newPath = $state('');

	function add() {
		const id = newId.trim();
		if (!id || draft.data.components?.[id]) return;
		draft.set(['components', id], { path: newPath.trim() || `${id}/**` });
		newId = '';
		newPath = '';
	}

	function declare(c: ConfigSchema['components'][number]) {
		const value: Record<string, unknown> = { path: onePath(c.roots) };
		if (c.owners.length) value.owners = c.owners;
		if (c.labels.length) value.labels = c.labels;
		draft.set(['components', c.id], value);
	}
</script>

<Card title="Components" subtitle="The parts of the repository, who owns them and how sensitive they are. Onus infers components from workspaces; declare one to name it, give it owners or labels.">
	<div class="grid gap-3">
		{#each components as [id, c] (id)}
			<div class="grid gap-3 rounded-lg border p-3 md:grid-cols-[10rem_minmax(0,1fr)_9rem_2rem]">
				<Field label="Name"><TextInput value={id} mono onchange={(v) => draft.rename(['components', id], v)} /></Field>
				<Field label="Files (globs)">
					<ListInput mono value={paths(c.path)} onchange={(v) => draft.set(['components', id, 'path'], onePath(v))} placeholder="services/orders/**" />
				</Field>
				<Field label="Kind">
					<NativeSelect value={c.kind ?? ''} onchange={(e) => draft.set(['components', id, 'kind'], e.currentTarget.value || undefined)}>
						<option value="">inferred</option>
						{#each schema?.componentKinds ?? ['service', 'package', 'module'] as k (k)}<option value={k}>{k}</option>{/each}
					</NativeSelect>
				</Field>
				<div class="flex items-end justify-end">
					<Button variant="ghost" size="icon-sm" aria-label="Remove {id}" onclick={() => draft.set(['components', id], undefined)}><Trash /></Button>
				</div>
				<Field label="Owners" class="md:col-span-2">
					<ListInput value={c.owners ?? []} onchange={(v) => draft.set(['components', id, 'owners'], v)} placeholder="@team-orders" />
				</Field>
				<Field label="Labels" class="md:col-span-2">
					<ListInput value={c.labels ?? []} suggestions={labels} onchange={(v) => draft.set(['components', id, 'labels'], v)} placeholder={labels.length ? 'pii, payments…' : 'Declare labels below first'} />
				</Field>
			</div>
		{/each}

		<form
			class="grid gap-3 rounded-lg border border-dashed p-3 md:grid-cols-[minmax(0,10rem)_minmax(0,1fr)_auto]"
			onsubmit={(e) => {
				e.preventDefault();
				add();
			}}
		>
			<Field label="New component"><input class="h-8 rounded-lg border border-input bg-transparent px-2.5 font-mono text-xs dark:bg-input/30" bind:value={newId} placeholder="search" /></Field>
			<Field label="Files"><input class="h-8 rounded-lg border border-input bg-transparent px-2.5 font-mono text-xs dark:bg-input/30" bind:value={newPath} placeholder="services/search/**" /></Field>
			<div class="flex items-end"><Button type="submit" variant="outline" size="sm" disabled={!newId.trim()}><Plus />Add</Button></div>
		</form>

		{#if undeclared.length}
			<div class="grid gap-2">
				<p class="text-xs text-muted-foreground">Found in the code but not declared:</p>
				<div class="flex flex-wrap gap-2">
					{#each undeclared as c (c.id)}
						<Button variant="outline" size="sm" title={c.roots.join(', ')} onclick={() => declare(c)}><Plus />{c.id}</Button>
					{/each}
				</div>
			</div>
		{/if}
	</div>
</Card>
