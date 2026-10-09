// The client of the local API that `onus ui` serves. Every call is a POST to
// /api/<name> with a JSON body, carrying the session token the opened URL
// had in its fragment.

const KEY = 'onus-ui-token';

function readToken(): string {
	if (typeof window === 'undefined') return '';
	const m = /(?:^#|&)token=([0-9a-zA-Z_-]+)/.exec(window.location.hash);
	if (m) {
		try {
			sessionStorage.setItem(KEY, m[1]);
		} catch {
			// Storage can be off; the token then lasts for this page only.
		}
		// Keep the token out of the address bar and the history.
		history.replaceState(history.state, '', window.location.pathname + window.location.search);
		return m[1];
	}
	try {
		return sessionStorage.getItem(KEY) ?? '';
	} catch {
		return '';
	}
}

let token = '';

export function sessionToken(): string {
	if (!token) token = readToken();
	return token;
}

export class ApiError extends Error {
	constructor(
		message: string,
		readonly status: number
	) {
		super(message);
	}
}

export async function api<T = unknown>(name: string, body: unknown = {}): Promise<T> {
	let res: Response;
	try {
		res = await fetch(`/api/${name}`, {
			method: 'POST',
			headers: { 'content-type': 'application/json', 'x-onus-token': sessionToken() },
			body: JSON.stringify(body)
		});
	} catch {
		throw new ApiError('Onus is not answering. Is `onus ui` still running?', 0);
	}
	let data: { ok?: T; error?: string };
	try {
		data = await res.json();
	} catch {
		throw new ApiError(`unexpected answer (${res.status})`, res.status);
	}
	if (!res.ok || data.error !== undefined) {
		throw new ApiError(data.error ?? `request failed (${res.status})`, res.status);
	}
	return data.ok as T;
}

/** A query to the map index, as `onus query` takes it. */
export type Query =
	| { query: 'status' }
	| { query: 'find'; text: string; limit?: number }
	| { query: 'symbol'; id: string }
	| { query: 'dependents' | 'dependencies'; target: string; depth?: number; limit?: number }
	| { query: 'tests-for'; target: string; limit?: number }
	| { query: 'owners'; target: string }
	| { query: 'component'; id: string }
	| { query: 'file'; path: string }
	| { query: 'impact'; target: string; change: ImpactChange; limit?: number }
	| { query: 'invariants'; target?: string };

export type ImpactChange =
	| 'remove'
	| 'rename'
	| 'change-signature'
	| 'add-required-member'
	| 'change-behavior';

export function query<T = unknown>(q: Query): Promise<T> {
	return api<T>('map.query', { query: q });
}
