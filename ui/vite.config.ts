import adapter from '@sveltejs/adapter-static';
import tailwindcss from '@tailwindcss/vite';
import { sveltekit } from '@sveltejs/kit/vite';
import { fileURLToPath } from 'node:url';
import { defineConfig } from 'vite';

// `npm run dev` talks to a running `onus ui`: start it with a fixed token,
//   ONUS_UI_TOKEN=dev onus ui --no-open
// and open http://localhost:5173/#token=dev
const target = process.env.ONUS_UI_URL ?? 'http://127.0.0.1:4387';

export default defineConfig({
	plugins: [
		tailwindcss(),
		sveltekit({
			// One page app: the binary serves index.html for every route.
			adapter: adapter({ fallback: 'index.html', precompress: false }),
			output: { bundleStrategy: 'split' },
			version: { name: 'onus-ui' }
		})
	],
	resolve: {
		// shadcn-svelte components import from $lib.
		alias: { $lib: fileURLToPath(new URL('./src/lib', import.meta.url)) }
	},
	server: {
		proxy: {
			'/api': {
				target,
				changeOrigin: true,
				headers: { host: new URL(target).host }
			}
		}
	}
});
