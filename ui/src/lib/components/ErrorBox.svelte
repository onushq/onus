<script lang="ts">
	import * as Alert from '$lib/components/ui/alert/index.js';
	import CircleAlert from '@lucide/svelte/icons/circle-alert';

	let { error, title = 'Something went wrong' }: { error: string; title?: string } = $props();
</script>

<Alert.Root variant="destructive" class="bg-danger-soft/60">
	<CircleAlert />
	<Alert.Title>{title}</Alert.Title>
	<Alert.Description><pre class="mt-1 border-0 bg-transparent p-0 whitespace-pre-wrap text-foreground">{error}</pre></Alert.Description>
</Alert.Root>
