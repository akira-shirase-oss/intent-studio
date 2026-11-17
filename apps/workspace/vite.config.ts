import adapter from '@sveltejs/adapter-static';
import tailwindcss from '@tailwindcss/vite';
import { sveltekit } from '@sveltejs/kit/vite';
import { defineConfig } from 'vite';

export default defineConfig({
	plugins: [
		// Tailwind CSS v4 before SvelteKit, as its docs require.
		tailwindcss(),
		sveltekit({
			compilerOptions: {
				runes: ({ filename }) => (filename.split(/[/\\]/).includes('node_modules') ? undefined : true)
			},
			// A static single-page app that intent-server serves at /workspace on its own origin, so
			// the operator's session cookie needs no CORS. It holds no authority and never talks to
			// the database (scripts/check-architecture.sh, rule A5).
			adapter: adapter({ pages: 'build', assets: 'build', fallback: '200.html' }),
			// The page's own policy pins every inline script by hash. frame-ancestors cannot be
			// set from a page, so intent-server sends it, with object-src and base-uri, as a header.
			csp: {
				mode: 'hash',
				directives: {
					'default-src': ['self'],
					'script-src': ['self'],
					'style-src': ['self', 'unsafe-inline'],
					'img-src': ['self', 'data:'],
					'font-src': ['self', 'data:'], // Vite inlines small font subsets as data: URLs
					'connect-src': ['self'],
					'base-uri': ['none'],
					'object-src': ['none'],
					'form-action': ['self']
				}
			},
			paths: { base: '/workspace', relative: false }
		})
	],
	server: {
		host: '127.0.0.1',
		port: 59401,
		strictPort: true,
		// In development the API is an intent-server on its default port.
		proxy: Object.fromEntries(['/v1', '/readyz', '/healthz'].map((path) => [path, 'http://127.0.0.1:59400']))
	},
	preview: { host: '127.0.0.1', port: 59401, strictPort: true }
});
