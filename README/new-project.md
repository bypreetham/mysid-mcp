# Creating a project: `mysid new`

```
mysid new react my-app                     # in the current folder
mysid new react my-app --dir D:\apps       # D:\apps\my-app
mysid new react my-app --template react-swc-ts
```

## React

1. Checks the name (lower case letters, digits, `-`, `_`, `.`; `"My App"` is refused with the suggestion `my-app`) and that
   `<dir>/<name>` is missing or empty. Nothing is ever overwritten or merged.
2. Runs the official generator: `npm create vite@latest <name> -- --template react-ts` (templates: `react-ts` default,
   `react`, `react-swc-ts`, `react-swc`). This downloads and runs `create-vite` from npm, so it needs Node.js and network access.
3. Runs `npm install` in the new folder.
4. Prints the path and the next steps:

```
Created React project 'demo-app' (react-ts, Vite) at D:\apps\demo-app
19 files, dependencies installed.
next:
  cd "D:\apps\demo-app"          (mysid finds the project from the folder you are in)
  mysid build            (tsc + production build)
  mysid skill install    (workflow guides for this workspace)
  mysid commit "Initial project"   (creates the git repository)
```

Measured: about 27 s for create plus install, and `mysid build` on the result passes in 9 s.

- **Default folder.** Without `--dir` the project goes in the *current folder*, never the saved workspace (which is
  usually a different project). Over MCP, pass `dir`.
- **No hidden steps.** It does not change the saved workspace, run `git init`, or build; each is one command in the list above.
- **Failures** say which step failed (create or install) with the last lines of npm's output; a project whose install failed
  is kept so `npm install` can be re-run.

## Android

Not available yet: it needs a built-in template (Gradle files, manifest, activity, wrapper) that is checked by a real build.
