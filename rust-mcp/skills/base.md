---
name: mysid-base
description: How to explore, edit, build and commit code with the mysid CLI/MCP tools (search, read, graph, patch, build, commit). Use at the start of any coding task in a workspace where mysid is installed.
---
# mysid: base workflow

mysid gives you small, token-cheap answers about a codebase. Use it instead of raw `grep`, `cat`, `find` and `git`.

## Rules

1. **Run mysid from inside the project folder.** It finds the project from your current folder (the nearest `.git`, `package.json`, `*.sln`, Gradle file, ...); there is nothing to set and other agents on other projects do not interfere. From elsewhere, add `--workspace_root <path or name>`, or give absolute file paths, which name their own project. `mysid set <path>` (alias of `set_workspace`) is optional.
2. **File names, not paths.** `mysid read MusicService.kt`, never `app/src/main/.../MusicService.kt`. mysid finds the file anywhere. Add a folder only when two files share a name (`ui/Foo.kt`); the error lists the choices. Output prints short names, so copy them as shown.
3. **Bundle reads.** Several slices go in ONE call: `mysid read "A.kt#L10-40" "B.cpp#L100-140"`.
4. **Do not pipe to `head`/`tail`/`2>&1`.** Output is already capped, and the footer says how to narrow it.
5. **Checkpoint, edit, build, `mysid diff`, then commit.** `restore` previews first and needs `--yes` to apply.
6. **Never push.** `mysid commit` only commits locally. Pushing is the user's decision.

## Workflow

| Step | Command | Gives you |
|---|---|---|
| Start | `mysid new react my-app --dir D:/apps` | a new Vite + React + TypeScript project, dependencies installed (needs Node; then work inside that folder, or pass `--workspace_root`) |
| Orient | `mysid list src/ui` | every entry of a folder (folders `D`, files `F`) |
| | `mysid graph` | where the code lives, in one call: it follows `src/main/java/...` chains and opens the big folders, with file and symbol counts |
| Find | `mysid search "term" --types kt` | `File.kt:42: line` hits (max 25; add `--regex`, `--case`, `--context 1`) |
| Understand | `mysid graph Symbol` | callers and callees; `--depth 2` hops, `--flow` as trees |
| | `mysid overview File.kt` | the outline: types and signatures with line numbers, no code |
| | `mysid read File.kt` | a short file whole (up to 150 lines); a longer source file returns its outline and a hint instead; `File.kt#L10-40` the code of a slice; `mysid read Symbol` a definition and its callers |
| Safety | `mysid checkpoint "before refactor"` | snapshot before a batch of edits (branch and history untouched) |
| Change | `mysid patch File.kt "old" "new"` | exact replace; refuses if 0 or 2+ matches (add context, or `--replace_all`; `--dry-run` previews) |
| | `mysid write File --file x.txt` | create a new file with exact content (refuses to replace one unless `--overwrite`); `mysid append File --file x.txt` adds to the end |
| | `mysid apply` (blocks on stdin or `--file`) | several edits at once, no shell quoting: use it for anything multi-line or containing quotes, `$`, backticks or backslashes |
| Verify | `mysid check` | TypeScript projects: `tsc` type-check, errors only (fast, no build) |
| | `mysid build` | result and errors only; the full log is `.mysid/build.log` |
| Review | `mysid diff` | files changed since the checkpoint (A/M/D, +/-) with the first lines of each change |
| Undo | `mysid restore` (preview), `mysid restore --yes` | puts files back to the checkpoint; a safety checkpoint is taken first |
| Save | `mysid commit "message"` | local commit, secrets skipped; `mysid history` lists the last 5 |

## Edits with quotes, `$`, backticks or backslashes

Shell quoting mangles code. Do not escape it: send the text so the shell never parses it.

```
mysid apply --file edit.txt        # write edit.txt with your file tool, then apply it
mysid apply <<'EOF'                # bash: the quotes around EOF are required
Page.tsx
<<<<<<< SEARCH
const msg = "hi"
=======
const msg = `Hello ${user.name}`
>>>>>>> REPLACE
EOF
mysid patch Page.tsx --old-file a.txt --new-file b.txt     # one replacement, text from files
```

One call may hold many blocks for many files; all apply or none is written (`NOT APPLIED` names the block that failed). In PowerShell
pipe a single-quoted here-string: `@'...'@ | mysid apply`.

## Examples

```
mysid search "resolveAudioStream" --types kt
mysid graph StreamResolver --depth 2
mysid read StreamResolver.kt#L40-90 Player.kt#L200-240
mysid patch Player.kt "val retries = 3" "val retries = 5"
mysid build
mysid commit "Raise stream retries to 5"
```

## Things to know

- `mysid build` waits about 20 s, then returns a progress line and keeps building. Check with `mysid build status`, cancel with `mysid build stop`.
- `mysid exec "cmd"` runs a shell command in the workspace (30 s timeout; `--timeout N`; `--out file` also saves everything it printed, even when it is killed).
- A misspelled command prints `Did you mean ...`; `mysid help` lists everything, `mysid <command> --help` details one.
- `.mysid/` in the project holds mysid's cache, logs and index. Do not edit or commit it.

## Stay inside mysid

Reach for these before a shell command (`head`, `grep`, `cp`, `git status`, `cargo test`):

- `--head N`, `--tail N`, `--grep <regex>`, `--grep-v <regex>` on any command trims its output: `mysid build --tail 20`, `mysid search "retry" --grep Player`.
- `mysid test [name]`: one line when green, only the failing tests when red (Cargo, npm, Gradle, pytest).
- `mysid status`: branch, ahead/behind, changed files. `mysid branch` lists branches.
- `mysid bootstrap` (git init only), then edit `.gitignore`, then `mysid commit --branch main --message "Initial commit"`. `mysid vcs` prints this workflow. `.gitignore` does not untrack files that are already committed.
- `mysid publish <repo-url>`: first upload to GitHub. It previews by default; `--yes` pushes. **Ask the user before using `--yes`**: it publishes the code.
- `mysid copy <src>... <dest>`: files or folders, many in one call; it refuses to overwrite unless `--overwrite`.
- `mysid set .` prints the source root, language, entry and the next-level folders. Then `mysid list <folder>` (sub-folders ranked by code, code files with line counts; `mysid overview <folder> --match word` to find by name) -> `mysid overview <file>` -> `mysid read <file>#L10-60` -> `mysid graph <Symbol>`. `mysid tree` is the folder map.
- Three ways to change code: `mysid patch <file> "old" "new"` (you know the exact old text), `mysid edit <file>#L10-20 "new text"` (you know the line numbers from `mysid read`; `--insert-after`, `--insert-before`, `--delete`, `--expect "text"` to refuse when the lines moved), `mysid replace "old" "new" --dry-run` (the same change in many places).
- `mysid exec --errors "<build command>"` keeps only the error lines (error, fatal, LNK1xxx, undefined reference) and the exit line. mysid's flags go before the command, after ONE quoted command, or before `--`.
- `mysid msvc <cmd>` runs with the Visual Studio C++ environment set up (cl, link, nmake); `mysid emsdk <cmd>` (or `mysid wasm`) with Emscripten (emcc, emmake).
- `mysid exec` is for what mysid does not cover; a command with `;` runs in PowerShell, `--shell bash` selects Git Bash.
