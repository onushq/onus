<script lang="ts">
	import { page } from '$app/state';
	import * as DropdownMenu from '$lib/components/ui/dropdown-menu/index.js';
	import * as Sidebar from '$lib/components/ui/sidebar/index.js';
	import { app } from '#lib/app.svelte.ts';
	import { active, nav } from '#lib/nav/nav.ts';
	import ChevronsUpDown from '@lucide/svelte/icons/chevrons-up-down';
	import GitBranch from '@lucide/svelte/icons/git-branch';
	import Monitor from '@lucide/svelte/icons/monitor';
	import Moon from '@lucide/svelte/icons/moon';
	import Sun from '@lucide/svelte/icons/sun';
	import { mode, setMode, userPrefersMode } from 'mode-watcher';

	const s = $derived(app.status);
</script>

<Sidebar.Root variant="inset" collapsible="icon">
	<Sidebar.Header>
		<Sidebar.Menu>
			<Sidebar.MenuItem>
				<Sidebar.MenuButton size="lg" class="data-[slot=sidebar-menu-button]:p-1.5!">
					{#snippet child({ props })}
						<a href="/" {...props}>
							<div class="flex aspect-square size-8 items-center justify-center rounded-lg bg-primary text-primary-foreground">
								<svg viewBox="0 0 32 32" class="size-5" aria-hidden="true">
									<circle cx="15" cy="17" r="8" fill="none" stroke="currentColor" stroke-width="3.4" />
									<circle cx="22.5" cy="9.5" r="3.6" fill="var(--signal)" />
								</svg>
							</div>
							<div class="grid flex-1 text-left leading-tight">
								<span class="truncate font-semibold">{s?.name ?? 'Onus'}</span>
								<span class="flex items-center gap-1 truncate font-mono text-xs text-muted-foreground">
									<GitBranch class="size-3!" />{s?.branch || s?.head.sha.slice(0, 8) || '…'}
								</span>
							</div>
						</a>
					{/snippet}
				</Sidebar.MenuButton>
			</Sidebar.MenuItem>
		</Sidebar.Menu>
	</Sidebar.Header>
	<Sidebar.Content>
		{#each nav as g (g.group)}
			<Sidebar.Group>
				<Sidebar.GroupLabel>{g.group}</Sidebar.GroupLabel>
				<Sidebar.GroupContent>
					<Sidebar.Menu>
						{#each g.items as item (item.href)}
							<Sidebar.MenuItem>
								<Sidebar.MenuButton isActive={active(page.url.pathname, item.href)} tooltipContent={item.label}>
									{#snippet child({ props })}
										<a href={item.href} {...props}><item.icon /><span>{item.label}</span></a>
									{/snippet}
								</Sidebar.MenuButton>
								{#if item.href === '/changes' && s?.dirty.length}
									<Sidebar.MenuBadge class="bg-signal-soft text-signal-foreground">{s.dirty.length}</Sidebar.MenuBadge>
								{/if}
							</Sidebar.MenuItem>
						{/each}
					</Sidebar.Menu>
				</Sidebar.GroupContent>
			</Sidebar.Group>
		{/each}
	</Sidebar.Content>
	<Sidebar.Footer>
		<Sidebar.Menu>
			<Sidebar.MenuItem>
				<DropdownMenu.Root>
					<DropdownMenu.Trigger>
						{#snippet child({ props })}
							<Sidebar.MenuButton {...props} class="data-[state=open]:bg-sidebar-accent">
								{#if mode.current === 'dark'}<Moon />{:else}<Sun />{/if}
								<span class="flex-1">Theme: {userPrefersMode.current}</span>
								<ChevronsUpDown class="ml-auto" />
							</Sidebar.MenuButton>
						{/snippet}
					</DropdownMenu.Trigger>
					<DropdownMenu.Content side="top" align="start" class="w-48">
						<DropdownMenu.Item onclick={() => setMode('light')}><Sun />Light</DropdownMenu.Item>
						<DropdownMenu.Item onclick={() => setMode('dark')}><Moon />Dark</DropdownMenu.Item>
						<DropdownMenu.Item onclick={() => setMode('system')}><Monitor />System</DropdownMenu.Item>
					</DropdownMenu.Content>
				</DropdownMenu.Root>
			</Sidebar.MenuItem>
		</Sidebar.Menu>
		{#if s}<p class="px-2 pb-1 font-mono text-[11px] text-muted-foreground group-data-[collapsible=icon]:hidden">onus {s.version}</p>{/if}
	</Sidebar.Footer>
	<Sidebar.Rail />
</Sidebar.Root>
