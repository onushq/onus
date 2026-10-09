<script lang="ts">
	import { api } from '#lib/api.ts';
	import ErrorBox from '#lib/components/ErrorBox.svelte';
	import Loading from '#lib/components/Loading.svelte';
	import PageHead from '#lib/components/PageHead.svelte';
	import { Task } from '#lib/task.svelte.ts';
	import ArrowUpRight from '@lucide/svelte/icons/arrow-up-right';
	import { onMount } from 'svelte';

	const topics = new Task<{ topics: { name: string; summary: string }[] }>();
	onMount(() => topics.run(() => api('guide')));
</script>

<PageHead title="Guide">The user guide that ships in the binary, the same as <code>onus help &lt;topic&gt;</code>.</PageHead>

{#if topics.error}
	<ErrorBox error={topics.error} />
{:else if !topics.value}
	<Loading />
{:else}
	<div class="grid gap-3 sm:grid-cols-2 xl:grid-cols-3">
		{#each topics.value.topics as t (t.name)}
			<a href="/guide/{t.name}" class="group grid gap-1 rounded-xl border bg-card p-4 no-underline shadow-xs transition-colors hover:border-ring/60">
				<span class="flex items-center justify-between font-medium">{t.name}<ArrowUpRight class="size-4 text-muted-foreground transition-transform group-hover:translate-x-0.5 group-hover:-translate-y-0.5" /></span>
				<span class="text-sm text-muted-foreground">{t.summary}</span>
			</a>
		{/each}
	</div>
{/if}
