<script lang="ts">
	import { Input } from '$lib/components/ui/input/index.js';
	import { app } from '#lib/app.svelte.ts';
	import Field from './Field.svelte';

	let {
		value = $bindable(),
		label,
		placeholder = 'branch, tag or commit',
		id
	}: { value: string; label: string; placeholder?: string; id: string } = $props();

	$effect(() => {
		app.loadRefs();
	});
</script>

<Field {label}>
	<Input list="{id}-refs" bind:value {placeholder} spellcheck={false} autocomplete="off" class="font-mono" />
	<datalist id="{id}-refs">
		{#each app.refs?.refs ?? [] as r (r.name)}<option value={r.name}>{r.subject}</option>{/each}
		{#each app.refs?.commits ?? [] as c (c.sha)}<option value={c.sha}>{c.subject}</option>{/each}
	</datalist>
</Field>
