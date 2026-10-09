<script lang="ts">
	import { Button } from '$lib/components/ui/button/index.js';
	import Card from '#lib/components/Card.svelte';
	import NativeSelect from '#lib/components/NativeSelect.svelte';
	import type { Draft } from '#lib/config/draft.svelte.ts';
	import Plus from '@lucide/svelte/icons/plus';
	import Trash from '@lucide/svelte/icons/trash-2';
	import TextInput from './TextInput.svelte';

	let { draft, sensitivities = ['low', 'medium', 'high'] }: { draft: Draft; sensitivities?: string[] } = $props();

	const labels = $derived(Object.entries(draft.data.labels ?? {}));
	const usedBy = (label: string) =>
		Object.entries(draft.data.components ?? {})
			.filter(([, c]) => c.labels?.includes(label))
			.map(([id]) => id);
	let name = $state('');
</script>

<Card title="Sensitivity labels" subtitle="Labels mark sensitive components. A change that alters how labelled code decides or writes goes to a person.">
	<div class="grid gap-2">
		{#each labels as [label, l] (label)}
			{@const users = usedBy(label)}
			<div class="grid items-center gap-3 rounded-lg border p-2 pl-3 sm:grid-cols-[minmax(0,10rem)_8rem_minmax(0,1fr)_auto]">
				<TextInput value={label} mono onchange={(v) => draft.rename(['labels', label], v)} />
				<NativeSelect value={l?.sensitivity ?? 'high'} onchange={(e) => draft.set(['labels', label, 'sensitivity'], e.currentTarget.value)}>
					{#each sensitivities as s (s)}<option value={s}>{s}</option>{/each}
				</NativeSelect>
				<span class="truncate text-xs text-muted-foreground">{users.length ? `on ${users.join(', ')}` : 'not on any component'}</span>
				<Button variant="ghost" size="icon-sm" aria-label="Remove {label}" onclick={() => draft.set(['labels', label], undefined)}><Trash /></Button>
			</div>
		{/each}
		<form
			class="flex gap-2"
			onsubmit={(e) => {
				e.preventDefault();
				if (name.trim() && !draft.data.labels?.[name.trim()]) draft.set(['labels', name.trim()], { sensitivity: 'high' });
				name = '';
			}}
		>
			<input class="h-8 w-48 rounded-lg border border-input bg-transparent px-2.5 font-mono text-xs dark:bg-input/30" bind:value={name} placeholder="pii" aria-label="New label" />
			<Button type="submit" variant="outline" size="sm" disabled={!name.trim()}><Plus />Add label</Button>
		</form>
	</div>
</Card>
