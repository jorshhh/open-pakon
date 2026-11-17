#!/bin/bash
# Build the page (web/: Vue + Tailwind, Vite) into web/static, which every
# app copies. Needs Node 20.19+ or 22.12+ with npm. The packaging scripts run it.
set -euo pipefail
# already built for this package (build-deb.sh builds it before Docker)
if [ "${PAKON_WEB_BUILT:-}" = 1 ]; then exit 0; fi
WEB="$(cd "$(dirname "$0")/../web" && pwd)"
command -v npm >/dev/null || { echo "building the page needs Node.js (npm)" >&2; exit 1; }
printf '\n==> page (web/)\n'
cd "$WEB"
npm ci --no-audit --no-fund
npm run build
[ -f static/index.html ] || { echo "the page build left no web/static/index.html" >&2; exit 1; }
