<script lang="ts">
	import LoaderCircle from '@lucide/svelte/icons/loader-circle';

	let { label = 'Loading…' }: { label?: string } = $props();
</script>

<div class="flex items-center gap-2 py-2 text-sm text-muted-foreground" role="status">
	<LoaderCircle class="size-4 animate-spin" />{label}
</div>
