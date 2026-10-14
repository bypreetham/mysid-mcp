---
name: mysid-react
description: Create, explore, edit and build a React + TypeScript (Vite) app with mysid: new project, component and hook graph, tsc/Vite build errors, lint, tests. Use for any React, Vite, TypeScript or JavaScript web frontend task in a workspace with mysid.
---
# mysid: React workflow

Read `mysid-base` first (file names, search, read, patch, checkpoint/diff/commit). This adds what is specific to a React app.

## Start a project

```
mysid new react my-app --dir D:/apps     # Vite + React + TypeScript, dependencies installed (needs Node and network)
cd D:/apps/my-app                        # mysid finds the project from the folder you are in
mysid skill install                      # optional: these guides as Claude Code skills in the project
```

The name is lower case (`my-app`). It only creates in a missing or empty folder.

## Find your way

```
mysid graph                          # map: folders and file/symbol counts
mysid graph src/components           # drill into a folder
mysid graph Navbar                   # who renders it, what it renders and calls
mysid graph useDarkMode --depth 2    # a hook: components that call it, 2 hops
mysid overview App.tsx               # outline: components, hooks and functions with line numbers (no code)
mysid read App.tsx                   # a short file whole; a long one: its outline + how to read a slice
mysid read App.tsx#L20-70            # a slice
mysid search "useState" --types tsx  # --types ts,tsx,css; add --regex, --context 1
```

- Names are file names: `App.tsx`, `Navbar.tsx`. Two files with the same name (`index.ts` in two folders) need a folder: `ui/index.ts`.
- `<Navbar />` in JSX counts as a call, so `graph Navbar` shows the components that render it.
- Top-level code is not a symbol: the `createRoot(...).render(<App />)` in `main.tsx` is not shown as App's caller. Use `mysid search "<App"` for that.
- Props types, `interface` and `type` declarations are not call-graph nodes; find them with `search`.

## Change, build, verify

```
mysid checkpoint "before nav change"
mysid patch Navbar.tsx "<Logo />" "<Logo size={24} />"
mysid build
mysid diff
mysid commit "Smaller logo in navbar"
```

- `mysid check` is the fast loop: `tsc` only (`tsc -b` for the Vite template), `CHECK OK` or the errors. Use it after each edit; `mysid build` before you finish.
- `mysid build` runs the project's `npm run build` (for the Vite template: `tsc -b` then `vite build`). Success is one line. **Failure prints only the errors**, as `src/App.tsx(124,7): error TS2322: ...`: fix them, build again. The full output is `.mysid/build.log`.
- Match JSX exactly when patching (indentation, quotes). If `patch` reports several matches, include the line above or below in both strings, or use `--replace_all`.
- Type-check: `mysid check`. Tests: `mysid test [name]` (runs `npm test` once, one line when green, failing tests when red; a project with no `test` script says so). Lint goes through `exec`: `mysid exec "npm run lint"` (the Vite template uses `oxlint`).
- Add a package: `mysid exec "npm install axios" --timeout 180`, then commit `package.json` and the lock file with the change.
- Do not start the dev server with `mysid exec "npm run dev"`: it never ends and is stopped at the timeout. Ask the user to run it in their own terminal.

## Examples

```
mysid search "fetch(" --types ts,tsx
mysid graph useAuth --depth 2
mysid read AuthContext.tsx#L1-60 LoginPage.tsx#L10-50
mysid patch LoginPage.tsx "const [email, setEmail] = useState('')" "const [email, setEmail] = useState<string>('')"
mysid build
mysid exec "npm run lint"
mysid diff
```

## Things to know

- `node_modules`, `dist` and `build` are skipped by search and graph. `.env` files are never committed by `mysid commit`.
- Files are `.tsx` for components and `.ts` for logic. `search --types` takes both: `--types ts,tsx`.
- A symbol called only from another language or from a config string (route names, env keys) will show no callers: use `search`.

## Release (Netlify)

```
mysid release                 # npm run build, then netlify deploy --prod; waits up to 20 s, then prints a progress line
mysid build status            # progress, or the final report
```

- One-time setup, by you: `npm install -g netlify-cli`, `netlify login`, and `netlify link` in the project. `mysid release` checks all of this first and says what is missing.
- Success is `RELEASE OK` and the live `URL:`. **Failure prints only the errors** with the step that failed (preflight, build, deploy).
- The full output of every step, with timed progress lines, is `.mysid/release.log`; a person can follow a live deployment with `Get-Content .mysid\release.log -Wait`.

## Check that it runs: dev server, and missing packages

```
mysid dev                 # starts "npm run dev" in the background; answers with the URL once it accepts connections
mysid dev log             # server errors from .mysid/dev.log (else its last lines); --lines 40 for a raw tail
mysid dev status          # running? URL, port answering, uptime
mysid dev stop            # stops it and everything it started
mysid deps                # imported packages that package.json is missing, with the files and the install command
```

- Do not start the dev server with `exec`: it never exits. `mysid dev` returns as soon as the URL answers (`DEV RUNNING http://localhost:5173/ ...`), and a failed start prints only the error lines (`DEV FAILED`).
- `mysid build` proves it compiles; opening the `dev` URL in a browser proves it renders. After a change, `mysid dev log` shows server-side errors (a failed import, a syntax error); browser console errors are only visible in the browser.
- Starting twice reuses the running server. Stop it when you are done.
- `mysid deps` replaces grepping for imports: it reads import, export-from, require and dynamic import, ignores relative imports, Node built-ins and tsconfig aliases (`@/`), and lists listed-but-unused dependencies (check config files and scripts before removing one).
