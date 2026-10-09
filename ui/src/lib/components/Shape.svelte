<script lang="ts">
	// A contract's normalized signature, written the way TypeScript reads.
	interface Param {
		name: string;
		typeText?: string | null;
		optional: boolean;
		rest?: boolean;
	}
	interface Member {
		name: string;
		kind: string;
		typeText?: string | null;
		optional: boolean;
		readonly?: boolean;
		line: number;
	}
	interface Shape {
		kind: string;
		typeParams?: string | null;
		params?: Param[];
		returns?: string | null;
		members?: Member[];
		typeText?: string | null;
		unverified: boolean;
	}

	let { shape, name, file }: { shape: Shape; name: string; file?: string | null } = $props();

	const param = (p: Param) => `${p.rest ? '...' : ''}${p.name}${p.optional ? '?' : ''}${p.typeText ? `: ${p.typeText}` : ''}`;
	const params = $derived((shape.params ?? []).map(param).join(', '));
	const head = $derived(
		shape.params?.length || shape.returns
			? `${name}${shape.typeParams ?? ''}(${params})${shape.returns ? `: ${shape.returns}` : ''}`
			: shape.typeText
				? `${name}${shape.typeParams ?? ''} = ${shape.typeText}`
				: `${name}${shape.typeParams ?? ''}`
	);
</script>

<div class="shape">
	<div class="sig font-mono">{head}{shape.members?.length ? ' {' : ''}</div>
	{#if shape.members?.length}
		<table>
			<tbody>
				{#each shape.members as m (m.name + m.line)}
					<tr>
						<td class="font-mono">
							{m.readonly ? 'readonly ' : ''}<strong>{m.name}</strong>{m.optional ? '?' : ''}{m.kind === 'method' ? '()' : ''}
						</td>
						<td class="type font-mono">{m.typeText ?? ''}</td>
						<td class="text-xs text-muted-foreground">
							{#if file}<a href="/map/file?path={encodeURIComponent(file)}&line={m.line}">:{m.line}</a>{:else}:{m.line}{/if}
						</td>
					</tr>
				{/each}
			</tbody>
		</table>
		<div class="sig font-mono">{'}'}</div>
	{/if}
	{#if shape.unverified}<p class="text-xs text-muted-foreground">Part of this shape is inferred rather than written.</p>{/if}
</div>

<style>
	.shape {
		display: grid;
		gap: 4px;
		font-size: 12.5px;
	}
	.sig {
		overflow-wrap: anywhere;
	}
	table {
		width: auto;
		border-collapse: collapse;
		margin-left: 16px;
	}
	td {
		padding: 2px 12px 2px 0;
		vertical-align: top;
	}
	.type {
		color: var(--info);
		overflow-wrap: anywhere;
	}
</style>
