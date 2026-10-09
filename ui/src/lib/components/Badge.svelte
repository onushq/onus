<script lang="ts">
	import type { Snippet } from 'svelte';

	let {
		tone = 'neutral',
		children,
		title
	}: {
		tone?: 'neutral' | 'signal' | 'add' | 'del' | 'info' | 'ink' | 'faint';
		children: Snippet;
		title?: string;
	} = $props();
</script>

<span class="badge {tone}" {title}>{@render children()}</span>

<style>
	.badge {
		display: inline-flex;
		align-items: center;
		gap: 4px;
		height: 20px;
		padding: 0 7px;
		border-radius: var(--radius-pill);
		font-size: 11.5px;
		font-weight: 600;
		white-space: nowrap;
		background: var(--sunken);
		color: var(--ink-muted);
		border: 1px solid transparent;
	}
	.signal {
		background: var(--signal-soft);
		color: var(--signal-ink);
		border-color: color-mix(in srgb, var(--signal) 45%, transparent);
	}
	.add {
		background: var(--add-soft);
		color: var(--add);
	}
	.del {
		background: var(--del-soft);
		color: var(--del);
	}
	.info {
		background: var(--info-soft);
		color: var(--info);
	}
	.ink {
		background: var(--ink);
		color: var(--canvas);
	}
	.faint {
		background: transparent;
		border-color: var(--line);
		color: var(--ink-faint);
	}
</style>
