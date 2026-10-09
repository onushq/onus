import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';
import { parseDocument } from 'yaml';
import { read, renameKey, setIn } from './edit.ts';

const shop = readFileSync(new URL('../../../../fixtures/shop/base/onus.yaml', import.meta.url), 'utf8');

/** The lines that differ, as [before, after] pairs of removed and added lines. */
function changed(a: string, b: string) {
	const x = a.split('\n');
	const y = b.split('\n');
	return { removed: x.filter((l) => !y.includes(l)), added: y.filter((l) => !x.includes(l)) };
}

describe('editing onus.yaml', () => {
	it('changes one value and nothing else', () => {
		const out = setIn(shop, ['components', 'logger', 'owners'], ['@team-observability']);
		expect(changed(shop, out)).toEqual({
			removed: ['  logger:           { path: "packages/logger/**",           owners: ["@team-platform"] }'],
			added: ['  logger:           { path: "packages/logger/**",           owners: ["@team-observability"] }']
		});
	});

	it('adds a component beside the others, in their style', () => {
		const out = setIn(shop, ['components', 'search'], { path: 'services/search/**', owners: ['@team-search'] });
		expect(changed(shop, out)).toEqual({ removed: [], added: ['  search: { path: services/search/**, owners: ["@team-search"] }'] });
		expect(out.indexOf('search:')).toBeGreaterThan(out.indexOf('money:'));
		expect(out.indexOf('search:')).toBeLessThan(out.indexOf('contracts:'));
	});

	it('removes a component by its line', () => {
		const out = setIn(shop, ['components', 'money'], undefined);
		expect(changed(shop, out)).toEqual({ removed: ['  money:            { path: "packages/money/**",            owners: ["@team-platform"] }'], added: [] });
	});

	it('removes a key inside a one-line map by rewriting only that map', () => {
		const out = setIn(shop, ['components', 'billing', 'labels'], []);
		const d = changed(shop, out);
		expect(d.removed).toHaveLength(1);
		expect(d.added).toEqual(['  billing:          { path: "services/billing/**", owners: ["@team-payments"] }']);
	});

	it('adds a whole section at the end in block style', () => {
		const out = setIn(shop, ['lanes'], { default: 'judge', minRecord: 10, rules: [{ lane: 'human', subkinds: ['migration-changed'] }] });
		expect(out.startsWith(shop)).toBe(true);
		expect(out.slice(shop.length)).toBe('lanes:\n  default: judge\n  minRecord: 10\n  rules:\n    - lane: human\n      subkinds: [migration-changed]\n');
	});

	it('turns a section on, then grows it by lines', () => {
		let out = setIn(shop, ['lanes'], { default: 'judge', minRecord: 10 });
		expect(out.slice(shop.length)).toBe('lanes:\n  default: judge\n  minRecord: 10\n');
		out = setIn(out, ['lanes', 'rules', 0], { lane: 'human', subkinds: ['migration-changed'] });
		expect(out.slice(shop.length)).toBe('lanes:\n  default: judge\n  minRecord: 10\n  rules:\n    - lane: human\n      subkinds: [migration-changed]\n');
		out = setIn(out, ['lanes', 'rules', 1], { lane: 'auto-merge', match: 'every', kinds: ['internal'], components: ['docs'] });
		expect(out.slice(shop.length)).toContain('    - lane: auto-merge\n      match: every\n      kinds: [internal]\n      components: [docs]\n');
	});

	it('appends to a list and edits inside its items', () => {
		let out = setIn(shop, ['rules', 1], { deny: { from: 'orders', to: 'billing.internal' } });
		expect(changed(shop, out)).toEqual({ removed: [], added: ['  - deny: { from: orders, to: billing.internal }'] });
		out = setIn(out, ['rules', 0, 'deny', 'to'], 'notifications');
		expect(read(out).data?.rules).toEqual([{ deny: { from: 'billing', to: 'notifications' } }, { deny: { from: 'orders', to: 'billing.internal' } }]);
	});

	it('removes a section when its last entry goes', () => {
		const out = setIn(shop, ['rules', 0], undefined);
		expect(read(out).data?.rules).toBeUndefined();
		expect(out).toContain('# The declared layer for the shop fixture');
		expect(out).toContain('labels:\n  payments: { sensitivity: high }');
	});

	it('renames a key in place', () => {
		const out = renameKey(shop, ['components', 'money'], 'currency');
		expect(changed(shop, out)).toEqual({
			removed: ['  money:            { path: "packages/money/**",            owners: ["@team-platform"] }'],
			added: ['  currency:            { path: "packages/money/**",            owners: ["@team-platform"] }']
		});
		expect(renameKey(shop, ['components', 'money'], 'orders')).toBe(shop);
	});

	it('starts a file from nothing', () => {
		const out = setIn('', ['version'], 1);
		expect(read(out).data).toEqual({ version: 1 });
	});

	it('always produces what the edit means', () => {
		const edits: [(string | number)[], unknown][] = [
			[['labels', 'internal'], { sensitivity: 'low' }],
			[['extractors', 'events', 'publish'], ['bus.emit($EVENT, ...)']],
			[['contracts', 'UserPreferences', 'invariants'], []],
			[['tests', 'globs'], ['**/*.spec.ts']],
			[['environment', 'image'], 'node:22']
		];
		let text = shop;
		for (const [path, value] of edits) {
			const next = setIn(text, path, value);
			const got = path.reduce<unknown>((o, k) => (o as Record<string | number, unknown> | undefined)?.[k], read(next).data);
			if (Array.isArray(value) && value.length === 0) expect(got).toBeUndefined();
			else expect(got).toEqual(value);
			expect(parseDocument(next).errors).toEqual([]);
			text = next;
		}
		expect(text).toContain('# billing may not reach into notifications');
	});
});
