// A line diff for reviewing an edit before it is saved.

export type DiffLine = { op: ' ' | '-' | '+'; text: string; a?: number; b?: number };

/** Lines of `a` and `b` aligned by their longest common subsequence. */
export function diffLines(a: string, b: string): DiffLine[] {
	const x = a.split('\n');
	const y = b.split('\n');
	if (x[x.length - 1] === '') x.pop();
	if (y[y.length - 1] === '') y.pop();
	const n = x.length;
	const m = y.length;
	const lcs: Uint32Array[] = Array.from({ length: n + 1 }, () => new Uint32Array(m + 1));
	for (let i = n - 1; i >= 0; i--)
		for (let j = m - 1; j >= 0; j--) lcs[i][j] = x[i] === y[j] ? lcs[i + 1][j + 1] + 1 : Math.max(lcs[i + 1][j], lcs[i][j + 1]);
	const out: DiffLine[] = [];
	let i = 0;
	let j = 0;
	while (i < n || j < m) {
		if (i < n && j < m && x[i] === y[j]) out.push({ op: ' ', text: x[i++], a: i, b: ++j });
		else if (j < m && (i >= n || lcs[i][j + 1] >= lcs[i + 1][j])) out.push({ op: '+', text: y[j++], b: j });
		else out.push({ op: '-', text: x[i++], a: i });
	}
	return out;
}

/** The changed lines with `context` lines around them; null marks a gap. */
export function hunks(lines: DiffLine[], context = 2): (DiffLine | null)[] {
	const keep = new Set<number>();
	lines.forEach((l, k) => {
		if (l.op !== ' ') for (let d = -context; d <= context; d++) keep.add(k + d);
	});
	const out: (DiffLine | null)[] = [];
	let last = -1;
	lines.forEach((l, k) => {
		if (!keep.has(k)) return;
		if (last !== -1 && k !== last + 1) out.push(null);
		out.push(l);
		last = k;
	});
	return out;
}
