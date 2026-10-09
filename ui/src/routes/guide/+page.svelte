<script lang="ts">
	import { api } from '#lib/api.ts';
	import Card from '#lib/components/Card.svelte';
	import ErrorBox from '#lib/components/ErrorBox.svelte';
	import Loading from '#lib/components/Loading.svelte';
	import PageHead from '#lib/components/PageHead.svelte';
	import { Task } from '#lib/task.svelte.ts';
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
	<div class="grid-3">
		{#each topics.value.topics as t (t.name)}
			<a class="topic" href="/guide/{t.name}">
				<Card title={t.name}><p class="muted">{t.summary}</p></Card>
			</a>
		{/each}
	</div>
{/if}

<style>
	.topic {
		text-decoration: none;
		display: block;
	}
	.topic:hover :global(.card) {
		border-color: var(--control-border);
	}
</style>
