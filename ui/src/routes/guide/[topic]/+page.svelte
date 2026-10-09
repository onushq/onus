<script lang="ts">
	import { page } from '$app/state';
	import { api } from '#lib/api.ts';
	import ErrorBox from '#lib/components/ErrorBox.svelte';
	import Loading from '#lib/components/Loading.svelte';
	import Markdown from '#lib/components/Markdown.svelte';
	import { Task } from '#lib/task.svelte.ts';

	const topic = new Task<{ name: string; summary: string; text: string }>();
	const topics = new Task<{ topics: { name: string; summary: string }[] }>();
	const name = $derived(page.params.topic ?? '');
	topics.run(() => api('guide'));

	$effect(() => {
		const n = name;
		topic.run(() => api('guide', { name: n }));
	});
</script>

<svelte:head><title>{name} · Guide · Onus</title></svelte:head>

<div class="grid gap-8 lg:grid-cols-[200px_minmax(0,1fr)]">
	<nav class="hidden lg:block">
		<ul class="sticky top-16 grid gap-0.5 text-sm">
			{#each topics.value?.topics ?? [] as t (t.name)}
				<li><a href="/guide/{t.name}" class="block rounded-md px-2 py-1 no-underline hover:bg-muted {t.name === name ? 'bg-muted font-medium' : 'text-muted-foreground'}">{t.name}</a></li>
			{/each}
		</ul>
	</nav>
	<div class="min-w-0">
		{#if topic.error}
			<ErrorBox error={topic.error} />
		{:else if !topic.value}
			<Loading />
		{:else}
			<Markdown text={topic.value.text} />
		{/if}
	</div>
</div>
