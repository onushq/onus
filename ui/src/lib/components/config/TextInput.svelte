<script lang="ts">
	import { Input } from '$lib/components/ui/input/index.js';

	// A text field that commits on blur or Enter, so each edit to onus.yaml is
	// one whole value rather than one keystroke.
	let {
		value = '',
		onchange,
		placeholder,
		mono = false,
		type = 'text',
		id,
		list,
		class: className = '',
		...rest
	}: {
		value?: string | number;
		onchange: (v: string) => void;
		placeholder?: string;
		mono?: boolean;
		type?: 'text' | 'number';
		id?: string;
		list?: string;
		class?: string;
		min?: number;
		max?: number;
		step?: number;
	} = $props();

	let current = $state('');
	$effect(() => {
		current = value === undefined || value === null ? '' : String(value);
	});
</script>

<Input
	{id}
	{type}
	{list}
	{placeholder}
	{...rest}
	class="{mono ? 'font-mono text-xs' : ''} {className}"
	bind:value={current}
	onkeydown={(e: KeyboardEvent) => {
		if (e.key === 'Enter') (e.currentTarget as HTMLInputElement).blur();
	}}
	onchange={() => {
		if (current !== String(value ?? '')) onchange(current);
	}}
/>
