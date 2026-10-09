// The onus.yaml being edited. The text is the one source of truth: the form
// and the YAML editor both change it, the form through minimal edits.

import { read, renameKey, setIn, type Path } from './edit.ts';
import type { OnusConfig } from './types.ts';

export class Draft {
	/** The text in the editor. */
	text = $state('');
	/** The text on disk when it was last loaded or saved. */
	base = $state('');
	/** Its sha256, so a save never overwrites edits made on disk meanwhile; null when there was no file. */
	baseHash = $state<string | null>(null);
	exists = $state(false);

	#parsed = $derived(read(this.text));
	/** The first YAML syntax error, before the server checks the meaning. */
	syntax = $derived(this.#parsed.error);
	data = $derived((this.#parsed.data ?? {}) as OnusConfig);
	dirty = $derived(this.text !== this.base);

	load(text: string | null, hash: string | null) {
		this.text = text ?? '';
		this.base = text ?? '';
		this.baseHash = hash;
		this.exists = text !== null;
	}

	/** Sets the value at `path`, or removes it when empty. Ignored while the YAML does not parse. */
	set(path: Path, value: unknown) {
		if (this.syntax) return;
		const start = this.text.trim() ? this.text : 'version: 1\n';
		this.text = setIn(start, path, value);
	}

	rename(path: Path, to: string) {
		if (this.syntax || !to.trim()) return;
		this.text = renameKey(this.text, path, to.trim());
	}
}
