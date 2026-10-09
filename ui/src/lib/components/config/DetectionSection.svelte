<script lang="ts">
	import { Button } from '$lib/components/ui/button/index.js';
	import Card from '#lib/components/Card.svelte';
	import Field from '#lib/components/Field.svelte';
	import type { Draft } from '#lib/config/draft.svelte.ts';
	import Plus from '@lucide/svelte/icons/plus';
	import Trash from '@lucide/svelte/icons/trash-2';
	import ListInput from './ListInput.svelte';
	import TextInput from './TextInput.svelte';

	let { draft }: { draft: Draft } = $props();

	const ex = $derived(draft.data.extractors ?? {});
	const externals = $derived(Object.entries(ex.externals ?? {}));
	let pkg = $state('');
</script>

<Card title="What Onus cannot infer" subtitle="How events are published, which packages are outside services, where tests and test data live. Changing these re-reads the files they affect.">
	<div class="grid gap-4">
		<div class="grid gap-3 sm:grid-cols-2">
			<Field label="Event publish calls" hint="Patterns such as bus.publish($EVENT, ...)">
				<ListInput mono value={ex.events?.publish ?? []} onchange={(v) => draft.set(['extractors', 'events', 'publish'], v)} placeholder="bus.publish($EVENT, ...)" />
			</Field>
			<Field label="Event subscribe calls" hint="Or decorators: @OnEvent($EVENT)">
				<ListInput mono value={ex.events?.subscribe ?? []} onchange={(v) => draft.set(['extractors', 'events', 'subscribe'], v)} placeholder="bus.subscribe($EVENT, ...)" />
			</Field>
		</div>

		<div class="grid gap-2">
			<p class="text-xs font-medium text-muted-foreground">External services, by the npm package that calls them</p>
			{#each externals as [name, s] (name)}
				<div class="grid gap-3 rounded-lg border p-3 md:grid-cols-[minmax(0,10rem)_minmax(0,1fr)_minmax(0,8rem)_auto]">
					<Field label="Package"><TextInput mono value={name} onchange={(v) => draft.rename(['extractors', 'externals', name], v)} /></Field>
					<Field label="Vendor"><TextInput value={s?.vendor ?? ''} onchange={(v) => draft.set(['extractors', 'externals', name, 'vendor'], v)} /></Field>
					<Field label="Category"><TextInput mono value={s?.category ?? ''} placeholder="sms" onchange={(v) => draft.set(['extractors', 'externals', name, 'category'], v)} /></Field>
					<div class="flex items-end justify-end">
						<Button variant="ghost" size="icon-sm" aria-label="Remove {name}" onclick={() => draft.set(['extractors', 'externals', name], undefined)}><Trash /></Button>
					</div>
					<Field label="Data it receives" class="md:col-span-2">
						<ListInput mono value={s?.egress ?? []} onchange={(v) => draft.set(['extractors', 'externals', name, 'egress'], v)} placeholder="phone, email" />
					</Field>
					<Field label="Hosts" class="md:col-span-2">
						<ListInput mono value={s?.hosts ?? []} onchange={(v) => draft.set(['extractors', 'externals', name, 'hosts'], v)} placeholder="api.acme-sms.com" />
					</Field>
				</div>
			{/each}
			<form
				class="flex gap-2"
				onsubmit={(e) => {
					e.preventDefault();
					const p = pkg.trim();
					if (p && !ex.externals?.[p]) draft.set(['extractors', 'externals', p], { vendor: p, category: 'api' });
					pkg = '';
				}}
			>
				<input class="h-8 w-64 rounded-lg border border-input bg-transparent px-2.5 font-mono text-xs dark:bg-input/30" bind:value={pkg} placeholder="@acme/sms" aria-label="npm package" />
				<Button type="submit" variant="outline" size="sm" disabled={!pkg.trim()}><Plus />Add service</Button>
			</form>
		</div>

		<div class="grid gap-3 sm:grid-cols-2">
			<Field label="Test files" hint="Defaults cover *.test.*, *.spec.* and __tests__/.">
				<ListInput mono value={draft.data.tests?.globs ?? []} onchange={(v) => draft.set(['tests', 'globs'], v)} placeholder="e2e/**/*.ts" />
			</Field>
			<Field label="Test data" hint="Fixtures and samples: left out of the map.">
				<ListInput mono value={draft.data.testData ?? []} onchange={(v) => draft.set(['testData'], v)} placeholder="fixtures/**" />
			</Field>
			<Field label="Prisma clients" hint="Identifiers that hold a client. Default: prisma.">
				<ListInput mono value={ex.prisma?.clients ?? []} onchange={(v) => draft.set(['extractors', 'prisma', 'clients'], v)} placeholder="db" />
			</Field>
			<Field label="Framework packs" hint="Query packs, relative to onus.yaml.">
				<ListInput mono value={ex.packs ?? []} onchange={(v) => draft.set(['extractors', 'packs'], v)} placeholder="onus/nestjs.yaml" />
			</Field>
		</div>
	</div>
</Card>
