<script lang="ts">
	import { Button } from '$lib/components/ui/button/index.js';
	import Card from '#lib/components/Card.svelte';
	import Field from '#lib/components/Field.svelte';
	import LaneBadge from '#lib/components/LaneBadge.svelte';
	import NativeSelect from '#lib/components/NativeSelect.svelte';
	import type { Draft } from '#lib/config/draft.svelte.ts';
	import type { ConfigSchema, Lane } from '#lib/config/types.ts';
	import Plus from '@lucide/svelte/icons/plus';
	import Trash from '@lucide/svelte/icons/trash-2';
	import ListInput from './ListInput.svelte';
	import TextInput from './TextInput.svelte';

	let { draft, schema, componentIds }: { draft: Draft; schema?: ConfigSchema; componentIds: string[] } = $props();

	const lanes = $derived(draft.data.lanes);
	const allLanes: Lane[] = ['auto-merge', 'judge', 'human', 'blocked'];
	const kinds = $derived((schema?.kinds ?? []).map((k) => ({ value: k.name, hint: k.meaning })));
	const subkinds = $derived((schema?.subkinds ?? []).map((k) => ({ value: k.name, hint: `${k.kind}: ${k.when}` })));
	const components = $derived(componentIds.map((c) => ({ value: c })));
	const labels = $derived(Object.keys(draft.data.labels ?? {}).map((l) => ({ value: l })));
	const num = (v: string) => (v.trim() === '' ? undefined : Number(v));
</script>

<Card title="Risk lanes" subtitle="Where a change goes after it is reported: merged on its own, to the judge, to a person, or blocked. Rules move a change up; hard floors keep sensitive work with people whatever the rules say.">
	{#if !lanes}
		<div class="flex flex-wrap items-center justify-between gap-3 rounded-lg border border-dashed p-3 text-sm">
			<span class="text-muted-foreground">Lanes are off: changes are reported, not routed.</span>
			<Button variant="outline" size="sm" onclick={() => draft.set(['lanes'], { default: 'judge', minRecord: 10 })}><Plus />Turn on lanes</Button>
		</div>
	{:else}
		<div class="grid gap-4">
			<div class="grid gap-3 sm:grid-cols-3">
				<Field label="Default lane" hint="For a change no rule speaks about.">
					<NativeSelect value={lanes.default ?? 'judge'} onchange={(e) => draft.set(['lanes', 'default'], e.currentTarget.value)}>
						{#each allLanes as l (l)}<option value={l}>{l}</option>{/each}
					</NativeSelect>
				</Field>
				<Field label="Audit rate" hint="Share of automatic merges a person reviews anyway (0 to 1).">
					<TextInput type="number" min={0} max={1} step={0.01} value={lanes.auditRate ?? ''} placeholder="0.05" onchange={(v) => draft.set(['lanes', 'auditRate'], num(v))} />
				</Field>
				<Field label="Record needed" hint="Merged changes an agent setup needs before it may auto-merge.">
					<TextInput type="number" min={0} step={1} value={lanes.minRecord ?? ''} placeholder="10" onchange={(v) => draft.set(['lanes', 'minRecord'], num(v))} />
				</Field>
			</div>

			<div class="grid gap-2">
				<p class="text-xs font-medium text-muted-foreground">Rules</p>
				{#each lanes.rules ?? [] as r, i (i)}
					<div class="grid gap-3 rounded-lg border p-3 md:grid-cols-2">
						<div class="flex flex-wrap items-end gap-2 md:col-span-2">
							<Field label="Send to">
								<NativeSelect value={r.lane} onchange={(e) => draft.set(['lanes', 'rules', i, 'lane'], e.currentTarget.value)}>
									{#each allLanes as l (l)}<option value={l}>{l}</option>{/each}
								</NativeSelect>
							</Field>
							<Field label="When">
								<NativeSelect value={r.match ?? 'any'} onchange={(e) => draft.set(['lanes', 'rules', i, 'match'], e.currentTarget.value === 'any' ? undefined : e.currentTarget.value)}>
									<option value="any">any row matches</option>
									<option value="every">every row matches</option>
								</NativeSelect>
							</Field>
							<span class="mb-1.5"><LaneBadge lane={r.lane} /></span>
							<Button class="ml-auto" variant="ghost" size="icon-sm" aria-label="Remove rule" onclick={() => draft.set(['lanes', 'rules', i], undefined)}><Trash /></Button>
						</div>
						<Field label="Kinds" hint="Empty matches any.">
							<ListInput mono value={r.kinds ?? []} suggestions={kinds} onchange={(v) => draft.set(['lanes', 'rules', i, 'kinds'], v)} placeholder="internal" />
						</Field>
						<Field label="Subkinds" hint="Empty matches any.">
							<ListInput mono value={r.subkinds ?? []} suggestions={subkinds} onchange={(v) => draft.set(['lanes', 'rules', i, 'subkinds'], v)} placeholder="migration-changed" />
						</Field>
						<Field label="Components">
							<ListInput mono value={r.components ?? []} suggestions={components} onchange={(v) => draft.set(['lanes', 'rules', i, 'components'], v)} placeholder="docs" />
						</Field>
						<Field label="Labels">
							<ListInput mono value={r.labels ?? []} suggestions={labels} onchange={(v) => draft.set(['lanes', 'rules', i, 'labels'], v)} placeholder="payments" />
						</Field>
					</div>
				{/each}
				<div>
					<Button variant="outline" size="sm" onclick={() => draft.set(['lanes', 'rules', (lanes.rules ?? []).length], { lane: 'human', subkinds: ['migration-changed'] })}><Plus />Add rule</Button>
				</div>
			</div>

			<div class="grid gap-3 rounded-lg border p-3 sm:grid-cols-3">
				<p class="text-xs font-medium text-muted-foreground sm:col-span-3">Held-out checks: tests the author never saw, run by the judge in a container</p>
				<Field label="Command"><TextInput mono value={lanes.heldOut?.command ?? ''} placeholder="npm run test:held-out" onchange={(v) => draft.set(['lanes', 'heldOut'], v ? { ...(lanes.heldOut ?? {}), command: v } : undefined)} /></Field>
				<Field label="Image"><TextInput mono value={lanes.heldOut?.image ?? ''} placeholder="node:22" onchange={(v) => lanes.heldOut && draft.set(['lanes', 'heldOut', 'image'], v)} /></Field>
				<Field label="Setup"><TextInput mono value={lanes.heldOut?.setup ?? ''} placeholder="npm ci" onchange={(v) => lanes.heldOut && draft.set(['lanes', 'heldOut', 'setup'], v)} /></Field>
				<Field label="Taste reviewer" hint="A program that reads the submission and prints concerns; it can send a change to a person, never approve it." class="sm:col-span-3">
					<ListInput mono value={lanes.taste?.command ?? []} placeholder="./scripts/review.sh" onchange={(v) => draft.set(['lanes', 'taste'], v.length ? { command: v } : undefined)} />
				</Field>
			</div>

			<div>
				<Button variant="ghost" size="sm" class="text-muted-foreground" onclick={() => draft.set(['lanes'], undefined)}>Turn off lanes</Button>
			</div>
		</div>
	{/if}
</Card>
