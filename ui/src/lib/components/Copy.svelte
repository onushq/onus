<script lang="ts">
	import { copy } from '#lib/format.ts';

	let { text, label = 'Copy' }: { text: string; label?: string } = $props();
	let done = $state(false);
</script>

<button
	class="small"
	onclick={async () => {
		done = await copy(text);
		setTimeout(() => (done = false), 1500);
	}}>{done ? 'Copied' : label}</button
>
