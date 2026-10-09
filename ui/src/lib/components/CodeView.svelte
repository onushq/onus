<script lang="ts">
	let { text, from = 0, to = 0 }: { text: string; from?: number; to?: number } = $props();
	const lines = $derived(text.split('\n'));
	let container: HTMLElement | undefined = $state();

	$effect(() => {
		if (!from || !container) return;
		container.querySelector<HTMLElement>(`[data-line="${from}"]`)?.scrollIntoView({ block: 'center' });
	});
</script>

<div bind:this={container} class="max-h-[75vh] overflow-auto rounded-xl border bg-card font-mono text-[12.5px] leading-6 shadow-xs">
	<table class="w-full border-collapse">
		<tbody>
			{#each lines as line, i (i)}
				{@const hit = from > 0 && i + 1 >= from && i + 1 <= (to || from)}
				<tr data-line={i + 1} class={hit ? 'bg-signal-soft' : 'hover:bg-muted/50'}>
					<td class="sticky left-0 w-px bg-inherit pr-3 pl-4 text-right whitespace-nowrap select-none {hit ? 'text-signal-foreground' : 'text-muted-foreground/60'}">{i + 1}</td>
					<td class="pr-4 whitespace-pre">{line || ' '}</td>
				</tr>
			{/each}
		</tbody>
	</table>
</div>
