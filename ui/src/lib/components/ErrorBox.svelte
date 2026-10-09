<script lang="ts">
	let { error, title = 'Something went wrong' }: { error: string; title?: string } = $props();
</script>

<div class="error" role="alert">
	<strong>{title}</strong>
	<pre>{error}</pre>
</div>

<style>
	.error {
		border: 1px solid color-mix(in srgb, var(--del) 40%, transparent);
		background: var(--del-soft);
		border-radius: var(--radius-s);
		padding: var(--space-3) var(--space-4);
		display: grid;
		gap: var(--space-2);
		color: var(--del);
	}
	pre {
		background: transparent;
		border: none;
		padding: 0;
		color: var(--ink);
		white-space: pre-wrap;
		font-size: 12.5px;
	}
</style>
