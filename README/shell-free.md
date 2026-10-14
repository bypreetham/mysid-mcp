# Staying inside mysid: no head, grep, cp or git status needed

An agent that has to leave mysid for a shell command pays for it in quoting mistakes, platform differences (`cp` vs `Copy-Item`, `diff` vs `Compare-Object`) and tokens. Counting the shell commands agents actually ran showed what was missing; these commands fill the biggest gaps.

## Trim any output: `--head`, `--tail`, `--grep`, `--grep-v`

```
mysid build --tail 20                 # last 20 lines of the report
mysid search "retry" --grep Player    # only hits mentioning Player
mysid list src --grep "\.kt$" --head 30
mysid history --grep-v merge
```

- Works on every command's printed output (not on `exec`'s command words: for `exec` put the flag first, `mysid exec --tail 20 "cargo test"`). Over MCP, tools have their own limits.
- `--grep` is a regex; a lower-case pattern ignores case, a pattern with capitals is exact; a pattern that is not valid regex is searched literally.
- Order: grep, then grep-v, then head, then tail. When lines were removed the last line says so: `[4 of 35 lines: --head 4]`.

## `mysid test`

```
mysid test                  # TESTS OK (Rust, 107 passed, 18.7s)
mysid test checkpoint       # only tests matching the name
```

The stack is detected: `cargo test`, `npm/pnpm/yarn test` (a `test` script is required; `CI=true` switches watch modes off), Gradle (`testDebugUnitTest` for Android, else `test`), pytest. Green is one line. Red lists each failing test with its message and location (a Rust panic's `file:line`, jest/vitest assertion lines, Gradle `Test > name FAILED`, pytest `FAILED` lines); a compile error lists the errors. The whole output is `.mysid/test.log`. `--timeout` (default 600 s) kills a run that hangs.

## `mysid status`, `mysid branch`

```
main -> origin/main [ahead 1] - 3 change(s): 1 staged, 1 unstaged, 1 untracked
 M src/a.rs
A  src/b.rs
?? notes.txt
last commit: 8b10fba Remove loop.txt from the repository (2 days ago)
```

Read only. Secret-looking files (`.env`, keystores) are marked, because `mysid commit` leaves them out. `mysid branch` lists local branches with their last commit. Creating and switching branches, and pushing, are not mysid's job.

## `mysid copy` (also `mysid file copy`)

```
mysid copy ../styleAI/project/src docs/reference/src       # a folder: its contents become the destination
mysid copy a.json b.json docs/                             # several files into a folder
mysid copy ../other/Button.tsx src/ui/ --overwrite
```

Sources can be anywhere (a sibling project, a bare file name); the destination must be inside the workspace. A folder is copied whole except `.git`, `node_modules` and `.mysid` (the answer names what it left out). If any source is missing or any target already exists, **nothing** is copied and the answer says which; `--overwrite` replaces. Many files in one call replace a shell loop of `cp`.
