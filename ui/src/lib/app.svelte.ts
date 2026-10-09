// What every page shares: the repository's status and its refs, refreshed
// while the interface is open so a rebuilt map shows up.

import { api } from './api';
import type { Refs, Status } from './types';

class App {
	status = $state<Status | undefined>(undefined);
	error = $state<string | undefined>(undefined);
	refs = $state<Refs | undefined>(undefined);
	#timer: ReturnType<typeof setInterval> | undefined;

	async refresh() {
		try {
			this.status = await api<Status>('status');
			this.error = undefined;
		} catch (e) {
			this.error = e instanceof Error ? e.message : String(e);
		}
	}

	async loadRefs(force = false) {
		if (this.refs && !force) return this.refs;
		try {
			this.refs = await api<Refs>('refs');
		} catch {
			// Pages that need refs say so themselves.
		}
		return this.refs;
	}

	start() {
		if (this.#timer) return;
		this.refresh();
		this.#timer = setInterval(() => {
			if (document.visibilityState === 'visible') this.refresh();
		}, 5000);
	}
}

export const app = new App();
