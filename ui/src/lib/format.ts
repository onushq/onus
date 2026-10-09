// Formatting shared by every page.

export function short(sha: string | null | undefined, n = 8): string {
	return (sha ?? '').slice(0, n);
}

export function plural(n: number, one: string, many = `${one}s`): string {
	return `${n.toLocaleString()} ${n === 1 ? one : many}`;
}

export function ago(seconds: number): string {
	if (!seconds) return '';
	const d = Math.max(0, Date.now() / 1000 - seconds);
	if (d < 60) return 'just now';
	if (d < 3600) return `${Math.floor(d / 60)} min ago`;
	if (d < 86400) return `${Math.floor(d / 3600)} h ago`;
	if (d < 86400 * 30) return `${Math.floor(d / 86400)} d ago`;
	return new Date(seconds * 1000).toLocaleDateString();
}

export function date(seconds: number): string {
	if (!seconds) return '';
	return new Date(seconds * 1000).toLocaleString();
}

export function percent(x: number | null | undefined): string {
	return x === null || x === undefined ? '–' : `${(x * 100).toFixed(1)}%`;
}

export function bytes(n: number): string {
	if (n < 1024) return `${n} B`;
	if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`;
	return `${(n / 1024 / 1024).toFixed(1)} MB`;
}

export function duration(seconds: number): string {
	if (seconds < 60) return `${seconds} s`;
	return `${Math.floor(seconds / 60)} min ${seconds % 60} s`;
}

/** The link to a symbol, file or component page. */
export function symbolHref(id: string): string {
	return `/map/symbol?id=${encodeURIComponent(id)}`;
}
export function fileHref(path: string, line?: number | null, end?: number | null): string {
	let href = `/map/file?path=${encodeURIComponent(path)}`;
	if (line) href += `&line=${line}`;
	if (end && end !== line) href += `&end=${end}`;
	return href;
}
export function componentHref(id: string): string {
	return `/map/component/${encodeURIComponent(id)}`;
}

/** Where an id points: a symbol (`component:path#name`), else a component. */
export function idHref(id: string): string {
	return id.includes('#') ? symbolHref(id) : componentHref(id);
}

export function download(name: string, text: string, type = 'application/json') {
	const url = URL.createObjectURL(new Blob([text], { type }));
	const a = document.createElement('a');
	a.href = url;
	a.download = name;
	a.click();
	setTimeout(() => URL.revokeObjectURL(url), 1000);
}

export async function copy(text: string): Promise<boolean> {
	try {
		await navigator.clipboard.writeText(text);
		return true;
	} catch {
		return false;
	}
}
