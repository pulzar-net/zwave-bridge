// The zwave-bridge docs: Starlight with the shared Pulzar theme
// (github.com/pulzar-net/docs-theme). Pages are in src/content/docs; the
// configuration reference there is generated from the code
// (crates/zwave-bridge/tests/config_reference.rs). Built and published by
// .github/workflows/docs.yml, which adds the API docs under /api.
import { defineConfig } from 'astro/config';
import starlight from '@astrojs/starlight';
import { pulzarPlugins } from '@pulzr/docs-theme';
import sidebar from './sidebar.mjs';

export default defineConfig({
	site: 'https://oss.pulzr.net',
	base: '/zwave-bridge',
	// Pages are page.html, the URLs the earlier mdBook site had.
	build: { format: 'file' },
	redirects: { '/library/index': '/zwave-bridge/library.html' },
	integrations: [
		starlight({
			title: 'zwave-bridge',
			description: 'A Z-Wave controller stack and Z-Wave to MQTT bridge in Rust.',
			plugins: pulzarPlugins({
				project: 'zwave-bridge',
				linkExclude: ['/zwave-bridge/api/**', '/zwave-bridge/config.schema.json'],
			}),
			sidebar,
		}),
	],
});
