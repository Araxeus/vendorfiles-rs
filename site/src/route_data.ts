import { defineRouteMiddleware } from '@astrojs/starlight/route-data';
import { renderCard } from 'astro-cards/runtime';

// The home page's title is the site name, which the card already shows.
const HOME_TITLE = 'Get the files and tools you need. Keep them pinned and current.';

// Adds a per-page preview image to the Open Graph and Twitter tags Starlight writes.
export const onRequest = defineRouteMiddleware(async (context) => {
  const { id, entry, head } = context.locals.starlightRoute;
  const title = id === '' ? HOME_TITLE : entry.data.title;
  const card = await renderCard('page', { title });

  // Crawlers ignore relative image URLs.
  const src = new URL(card.src, context.site).href;
  const alt = `vendorfiles: ${title}`;

  const meta = (key: 'property' | 'name', name: string, content: string | number) =>
    head.push({ tag: 'meta', attrs: { [key]: name, content: String(content) } });

  meta('property', 'og:image', src);
  meta('property', 'og:image:width', card.width);
  meta('property', 'og:image:height', card.height);
  meta('property', 'og:image:type', card.type);
  meta('property', 'og:image:alt', alt);
  meta('name', 'twitter:image', src);
  meta('name', 'twitter:image:alt', alt);
});
