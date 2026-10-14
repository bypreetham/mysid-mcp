# Dev server and dependencies: `mysid dev`, `mysid deps`

## `mysid dev`

A dev server never exits, so it cannot run through `mysid exec`. `mysid dev` starts it detached and keeps its output in `.mysid/dev.log`.

```
mysid dev                  # npm / pnpm / yarn run dev (else start); --port 5174 passes a port
mysid dev log              # server errors from the log, else its last lines; --lines 40 for the raw tail
mysid dev status           # running? URL, port answering, uptime
mysid dev stop             # stops it and everything it started (the whole process tree)
```

Start waits (up to 30 s, `--wait`) until the server prints a local URL **and** that port accepts connections, then answers:

```
DEV RUNNING http://localhost:5173/ (npm run dev, pid 21756, ready in 0.9s)
log: .mysid/dev.log   stop: mysid dev stop
```

- Several URLs are printed (Local, Network): the local one is reported.
- A server that exits (a syntax error, a missing module, the port in use) gives `DEV FAILED` with only the error lines.
- Starting again while it runs reuses it (`(already running)`).
- Output colours are stripped; `BROWSER=none` is set so no window opens. The agent opens the URL in its own browser pane.
- State is in `.mysid/dev.state.json`; a server whose process is gone is reported as not running.

`mysid build` proves the code compiles. Opening the URL proves it renders, and `mysid dev log` shows server errors (not browser console errors).

## `mysid deps`

```
scanned 159 source file(s); 19 package(s) imported
MISSING from package.json (2):
  lucide-react  (12 files: Navbar.tsx, Hero.tsx, Footer.tsx, ...)
  react-slick  (1 file: Gallery.tsx)
install: npm install lucide-react react-slick
listed in dependencies but never imported (1): left-pad
  (a config file, script or CLI may still use them: check before removing)
```

It reads `import ... from`, `import "x"`, `export ... from`, `require()` and `import()` in ts, tsx, js, jsx, mjs, cjs, vue and svelte files (not `node_modules`, build output, or `.gitignore`d files). Not packages: relative imports, Node built-ins, `node:`/`virtual:` specifiers, and tsconfig / jsconfig path aliases (`@/`, `~/`). `lodash/fp` counts as `lodash`; `@scope/name/deep` as `@scope/name`. The install line uses the project's package manager (npm, pnpm or yarn, by lock file). Versions are not guessed: the package manager picks the latest.
