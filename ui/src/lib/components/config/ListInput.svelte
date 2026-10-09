<script lang="ts">
	import X from '@lucide/svelte/icons/x';

	// A list of short strings as chips: Enter or a comma adds what was typed,
	// Backspace in an empty field removes the last one.
	let {
		value = [],
		onchange,
		suggestions = [],
		placeholder = 'Add…',
		mono = false,
		id
	}: {
		value?: string[];
		onchange: (v: string[]) => void;
		suggestions?: { value: string; hint?: string }[];
		placeholder?: string;
		mono?: boolean;
		id?: string;
	} = $props();

	let typed = $state('');
	const listId = `list-${Math.random().toString(36).slice(2)}`;
	const offered = $derived(suggestions.filter((s) => !value.includes(s.value)));

	function add(text: string) {
		const items = text
			.split(',')
			.map((s) => s.trim())
			.filter((s) => s && !value.includes(s));
		if (items.length) onchange([...value, ...items]);
		typed = '';
	}

	function keydown(e: KeyboardEvent) {
		if (e.key === 'Enter' || e.key === ',') {
			if (typed.trim()) {
				e.preventDefault();
				add(typed);
			}
		} else if (e.key === 'Backspace' && !typed && value.length) {
			onchange(value.slice(0, -1));
		}
	}
</script>

<div
	class="flex min-h-8 w-full flex-wrap items-center gap-1 rounded-lg border border-input bg-transparent px-1.5 py-1 text-sm shadow-xs focus-within:border-ring focus-within:ring-3 focus-within:ring-ring/50 dark:bg-input/30"
>
	{#each value as v, i (v)}
		<span class="inline-flex max-w-full items-center gap-0.5 rounded-md bg-muted py-0.5 pr-0.5 pl-2 text-xs {mono ? 'font-mono' : ''}">
			<span class="truncate">{v}</span>
			<button
				type="button"
				class="rounded p-0.5 text-muted-foreground hover:bg-background hover:text-foreground"
				aria-label="Remove {v}"
				onclick={() => onchange(value.filter((_, k) => k !== i))}><X class="size-3" /></button
			>
		</span>
	{/each}
	<input
		{id}
		class="h-6 min-w-24 flex-1 bg-transparent px-1 outline-none placeholder:text-muted-foreground {mono ? 'font-mono text-xs' : ''}"
		placeholder={value.length ? '' : placeholder}
		list={offered.length ? listId : undefined}
		bind:value={typed}
		onkeydown={keydown}
		onblur={() => typed.trim() && add(typed)}
		onchange={(e) => {
			// Picking from the suggestions fires change without Enter.
			if (offered.some((s) => s.value === e.currentTarget.value)) add(e.currentTarget.value);
		}}
	/>
	{#if offered.length}
		<datalist id={listId}>
			{#each offered as s (s.value)}<option value={s.value}>{s.hint ?? ''}</option>{/each}
		</datalist>
	{/if}
</div>
