<script lang="ts">
	import { marked } from 'marked';

	// Only for text Onus ships (the guide, its own reports): links to other
	// topics stay inside the interface.
	let { text }: { text: string } = $props();
	const html = $derived(
		(marked.parse(text, { async: false, gfm: true }) as string).replace(
			/href="(?:\.\/)?([a-z-]+)\.md"/g,
			'href="/guide/$1"'
		)
	);
</script>

<div class="prose">{@html html}</div>
