<script lang="ts">
	let { text, from = 0, to = 0 }: { text: string; from?: number; to?: number } = $props();
	const lines = $derived(text.split('\n'));
	let container: HTMLElement | undefined = $state();

	$effect(() => {
		if (!from || !container) return;
		const el = container.querySelector<HTMLElement>(`[data-line="${from}"]`);
		el?.scrollIntoView({ block: 'center' });
	});
</script>

<div class="code" bind:this={container}>
	<table>
		<tbody>
			{#each lines as line, i (i)}
				<tr data-line={i + 1} class:hit={from && i + 1 >= from && i + 1 <= (to || from)}>
					<td class="n">{i + 1}</td>
					<td class="l"><pre>{line || ' '}</pre></td>
				</tr>
			{/each}
		</tbody>
	</table>
</div>

<style>
	.code {
		border: 1px solid var(--line);
		border-radius: var(--radius-s);
		background: var(--sunken);
		overflow: auto;
		max-height: 75vh;
		font-family: var(--font-mono);
		font-size: 12.5px;
	}
	table {
		border-collapse: collapse;
		width: 100%;
	}
	td {
		padding: 0;
		vertical-align: top;
	}
	.n {
		text-align: right;
		padding: 0 var(--space-3);
		color: var(--ink-faint);
		user-select: none;
		width: 1%;
		white-space: nowrap;
		position: sticky;
		left: 0;
		background: var(--sunken);
	}
	pre {
		background: transparent;
		border: none;
		border-radius: 0;
		padding: 0 var(--space-3);
		overflow: visible;
		font-size: inherit;
		line-height: 1.6;
	}
	tr.hit td {
		background: var(--signal-soft);
	}
	tr.hit .n {
		color: var(--signal-ink);
	}
</style>
