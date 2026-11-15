import { defineConfig } from 'astro/config';
import starlight from '@astrojs/starlight';
import starlightLinksValidator from 'starlight-links-validator';
import starlightThemeNext from 'starlight-theme-next';

// Written by `cargo xtask site` from its page map, next to the pages themselves.
import sidebar from './src/generated/sidebar.json' with { type: 'json' };

export default defineConfig({
  site: 'https://araxeus.github.io',
  base: '/vendorfiles-rs',

  // Hovering a link fetches its page, and Chromium also renders it ahead of the click.
  prefetch: { prefetchAll: true, defaultStrategy: 'hover' },
  experimental: { clientPrerender: true },

  integrations: [
    starlight({
      title: 'vendorfiles',
      social: [
        { icon: 'github', label: 'GitHub', href: 'https://github.com/Araxeus/vendorfiles-rs' },
      ],
      sidebar,
      customCss: ['./src/styles/custom.css'],
      plugins: [starlightThemeNext(), starlightLinksValidator()],
    }),
  ],
});
