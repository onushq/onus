// A piece of work a page waits on: its result, its error, whether it runs.

export class Task<T> {
	value = $state<T | undefined>(undefined);
	error = $state<string | undefined>(undefined);
	running = $state(false);
	#seq = 0;

	constructor(initial?: T) {
		this.value = initial;
	}

	/** Runs `fn`; a later run wins over an earlier one still in flight. */
	async run(fn: () => Promise<T>): Promise<T | undefined> {
		const seq = ++this.#seq;
		this.running = true;
		this.error = undefined;
		try {
			const v = await fn();
			if (seq === this.#seq) this.value = v;
			return v;
		} catch (e) {
			if (seq === this.#seq) this.error = e instanceof Error ? e.message : String(e);
			return undefined;
		} finally {
			if (seq === this.#seq) this.running = false;
		}
	}

	reset() {
		this.#seq++;
		this.value = undefined;
		this.error = undefined;
		this.running = false;
	}
}
