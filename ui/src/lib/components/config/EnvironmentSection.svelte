<script lang="ts">
	import Card from '#lib/components/Card.svelte';
	import Field from '#lib/components/Field.svelte';
	import type { Draft } from '#lib/config/draft.svelte.ts';
	import ListInput from './ListInput.svelte';
	import TextInput from './TextInput.svelte';

	let { draft }: { draft: Draft } = $props();
	const env = $derived(draft.data.environment ?? {});
	const set = (key: string, v: unknown) => draft.set(['environment', key], v);
</script>

<Card title="Environment" subtitle="The container that runs a change and records evidence. Read from your working tree, never from the change under test.">
	<div class="grid gap-3 sm:grid-cols-2">
		<Field label="Image" hint="Default: the devcontainer's image."><TextInput mono value={env.image ?? ''} placeholder="node:22" onchange={(v) => set('image', v)} /></Field>
		<Field label="Setup" hint="Runs once per lockfile, with network."><TextInput mono value={env.setup ?? ''} placeholder="npm ci" onchange={(v) => set('setup', v)} /></Field>
		<Field label="Seed" hint="Loads synthetic data."><TextInput mono value={env.seed ?? ''} placeholder="npm run db:seed" onchange={(v) => set('seed', v)} /></Field>
		<Field label="Egress proxy image"><TextInput mono value={env.egressImage ?? ''} placeholder="ubuntu/squid" onchange={(v) => set('egressImage', v)} /></Field>
		<Field label="Evidence" hint="JUnit XML a run writes (globs)."><ListInput mono value={env.evidence ?? []} onchange={(v) => set('evidence', v)} placeholder="reports/junit.xml" /></Field>
		<Field label="Traces" hint="OTLP JSON traces a run writes (globs)."><ListInput mono value={env.traces ?? []} onchange={(v) => set('traces', v)} placeholder="traces/*.json" /></Field>
		<Field label="Lockfiles" hint="When they change, setup runs again."><ListInput mono value={env.lockfiles ?? []} onchange={(v) => set('lockfiles', v)} placeholder="package-lock.json" /></Field>
	</div>
</Card>
