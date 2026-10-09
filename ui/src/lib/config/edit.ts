// Edits to onus.yaml that touch only what changed. The form sets and deletes
// values by path; each edit splices the text of the one node it changes, so
// comments, alignment and quoting elsewhere in the file stay as written.
// Every result is checked against the same edit made on the parsed document,
// and when the two disagree the document is written out whole instead: the
// file is always right, and minimal when it can be.

import { isCollection, isMap, isNode, isPair, isScalar, isSeq, parseDocument, stringify, type Document, type Node } from 'yaml';

export type Path = (string | number)[];

/**
 * A value on one line: a scalar, or a collection in flow style written
 * `{ key: value }` and `[a, b]`. Parsed nodes keep their quoting.
 */
export function inline(value: unknown): string {
	if (isMap(value)) {
		const items = value.items.map((p) => `${inline(p.key)}: ${inline(p.value)}`);
		return items.length ? `{ ${items.join(', ')} }` : '{}';
	}
	if (isSeq(value)) return `[${value.items.map(inline).join(', ')}]`;
	if (isScalar(value)) return stringify(value, { lineWidth: 0 }).trimEnd();
	if (Array.isArray(value)) return `[${value.map(inline).join(', ')}]`;
	if (isPlainObject(value)) {
		const items = Object.entries(value).map(([k, v]) => `${inline(k)}: ${inline(v)}`);
		return items.length ? `{ ${items.join(', ')} }` : '{}';
	}
	return stringify(value ?? null, { lineWidth: 0 }).trimEnd();
}

type Json = null | boolean | number | string | Json[] | { [k: string]: Json };

function isPlainObject(v: unknown): v is Record<string, unknown> {
	return typeof v === 'object' && v !== null && !Array.isArray(v);
}

/** Small enough to read on one line: scalars, lists of scalars, and maps of those. */
function fitsInline(v: unknown): boolean {
	const leaf = (x: unknown) => !isPlainObject(x) && (!Array.isArray(x) || x.every((y) => typeof y !== 'object' || y === null));
	if (leaf(v)) return true;
	return isPlainObject(v) && Object.values(v).every(leaf) && inline(v).length <= 100;
}

/** Lines for `key: value` at `indent`, nested values in block style. */
function pairLines(key: string | number, value: unknown, indent: number): string[] {
	const pad = ' '.repeat(indent);
	const k = typeof key === 'number' ? String(key) : inline(key);
	// Top-level sections are always blocks, so they read like the rest of the file and grow by lines.
	const section = indent === 0 && isPlainObject(value) && Object.keys(value).length > 0;
	if (!section && fitsInline(value)) return [`${pad}${k}: ${inline(value)}`];
	return [`${pad}${k}:`, ...blockLines(value, indent + 2)];
}

/** Lines for a map or list in block style at `indent`. */
function blockLines(value: unknown, indent: number): string[] {
	const pad = ' '.repeat(indent);
	if (Array.isArray(value)) {
		return value.flatMap((item) => {
			if (isPlainObject(item) && !fitsInlineItem(item)) {
				const lines = Object.entries(item).flatMap(([k, v]) => pairLines(k, v, indent + 2));
				return lines.length ? [`${pad}- ${lines[0].trimStart()}`, ...lines.slice(1)] : [`${pad}- {}`];
			}
			return [`${pad}- ${inline(item)}`];
		});
	}
	if (isPlainObject(value)) return Object.entries(value).flatMap(([k, v]) => pairLines(k, v, indent));
	return [`${pad}${inline(value)}`];
}

/** List items that are maps read better as `- key: value` lines unless tiny. */
function fitsInlineItem(item: Record<string, unknown>): boolean {
	return Object.keys(item).length <= 1 && fitsInline(item);
}

function lineStart(text: string, pos: number): number {
	return text.lastIndexOf('\n', pos - 1) + 1;
}

/** Just past the newline that ends the line `pos` is on; a `pos` right after a newline counts as the line before. */
function lineEnd(text: string, pos: number): number {
	while (pos > 0 && (text[pos - 1] === '\n' || text[pos - 1] === '\r')) pos--;
	const n = text.indexOf('\n', pos);
	return n === -1 ? text.length : n + 1;
}

function column(text: string, pos: number): number {
	return pos - lineStart(text, pos);
}

function isEmpty(v: unknown): boolean {
	return v === undefined || v === null || v === '' || (Array.isArray(v) && v.length === 0) || (isPlainObject(v) && Object.keys(v).length === 0);
}

function nodeAt(doc: Document, path: Path): Node | undefined {
	const n = path.length ? doc.getIn(path, true) : doc.contents;
	return isNode(n) ? n : undefined;
}

/** The text of `node` rewritten from the (already edited) document. */
function rendered(node: Node, text: string): string {
	const range = node.range!;
	if (isCollection(node) && !node.flow) {
		const col = column(text, range[0]);
		const lines = blockLines(node.toJSON(), col);
		return lines.map((l, i) => (i === 0 ? l.trimStart() : l)).join('\n');
	}
	return inline(node);
}

function splice(text: string, from: number, to: number, insert: string): string {
	return text.slice(0, from) + insert + text.slice(to);
}

/** The minimal edit, or undefined when this shape is not handled. */
function minimal(text: string, path: Path, value: unknown): string | undefined {
	const doc = parseDocument(text);
	if (doc.errors.length) return undefined;
	const remove = isEmpty(value);
	const target = nodeAt(doc, path);

	// An existing value: replace just its text.
	if (target && !remove && target.range) {
		const range = target.range;
		const replaceBlock = isCollection(target) && !target.flow;
		const insert = replaceBlock && (Array.isArray(value) || isPlainObject(value))
			? blockLines(value, column(text, range[0])).map((l, i) => (i === 0 ? l.trimStart() : l)).join('\n')
			: inline(value);
		return splice(text, range[0], range[1], insert);
	}

	// Find the nearest collection that exists.
	let depth = path.length - 1;
	while (depth >= 0 && !isCollection(nodeAt(doc, path.slice(0, depth)))) depth--;
	if (depth < 0) return undefined;
	const parentPath = path.slice(0, depth);
	const parent = nodeAt(doc, parentPath);
	if (!parent || !isCollection(parent) || !parent.range) return undefined;

	if (remove) {
		if (depth !== path.length - 1 || !target) return text; // nothing there to remove
		// The last entry goes, and the collection it leaves empty with it.
		if (parent.items.length === 1) return path.length > 1 ? minimal(text, path.slice(0, -1), undefined) : undefined;
		if (parent.flow) {
			doc.deleteIn(path);
			return splice(text, parent.range[0], parent.range[1], rendered(parent, text));
		}
		const key = path[path.length - 1];
		const item = isMap(parent) ? parent.items.find((p) => isPair(p) && isScalar(p.key) && p.key.value === key) : isSeq(parent) ? parent.items[key as number] : undefined;
		const start = isPair(item) ? (item.key as Node).range?.[0] : isNode(item) ? item.range?.[0] : undefined;
		const end = isPair(item) ? ((item.value as Node | null)?.range?.[1] ?? (item.key as Node).range?.[1]) : isNode(item) ? item.range?.[1] : undefined;
		if (start === undefined || end === undefined) return undefined;
		return splice(text, lineStart(text, start), lineEnd(text, end), '');
	}

	// A new key (and any missing maps under it) in an existing collection.
	const rest = path.slice(depth);
	let nested: unknown = value;
	for (let i = rest.length - 1; i >= 1; i--) nested = typeof rest[i] === 'number' ? [nested] : { [rest[i]]: nested };
	const key = rest[0];
	if (parent.flow) {
		doc.setIn(path, value);
		return splice(text, parent.range[0], parent.range[1], rendered(parent, text));
	}
	if (isSeq(parent)) {
		if (typeof key !== 'number' || key !== parent.items.length || !parent.items.length) return undefined;
		const first = parent.items[0] as Node;
		const indent = column(text, text.lastIndexOf('-', first.range![0]));
		const last = parent.items[parent.items.length - 1] as Node;
		const at = lineEnd(text, last.range![1]);
		const lines = blockLines([nested], indent);
		return splice(text, at, at, (at === text.length && !text.endsWith('\n') ? '\n' : '') + lines.join('\n') + '\n');
	}
	if (!isMap(parent) || typeof key !== 'string' || !parent.items.length) return undefined;
	const firstKey = (parent.items[0] as { key: Node }).key;
	const indent = column(text, firstKey.range![0]);
	const lastPair = parent.items[parent.items.length - 1] as { key: Node; value: Node | null };
	const end = lastPair.value?.range?.[1] ?? lastPair.key.range![1];
	const at = lineEnd(text, end);
	const lines = pairLines(key, nested, indent);
	return splice(text, at, at, (at === text.length && !text.endsWith('\n') ? '\n' : '') + lines.join('\n') + '\n');
}

/** The same edit made on the parsed document, removing maps it leaves empty. */
function full(text: string, path: Path, value: unknown): Document {
	const doc = parseDocument(text);
	if (isEmpty(value)) {
		doc.deleteIn(path);
		for (let i = path.length - 1; i > 0; i--) {
			const p = path.slice(0, i);
			const n = doc.getIn(p, true);
			if (isCollection(n) && n.items.length === 0) doc.deleteIn(p);
			else break;
		}
	} else if (doc.contents === null || !isCollection(doc.contents)) {
		doc.contents = doc.createNode({}) as unknown as typeof doc.contents;
		doc.setIn(path, value);
	} else {
		doc.setIn(path, value);
	}
	return doc;
}

function same(a: unknown, b: unknown): boolean {
	return JSON.stringify(a) === JSON.stringify(b);
}

/** `text` with the value at `path` set, or removed when `value` is empty. */
export function setIn(text: string, path: Path, value: unknown): string {
	const expected = full(text, path, value);
	const want = expected.toJS() as Json;
	const small = minimal(text, path, value);
	if (small !== undefined) {
		const got = parseDocument(small);
		if (!got.errors.length && same(got.toJS(), want)) return small;
	}
	return expected.toString({ lineWidth: 0 });
}

/** Renames a map key in place, keeping its value and position. */
export function renameKey(text: string, path: Path, to: string): string {
	const doc = parseDocument(text);
	const parent = nodeAt(doc, path.slice(0, -1));
	const from = path[path.length - 1];
	if (!isMap(parent) || from === to) return text;
	const pair = parent.items.find((p) => isScalar(p.key) && p.key.value === from);
	const key = pair?.key as Node | undefined;
	if (!key?.range || parent.has(to)) return text;
	const out = splice(text, key.range[0], key.range[1], inline(to));
	const check = parseDocument(out);
	const want = doc.toJS() as Record<string, unknown>;
	const at = path.slice(0, -1).reduce<Record<string, unknown>>((o, k) => o?.[k as string] as Record<string, unknown>, want);
	if (at) {
		const entries = Object.entries(at).map(([k, v]) => [k === from ? to : k, v]);
		const target = path.slice(0, -1).reduce<Record<string, unknown>>((o, k) => o[k as string] as Record<string, unknown>, want);
		for (const k of Object.keys(target)) delete target[k];
		for (const [k, v] of entries) target[k as string] = v;
	}
	return !check.errors.length && same(check.toJS(), want) ? out : text;
}

/** The parsed value, or the first syntax error. */
export function read(text: string): { data: Record<string, unknown> | null; error: string | null } {
	const doc = parseDocument(text);
	if (doc.errors.length) return { data: null, error: doc.errors[0].message };
	const data = doc.toJS();
	return { data: isPlainObject(data) ? data : {}, error: null };
}
