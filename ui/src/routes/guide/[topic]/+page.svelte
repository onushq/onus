<script lang="ts">
	import { page } from '$app/state';
	import { api } from '#lib/api.ts';
	import ErrorBox from '#lib/components/ErrorBox.svelte';
	import Loading from '#lib/components/Loading.svelte';
	import Markdown from '#lib/components/Markdown.svelte';
	import { Task } from '#lib/task.svelte.ts';

	const topic = new Task<{ name: string; summary: string; text: string }>();
	const name = $derived(page.params.topic ?? '');

	$effect(() => {
		const n = name;
		topic.run(() => api('guide', { name: n }));
	});
</script>

<svelte:head><title>{name} · Guide · Onus</title></svelte:head>

<p class="small"><a href="/guide">Guide</a> / {name}</p>
<div style="height: 16px"></div>
{#if topic.error}
	<ErrorBox error={topic.error} />
{:else if !topic.value}
	<Loading />
{:else}
	<Markdown text={topic.value.text} />
{/if}
