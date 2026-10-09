<script lang="ts">
	let { label = 'Loading…' }: { label?: string } = $props();
</script>

<div class="loading" role="status"><span class="dot"></span>{label}</div>

<style>
	.loading {
		display: inline-flex;
		align-items: center;
		gap: var(--space-2);
		color: var(--ink-muted);
		font-size: 13px;
		padding: var(--space-2) 0;
	}
	.dot {
		width: 10px;
		height: 10px;
		border-radius: 50%;
		border: 2px solid var(--line);
		border-top-color: var(--ink);
		animation: spin 0.8s linear infinite;
	}
	@keyframes spin {
		to {
			transform: rotate(360deg);
		}
	}
</style>
