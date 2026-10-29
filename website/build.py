#!/usr/bin/env python3
"""Build the OpenPakon website: src/ → public/ (plain static HTML).

Each page in src/pages/ is an HTML fragment that starts with a comment
holding its settings as JSON:

    <!--
    {"title": "...", "description": "...", "faq": [["Question?", "Answer."]]}
    -->

The fragment can use {{downloads}} (the four download buttons) and {{faq}}
(the page's questions, which also become FAQ structured data). index.html is
served at /, 404.html at /404.html and every other page at /<name>/.

    python3 build.py                       # SITE_URL below
    SITE_URL=https://example.org python3 build.py

Standard library only.
"""

import html
import json
import os
import re
import shutil
from pathlib import Path

SITE_URL = os.environ.get("SITE_URL", "https://openpakon.com").rstrip("/")
SITE_NAME = "OpenPakon"
REPO = "https://github.com/jorshhh/open-pakon"
COMMUNITY = "https://www.facebook.com/groups/PakonF135"
DOWNLOAD = REPO + "/releases/latest/download/"

ROOT = Path(__file__).resolve().parent
SRC = ROOT / "src"
OUT = ROOT / "public"

NAV = [("/setup/", "How to scan"), ("/mac/", "Mac"),
       ("/windows/", "Windows"), ("/linux/", "Linux")]

FOOTER = [
    ("Get it", [("/mac/", "Mac"), ("/windows/", "Windows 10 & 11"),
                ("/linux/", "Linux"), ("/setup/", "How to scan")]),
    ("Questions", [("/pakon-without-windows-xp/", "Without Windows XP"),
                   ("/tlxclientdemo-alternative/", "Coming from TLX"),
                   ("/pakon-scanners/", "Which scanners work"),
                   ("/16-bit-tiff/", "16-bit TIFFs"),
                   ("/troubleshooting/", "Troubleshooting")]),
    ("About", [("/openpakon-vs-pakonscan/", "OpenPakon vs PakonScan"),
               ("/privacy/", "Privacy"), (REPO, "GitHub"), (COMMUNITY, "Pakon F-135 Facebook group"),
               (REPO + "/issues", "Report a problem")]),
]

# name: (file, button label, the line under it, label in the "other downloads" list)
TARGETS = {
    "mac": ("Pakon.dmg", "Download for Mac", "Apple silicon (M1 and later) · macOS 13 or later", "Mac (Apple silicon)"),
    "mac-intel": ("Pakon-Universal.dmg", "Download for Intel Mac", "Intel Macs · macOS 13 or later", "Mac (Intel)"),
    "windows": ("Pakon-Setup.exe", "Download for Windows", "Windows 10 or 11, 64-bit", "Windows 10 &amp; 11"),
    "linux": ("pakon_amd64.deb", "Download for Linux", "Ubuntu 24.04, Linux Mint 22", "Linux (.deb)"),
}

ARROW = ('<svg viewBox="0 0 24 24" width="18" height="18" fill="none" stroke="currentColor" '
         'stroke-width="2.5" aria-hidden="true"><path d="M12 4v11M7 10l5 5 5-5M5 20h14"/></svg>')


def download_button(name, ghost=False):
    file, label, note, _ = TARGETS[name]
    cls = "btn btn--dl btn--ghost" if ghost else "btn btn--dl"
    return (f'<div class="get">\n    <a class="{cls}" href="{DOWNLOAD}{file}">{ARROW}{label}</a>\n'
            f'    <span class="get__note">{note}</span>\n</div>')


GITHUB_ICON = ('<svg viewBox="0 0 16 16" width="18" height="18" fill="currentColor" aria-hidden="true">'
               '<path d="M8 0a8 8 0 0 0-2.53 15.59c.4.07.55-.17.55-.38v-1.33c-2.23.48-2.7-1.07-2.7-1.07'
               '-.36-.93-.89-1.17-.89-1.17-.73-.5.05-.49.05-.49.8.06 1.23.83 1.23.83.72 1.22 1.87.87 2.33.66'
               '.07-.52.28-.87.5-1.07-1.78-.2-3.65-.89-3.65-3.95 0-.87.31-1.59.82-2.15-.08-.2-.36-1.02.08-2.12'
               ' 0 0 .67-.21 2.2.82a7.6 7.6 0 0 1 4 0c1.53-1.04 2.2-.82 2.2-.82.44 1.1.16 1.92.08 2.12.51.56.82'
               ' 1.28.82 2.15 0 3.07-1.87 3.75-3.66 3.95.29.25.54.73.54 1.48v2.2c0 .21.15.46.55.38A8 8 0 0 0 8 0z"/></svg>')

# The home page's button: Mac by default, switched to the visitor's system by
# the script (Intel Macs can't be told apart, hence the Intel link below it).
# The "Also for" links are separated in CSS, so the script can drop one.
DOWNLOADS = f"""<div class="get-row">
    <div class="get">
        <a class="btn btn--dl" id="get-main" href="{DOWNLOAD}{TARGETS['mac'][0]}">{ARROW}<span id="get-label">{TARGETS['mac'][1]}</span></a>
        <span class="get__note" id="get-note">{TARGETS['mac'][2]}</span>
    </div>
    <a class="btn btn--dl btn--ghost" href="{REPO}">{GITHUB_ICON}View on GitHub</a>
</div>
<p class="get__more"><strong>Free forever.</strong> Also for <span class="get__list">""" + "".join(
    f'<a href="{DOWNLOAD}{f}" data-os="{n}">{other}</a>'
    for n, (f, _, _, other) in TARGETS.items()) + f"""</span> · <a href="{REPO}/releases">all versions</a></p>
<script>
(function () {{
    var t = {json.dumps({n: [DOWNLOAD + f, label, note] for n, (f, label, note, _) in TARGETS.items()})};
    var p = ((navigator.userAgentData && navigator.userAgentData.platform) || navigator.platform || "").toLowerCase();
    var os = /win/.test(p) ? "windows" : /linux/.test(p) && !/android/i.test(navigator.userAgent) ? "linux" : "mac";
    document.getElementById("get-main").href = t[os][0];
    document.getElementById("get-label").textContent = t[os][1];
    document.getElementById("get-note").textContent = t[os][2];
    document.querySelector('.get__list [data-os="' + os + '"]').remove();
}})();
</script>"""

APP_LD = {
    "@context": "https://schema.org",
    "@type": "SoftwareApplication",
    "name": "OpenPakon",
    "alternateName": ["open-pakon", "Pakon", "Pakon F-135 scanner software"],
    "description": "Free software for the Kodak Pakon F-135 and F-135+ film "
                   "scanners on Mac, Windows 10/11 and Linux. No Windows XP needed.",
    "applicationCategory": "MultimediaApplication",
    "operatingSystem": "macOS 13+, Windows 10, Windows 11, Ubuntu 24.04, Linux Mint 22",
    "offers": {"@type": "Offer", "price": "0", "priceCurrency": "USD"},
    "license": "https://www.gnu.org/licenses/agpl-3.0.html",
    "downloadUrl": REPO + "/releases/latest",
    "codeRepository": REPO,
    "image": SITE_URL + "/assets/og.png",
    "screenshot": SITE_URL + "/assets/frames.jpg",
}

TEMPLATE = """<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>{title}</title>
<meta name="description" content="{description}">
<link rel="canonical" href="{url}">
<meta property="og:type" content="website">
<meta property="og:site_name" content="{site_name}">
<meta property="og:title" content="{title}">
<meta property="og:description" content="{description}">
<meta property="og:url" content="{url}">
<meta property="og:image" content="{site_url}/assets/og.png">
<meta property="og:image:width" content="1200">
<meta property="og:image:height" content="630">
<meta name="twitter:card" content="summary_large_image">
<link rel="icon" type="image/png" sizes="32x32" href="/assets/favicon-32.png">
<link rel="apple-touch-icon" href="/assets/apple-touch-icon.png">
<link rel="preconnect" href="https://fonts.googleapis.com">
<link rel="preconnect" href="https://fonts.gstatic.com" crossorigin>
<link rel="stylesheet" href="https://fonts.googleapis.com/css2?family=IBM+Plex+Mono:wght@400;600&family=Inter:wght@400;500;600&family=Space+Grotesk:wght@500;700&display=swap">
<link rel="stylesheet" href="/assets/site.css">
{ld}</head>
<body>
<a class="skip-link" href="#main">Skip to content</a>

<header class="site-header">
    <div class="wrap site-header__inner">
        <a class="brand" href="/">
            <img class="brand__img" src="/assets/logo.png" alt="" width="44" height="39">
            <span class="brand__name">Open<span class="red">Pakon</span></span>
        </a>
        <nav class="site-nav" aria-label="Main">
{nav}
            <a class="nav-gh" href="{repo}" aria-label="GitHub">{github}<span class="nav-extra">GitHub</span></a>
            <a class="btn" href="/#download">Download</a>
        </nav>
    </div>
</header>

<main id="main">
{body}
</main>

<footer class="site-footer">
    <div class="wrap footer-grid">
        <div>
            <div class="footer-badge">Open<span class="red">Pakon</span></div>
            <p class="footer-note">
                Free software for the Kodak Pakon F-135 and F-135+ film scanners.<br>
                An unofficial, independent project. Not affiliated with or endorsed by Kodak or Pakon.
            </p>
        </div>
{footer}
    </div>
</footer>
</body>
</html>
"""


def link(href, text, current=None, cls=""):
    attrs = f' class="{cls}"' if cls else ""
    if href == current:
        attrs += ' aria-current="page"'
    return f'<a{attrs} href="{href}">{text}</a>'


def faq_html(faq):
    items = "\n".join(
        f'<details>\n    <summary>{q}</summary>\n'
        f'    <div class="faq__a"><p>{a}</p></div>\n</details>'
        for q, a in faq)
    return f'<div class="faq">\n{items}\n</div>'


def plain(fragment):
    """Text of an HTML fragment, for structured data."""
    return html.unescape(re.sub(r"<[^>]+>", "", fragment))


def json_ld(data):
    text = json.dumps(data, ensure_ascii=False, indent=1).replace("</", "<\\/")
    return f'<script type="application/ld+json">\n{text}\n</script>\n'


def build_page(src):
    m = re.match(r"\s*<!--(.*?)-->\s*", src.read_text(), re.S)
    if not m:
        raise SystemExit(f"{src.name}: missing the settings comment")
    meta = json.loads(m.group(1))
    body = src.read_text()[m.end():]

    name = src.stem
    path = "/" if name == "index" else "/404.html" if name == "404" else f"/{name}/"
    url = SITE_URL + path

    faq = meta.get("faq", [])
    body = re.sub(r"\{\{download:([a-z-]+)( ghost)?\}\}",
                  lambda m: download_button(m.group(1), bool(m.group(2))), body)
    body = body.replace("{{downloads}}", DOWNLOADS).replace("{{faq}}", faq_html(faq))

    ld = ""
    if meta.get("app"):
        ld += json_ld(APP_LD)
    if faq:
        ld += json_ld({
            "@context": "https://schema.org",
            "@type": "FAQPage",
            "mainEntity": [{"@type": "Question", "name": plain(q),
                            "acceptedAnswer": {"@type": "Answer", "text": plain(a)}}
                           for q, a in faq]})

    nav = "\n".join(" " * 12 + link(h, t, path, "nav-extra") for h, t in NAV)
    footer = "\n".join(
        f'        <div>\n            <h2>{heading}</h2>\n            <ul>\n'
        + "\n".join(f"                <li>{link(h, t, path)}</li>" for h, t in links)
        + "\n            </ul>\n        </div>"
        for heading, links in FOOTER)

    page = TEMPLATE.format(
        title=html.escape(meta["title"]), description=html.escape(meta["description"]),
        url=url, site_url=SITE_URL, site_name=SITE_NAME, ld=ld,
        nav=nav, body=body.strip(), footer=footer, repo=REPO, github=GITHUB_ICON)

    dest = OUT / ("404.html" if name == "404" else
                  "index.html" if name == "index" else f"{name}/index.html")
    dest.parent.mkdir(parents=True, exist_ok=True)
    dest.write_text(page)
    return None if name == "404" else url


def main():
    # empty public/ rather than remove it, so a server running in it survives
    OUT.mkdir(exist_ok=True)
    for old in OUT.iterdir():
        shutil.rmtree(old) if old.is_dir() else old.unlink()
    shutil.copytree(SRC / "assets", OUT / "assets")

    urls = [u for u in map(build_page, sorted((SRC / "pages").glob("*.html"))) if u]
    urls.sort(key=lambda u: (u != SITE_URL + "/", u))

    (OUT / "sitemap.xml").write_text(
        '<?xml version="1.0" encoding="UTF-8"?>\n'
        '<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">\n'
        + "".join(f"  <url><loc>{u}</loc></url>\n" for u in urls)
        + "</urlset>\n")
    (OUT / "robots.txt").write_text(
        f"User-agent: *\nAllow: /\n\nSitemap: {SITE_URL}/sitemap.xml\n")

    print(f"{len(urls)} pages for {SITE_URL} → {OUT.relative_to(ROOT.parent)}/")


if __name__ == "__main__":
    main()
