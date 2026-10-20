# Bricks documentation site

The documentation site of Bricks is a Next.js application built with
[Fumadocs](https://fumadocs.dev). Its pages are MDX files in
`content/docs/`, and the sidebar order is set by `content/docs/meta.json`.

The site uses **npm** and its lock file `package-lock.json`; no other package
manager or lock file is kept. Run these commands in this folder:

```bash
npm ci                 # install exactly the locked versions (runs fumadocs-mdx)
npm run types:check    # MDX, Next type generation, TypeScript
npm run build          # production build
npm run dev            # development server, http://localhost:3000
```

The Markdown guides in the parent folder (`docs/*.md`) are the reference
documents of the repository. The site presents the same subjects as pages.

## Layout

| Path | Contents |
|---|---|
| `content/docs/` | the pages (MDX) and `meta.json`, the navigation |
| `source.config.ts` | the Fumadocs collection: page and `meta.json` schemas |
| `lib/source.ts` | the content source adapter; its `loader()` gives access to the pages |
| `lib/layout.shared.tsx` | the options shared by the layouts |
| `app/(home)` | the landing page and its route group |
| `app/docs` | the documentation layout and the page route |
| `app/api/search/route.ts` | the route handler of the search |

## Navigation

`content/docs/meta.json` lists the pages in order, grouped by section headings
(`---Section---`). A page that is not listed is still served at its URL; it
does not appear in the sidebar.

## Learn more

- [Next.js documentation](https://nextjs.org/docs)
- [Fumadocs](https://fumadocs.dev), with its [MDX introduction](https://fumadocs.dev/docs/mdx)
