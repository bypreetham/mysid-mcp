# Releasing a web project: `mysid release`

In a project with a `package.json` (and no `gradlew`), `mysid release` builds and deploys to Netlify production.

```
mysid release              # waits up to 20 s, then prints a progress line
mysid build status         # progress, or the final report
mysid build stop           # cancel
```

## One-time setup (yours, not mysid's)

```
npm install -g netlify-cli
netlify login
netlify link               # in the project folder (creates .netlify/state.json); or set NETLIFY_SITE_ID
```

No credentials pass through mysid; the Netlify CLI uses its own login (or `NETLIFY_AUTH_TOKEN`).

## What it does

1. **preflight**: `package.json` has a `build` script, `netlify` is on PATH, the site is linked. Each missing piece is one line saying how to fix it.
2. **build**: `npm run build` (`pnpm` / `yarn` when their lock file is present). Output folder: `publish` from `netlify.toml`, else `dist`, `build` or `out` (must contain `index.html`).
3. **deploy**: `netlify deploy --prod --dir <output> --json`, then the live URL is read from the result.

It runs in the same detached worker as `mysid build`, so it survives a caller that times out, and a second `mysid release` attaches to the running one.

## What is printed

While it runs, a few timed lines (on stderr):

```
[0:00] preflight: package.json, netlify CLI, site link
[0:00] build: npm run build
[0:09] build: ok, output dist/
[0:09] deploy: netlify deploy --prod --dir dist
[0:31] done: https://demo.netlify.app
```

The answer is one line and the URL:

```
RELEASE OK (React, Netlify, 31s)
URL: https://demo.netlify.app
deploy: https://64f...--demo.netlify.app
log: .mysid/release.log
```

A failure prints **only the error lines** and the step that failed, exit code 1:

```
RELEASE FAILED at build (React, Netlify, 9s)
  src/App.tsx(3,1): error TS2304: Cannot find name 'x'.
log: .mysid/release.log
```

## `.mysid/release.log`

Rewritten on every release. It holds the timed progress lines **and** the complete output of every command (indented), flushed line by line, so a person can watch a deployment live:

```powershell
Get-Content .mysid\release.log -Wait
```

The Android flavour of `mysid release` (version bump, bundle) is in [android-dev.md](android-dev.md).
