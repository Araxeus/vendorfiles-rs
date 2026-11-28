import { defineConfig } from 'astro/config';
import starlight from '@astrojs/starlight';
import starlightLinksValidator from 'starlight-links-validator';
import starlightThemeNext from 'starlight-theme-next';
import cards from 'astro-cards';

// Written by `cargo xtask site` from its page map, next to the pages themselves.
import sidebar from './src/generated/sidebar.json' with { type: 'json' };

export default defineConfig({
  site: 'https://araxeus.github.io',
  base: '/vendorfiles-rs',

  // Hovering a link fetches its page, and Chromium also renders it ahead of the click.
  prefetch: { prefetchAll: true, defaultStrategy: 'hover' },
  experimental: { clientPrerender: true },

  integrations: [
    cards(),
    starlight({
      title: 'vendorfiles',
      description:
        'Get the files and tools you need from GitHub, releases and any URL, and keep them pinned and current.',
      social: [
        { icon: 'github', label: 'GitHub', href: 'https://github.com/Araxeus/vendorfiles-rs' },
      ],
      sidebar,
      routeMiddleware: './src/route_data.ts',

      // Discord colors a link's embed with this.
      head: [{ tag: 'meta', attrs: { name: 'theme-color', content: '#8aa7f7' } }],

      customCss: ['./src/styles/custom.css'],
      components: { Footer: './src/components/Footer.astro', Hero: './src/components/Hero.astro' },
      plugins: [starlightThemeNext(), starlightLinksValidator()],
    }),
  ],
});
