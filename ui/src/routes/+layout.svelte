<script lang="ts">
	import '@fontsource-variable/archivo';
	import '@fontsource-variable/geist-mono';
	import '#lib/styles/app.css';
	import { goto } from '$app/navigation';
	import { page } from '$app/state';
	import { app } from '#lib/app.svelte.ts';
	import { short, ago } from '#lib/format.ts';
	import { onMount } from 'svelte';

	let { children } = $props();
	let search = $state('');
	let navOpen = $state(false);

	onMount(() => app.start());

	const nav = [
		{
			group: 'Understand',
			items: [
				{ href: '/', label: 'Overview', icon: 'M3 12l9-8 9 8M5 10v10h14V10' },
				{ href: '/map', label: 'Map', icon: 'M9 4l-6 3v13l6-3 6 3 6-3V4l-6 3-6-3zM9 4v13M15 7v13' },
				{ href: '/changes', label: 'Changes', icon: 'M6 3v12M18 9v12M6 15a3 3 0 100 6 3 3 0 000-6zM18 3a3 3 0 100 6 3 3 0 000-6zM6 9c0 6 12 0 12 6' }
			]
		},
		{
			group: 'Decide',
			items: [
				{ href: '/lanes', label: 'Lanes & judge', icon: 'M4 6h16M4 12h16M4 18h16M8 3v18' },
				{ href: '/environments', label: 'Environments', icon: 'M4 7l8-4 8 4v10l-8 4-8-4V7zM4 7l8 4 8-4M12 11v10' },
				{ href: '/outcomes', label: 'Outcomes', icon: 'M4 19V5M4 19h16M8 15l4-4 3 3 5-6' }
			]
		},
		{
			group: 'Access',
			items: [
				{ href: '/scopes', label: 'Tokens & scopes', icon: 'M15 7a4 4 0 11-8 0 4 4 0 018 0zM11 11v10M11 16h4M11 19h3' },
				{ href: '/escalations', label: 'Escalations', icon: 'M12 3l9 16H3l9-16zM12 10v4M12 17v.5' },
				{ href: '/audit', label: 'Audit log', icon: 'M6 3h9l3 3v15H6V3zM9 9h6M9 13h6M9 17h4' }
			]
		},
		{
			group: 'Setup',
			items: [
				{ href: '/config', label: 'onus.yaml', icon: 'M12 8a4 4 0 100 8 4 4 0 000-8zM12 2v3M12 19v3M2 12h3M19 12h3M5 5l2 2M17 17l2 2M5 19l2-2M17 7l2-2' },
				{ href: '/guide', label: 'Guide', icon: 'M4 5a2 2 0 012-2h13v16H6a2 2 0 00-2 2V5zM4 19a2 2 0 012-2h13' }
			]
		}
	];

	function active(href: string): boolean {
		const p = page.url.pathname;
		return href === '/' ? p === '/' : p === href || p.startsWith(href + '/');
	}

	function find(e: SubmitEvent) {
		e.preventDefault();
		if (search.trim()) goto(`/map?q=${encodeURIComponent(search.trim())}`);
	}

	$effect(() => {
		// Close the menu on narrow screens after navigating.
		void page.url.pathname;
		navOpen = false;
	});
</script>

<div class="shell">
	<aside class:open={navOpen}>
		<a class="brand" href="/">
			<svg viewBox="0 0 32 32" width="22" height="22" aria-hidden="true">
				<circle cx="16" cy="16" r="9" fill="none" stroke="currentColor" stroke-width="3.2" />
				<circle cx="23" cy="9" r="3.6" fill="var(--signal)" />
			</svg>
			<span>Onus</span>
		</a>
		<nav>
			{#each nav as g (g.group)}
				<div class="group">{g.group}</div>
				{#each g.items as item (item.href)}
					<a href={item.href} class:active={active(item.href)} aria-current={active(item.href) ? 'page' : undefined}>
						<svg viewBox="0 0 24 24" width="16" height="16" aria-hidden="true"><path d={item.icon} /></svg>
						{item.label}
					</a>
				{/each}
			{/each}
		</nav>
		<div class="foot">
			<div class="theme" role="group" aria-label="Theme">
				{#each ['system', 'light', 'dark'] as const as t (t)}
					<button class="ghost small" class:on={app.theme === t} onclick={() => app.setTheme(t)}>{t}</button>
				{/each}
			</div>
			{#if app.status}<span class="faint small">onus {app.status.version}</span>{/if}
		</div>
	</aside>

	<div class="main">
		<header>
			<button class="ghost menu" aria-label="Menu" onclick={() => (navOpen = !navOpen)}>☰</button>
			<div class="repo">
				{#if app.status}
					<strong>{app.status.name}</strong>
					<span class="muted">on</span>
					<span class="mono branch">{app.status.branch || short(app.status.head.sha)}</span>
					<span class="faint small hide-narrow" title={app.status.head.subject}>
						{short(app.status.head.sha)} · {ago(app.status.head.at)}
					</span>
					{#if app.status.dirty.length}
						<a class="dirty small" href="/changes" title={app.status.dirty.join('\n')}>{app.status.dirty.length} uncommitted</a>
					{/if}
				{:else if app.error}
					<span class="error small">{app.error}</span>
				{:else}
					<span class="muted small">Building the map…</span>
				{/if}
			</div>
			<form class="search" onsubmit={find}>
				<input type="search" placeholder="Find symbols and files" bind:value={search} aria-label="Find in the map" />
			</form>
			{#if app.status}
				<span class="map small hide-narrow" title="Files, symbols and edges in the map; it follows changes on disk">
					<span class="live" class:on={app.status.mapMeta.watching}></span>
					{app.status.map.files.toLocaleString()} files · {app.status.map.symbols.toLocaleString()} symbols
				</span>
			{/if}
		</header>
		<main>
			{@render children()}
		</main>
	</div>
</div>

<style>
	.shell {
		display: grid;
		grid-template-columns: var(--sidebar) 1fr;
		min-height: 100vh;
	}
	aside {
		background: var(--band);
		color: var(--on-band);
		display: flex;
		flex-direction: column;
		position: sticky;
		top: 0;
		height: 100vh;
		overflow-y: auto;
		padding: var(--space-4) var(--space-3);
		gap: var(--space-4);
	}
	.brand {
		display: flex;
		align-items: center;
		gap: var(--space-2);
		font-weight: 700;
		font-size: 17px;
		text-decoration: none;
		padding: 0 var(--space-2);
		color: var(--on-band);
	}
	nav {
		display: grid;
		gap: 2px;
	}
	.group {
		font-size: 11px;
		text-transform: uppercase;
		letter-spacing: 0.08em;
		color: var(--on-band-muted);
		padding: var(--space-3) var(--space-2) var(--space-1);
		opacity: 0.8;
	}
	nav a {
		display: flex;
		align-items: center;
		gap: var(--space-2);
		padding: 6px var(--space-2);
		border-radius: var(--radius-s);
		color: var(--on-band-muted);
		text-decoration: none;
		font-weight: 500;
	}
	nav a:hover {
		color: var(--on-band);
		background: rgba(255, 255, 255, 0.05);
	}
	nav a.active {
		color: var(--on-band);
		background: rgba(255, 255, 255, 0.09);
		box-shadow: inset 2px 0 0 var(--signal);
	}
	nav svg {
		fill: none;
		stroke: currentColor;
		stroke-width: 1.8;
		stroke-linecap: round;
		stroke-linejoin: round;
		flex: none;
	}
	.foot {
		margin-top: auto;
		display: grid;
		gap: var(--space-2);
		padding: 0 var(--space-2);
	}
	.theme {
		display: flex;
		gap: 2px;
	}
	.theme button {
		color: var(--on-band-muted);
		flex: 1;
		justify-content: center;
	}
	.theme button.on {
		color: var(--on-band);
		background: rgba(255, 255, 255, 0.09);
	}
	.foot .faint {
		color: var(--on-band-muted);
		opacity: 0.7;
	}
	.main {
		min-width: 0;
		display: flex;
		flex-direction: column;
	}
	header {
		position: sticky;
		top: 0;
		z-index: 5;
		display: flex;
		align-items: center;
		gap: var(--space-4);
		padding: var(--space-2) var(--space-5);
		height: 52px;
		background: color-mix(in srgb, var(--canvas) 88%, transparent);
		backdrop-filter: blur(8px);
		border-bottom: 1px solid var(--line);
	}
	.repo {
		display: flex;
		align-items: baseline;
		gap: var(--space-2);
		min-width: 0;
		white-space: nowrap;
		overflow: hidden;
	}
	.branch {
		background: var(--sunken);
		padding: 1px 6px;
		border-radius: var(--radius-xs);
	}
	.dirty {
		color: var(--signal-ink);
		background: var(--signal-soft);
		border-radius: var(--radius-pill);
		padding: 1px 8px;
		text-decoration: none;
		font-weight: 600;
	}
	.error {
		color: var(--del);
	}
	.search {
		margin-left: auto;
		flex: 0 1 320px;
	}
	.search input {
		width: 100%;
	}
	.map {
		display: flex;
		align-items: center;
		gap: 6px;
		color: var(--ink-muted);
		white-space: nowrap;
	}
	.live {
		width: 7px;
		height: 7px;
		border-radius: 50%;
		background: var(--ink-faint);
	}
	.live.on {
		background: var(--add);
	}
	main {
		padding: var(--space-5);
		max-width: 1320px;
		width: 100%;
	}
	.menu {
		display: none;
	}
	@media (max-width: 860px) {
		.shell {
			grid-template-columns: 1fr;
		}
		aside {
			position: fixed;
			z-index: 10;
			width: var(--sidebar);
			transform: translateX(-100%);
			transition: transform 0.2s;
		}
		aside.open {
			transform: none;
			box-shadow: var(--shadow-pop);
		}
		.menu {
			display: inline-flex;
		}
		.hide-narrow {
			display: none;
		}
		header {
			padding: var(--space-2) var(--space-3);
			gap: var(--space-2);
		}
		main {
			padding: var(--space-3);
		}
	}
</style>
