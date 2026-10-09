<script lang="ts">
	// Text from reports, where `backticks` mark code. Rendered as text and
	// <code> elements, never as HTML, since it quotes the repository.
	let { text }: { text: string } = $props();
	const parts = $derived(text.split('`'));
</script>

{#each parts as part, i (i)}{#if i % 2 === 1 && i < parts.length - 1}<code>{part}</code>{:else}{i % 2 === 1 ? '`' + part : part}{/if}{/each}
