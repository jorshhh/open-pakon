# Website

The project site: plain static HTML, one page per question people search
for (Pakon on a Mac, on Windows 11, without Windows XP, coming from TLX…).
Written for photographers, not developers: plain words, no file paths unless
someone needs them.

- `src/pages/`: one HTML fragment per page. The comment at the top holds the
  page's title, description and questions (as JSON); see `build.py`.
- `src/assets/`: the stylesheet (same retro Kodak minilab look as the app),
  logo, icons, screenshots and the social-preview image `og.png`.
- `build.py`: wraps each page in the shared header and footer, adds the
  search-engine data (canonical link, social preview, app and FAQ
  structured data), and writes `public/` with `sitemap.xml` and `robots.txt`.
  Python 3, standard library only.

The logo and `icon.png` come from `brand/make_logo.py`. After running it,
redo the small icons:

```sh
sips -Z 180 src/assets/icon.png --out src/assets/apple-touch-icon.png
sips -Z 32 src/assets/icon.png --out src/assets/favicon-32.png
```

## Build and preview

```sh
cd website
python3 build.py
python3 -m http.server 8765 --directory public   # http://127.0.0.1:8765
```

The site's address goes into every page (canonical links, sitemap, social
previews). It's `SITE_URL` at the top of `build.py`; override it with
`SITE_URL=https://example.org python3 build.py`.

## Add a page

Copy one of `src/pages/*.html`, change the comment at the top, and add it to
`NAV` or `FOOTER` in `build.py` if it should be linked from every page.
`{{downloads}}` puts in the four download buttons and `{{faq}}` the page's
questions. A page called `name.html` is served at `/name/`.

The download buttons point at the latest GitHub release, so a new release
needs no change here.

## Publish

Upload the contents of `public/` to any static host (GitHub Pages,
Cloudflare Pages, Netlify). Pages are served as `/name/index.html`, and
`404.html` is the not-found page on all three.
