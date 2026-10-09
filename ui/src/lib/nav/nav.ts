// The pages of the interface, grouped as in the sidebar.
import BookOpen from '@lucide/svelte/icons/book-open';
import Boxes from '@lucide/svelte/icons/boxes';
import ChartLine from '@lucide/svelte/icons/chart-line';
import FileCog from '@lucide/svelte/icons/file-cog';
import GitCompareArrows from '@lucide/svelte/icons/git-compare-arrows';
import Gavel from '@lucide/svelte/icons/gavel';
import KeyRound from '@lucide/svelte/icons/key-round';
import LayoutDashboard from '@lucide/svelte/icons/layout-dashboard';
import Network from '@lucide/svelte/icons/network';
import ScrollText from '@lucide/svelte/icons/scroll-text';
import TriangleAlert from '@lucide/svelte/icons/triangle-alert';
import type { Component } from 'svelte';

export interface NavItem {
	href: string;
	label: string;
	icon: Component;
	hint: string;
}

export const nav: { group: string; items: NavItem[] }[] = [
	{
		group: 'Understand',
		items: [
			{ href: '/', label: 'Overview', icon: LayoutDashboard, hint: 'The repository at a glance' },
			{ href: '/map', label: 'Map', icon: Network, hint: 'Components, files, events and search' },
			{ href: '/changes', label: 'Changes', icon: GitCompareArrows, hint: 'What uncommitted work or two refs mean' }
		]
	},
	{
		group: 'Decide',
		items: [
			{ href: '/lanes', label: 'Lanes & judge', icon: Gavel, hint: 'Submit, classify and judge a change' },
			{ href: '/environments', label: 'Environments', icon: Boxes, hint: 'Run commands and keep evidence' },
			{ href: '/outcomes', label: 'Outcomes', icon: ChartLine, hint: 'Track records and the production loop' }
		]
	},
	{
		group: 'Access',
		items: [
			{ href: '/scopes', label: 'Tokens & scopes', icon: KeyRound, hint: 'Root keys, plans and task tokens' },
			{ href: '/escalations', label: 'Escalations', icon: TriangleAlert, hint: 'Requests for more access' },
			{ href: '/audit', label: 'Audit log', icon: ScrollText, hint: 'Every decision, hash-chained' }
		]
	},
	{
		group: 'Setup',
		items: [
			{ href: '/config', label: 'onus.yaml', icon: FileCog, hint: 'What is declared' },
			{ href: '/guide', label: 'Guide', icon: BookOpen, hint: 'The user guide' }
		]
	}
];

export const pages = nav.flatMap((g) => g.items);

export function active(pathname: string, href: string): boolean {
	return href === '/' ? pathname === '/' : pathname === href || pathname.startsWith(href + '/');
}

/** The page a path belongs to, for breadcrumbs. */
export function pageOf(pathname: string): NavItem | undefined {
	return [...pages].sort((a, b) => b.href.length - a.href.length).find((p) => active(pathname, p.href));
}
