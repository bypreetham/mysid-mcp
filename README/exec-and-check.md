# Running commands and type-checking

## `mysid exec`

```
mysid exec "npx tsx tests/p1.ts" --timeout 120 --out logs/p1.txt
```

- Runs a shell command in the project (cmd.exe on Windows, sh elsewhere). Default timeout 30 s; `--timeout N` changes it. At the
  timeout the command **and everything it started** is killed, and the reply says so.
- `--out <file>` saves everything the command printed (stdout and stderr, with the command and its exit code as a header), also
  when it was killed. The reply keeps only the last 32 KB and adds `[full output: logs/p1.txt (N bytes)]`.
- An exit code of 1 with no output usually means "no match" (`grep`, `findstr`, `diff`), and the reply says so.

## `mysid check`

A fast type-check without building.

```
mysid check
CHECK OK (tsc -b, 5.9s)

CHECK FAILED (tsc -b, 1.3s) - 2 error(s)
  src/App.tsx(124,7): error TS2322: Type 'string' is not assignable to type 'number'.
  src/App.tsx(124,7): error TS6133: 'bad' is declared but its value is never read.
```

- In a project with a `tsconfig.json` it runs `tsc --noEmit`; a solution-style config (`"references"`, as in the Vite templates, where a
  plain `tsc --noEmit` would check nothing) runs `tsc -b`, the same step as the template's own build. A local `node_modules/.bin/tsc`
  is used, else `npx --no-install tsc`; when TypeScript is not installed the reply says to run `npm install`.
- Only error lines are shown (30 at most, then `+N more`). Exit status 1 on failure. `--timeout N` (default 120 s) kills a hung `tsc`.
- Other projects (no `tsconfig.json`) get `mysid lint`, as `check` always did.
- `mysid build` still does the full job (`tsc` then `npm run build`); `check` is the quick loop between edits.

## `mysid file delete`

A folder that is not empty is not deleted by accident: `NOT DELETED - big is not empty (3 item(s) inside). Use --recursive to delete it
and everything in it.` (exit 1). `--recursive` deletes it.

### Flags: before the command, after one quoted command, or after `--`

mysid's own flags (`--timeout`, `--out`, `--shell`, `--env`, `--head`, `--tail`, `--grep`, `--grep-v`, `--errors`, `--workspace_root`) are taken out of the line in these places only, so `docker logs --tail 50` and `git log --grep x` reach the command untouched:

```
mysid exec --tail 25 --errors cargo build             # before the command
mysid exec "npm run build" --timeout 120 --tail 30    # after ONE quoted command string
mysid exec --timeout 5 -- docker logs --tail 50 web   # `--` ends mysid's flags: the rest is the command
```

`--errors` keeps only the lines that say something went wrong (`error`, `fatal`, MSVC `LNK1xxx` / `warning C4xxx`, `undefined reference`, `cannot open/find`, `failed`, `exception`) and the `[exit N]` line, so a long build answers in a few lines. The last line says how many lines were dropped. It works on any command, not only `exec`.

### Toolchain environments: `mysid msvc`, `mysid emsdk`

```
mysid msvc cl /nologo /EHsc main.cpp          # Visual Studio C++ tools on PATH (cl, link, nmake, msbuild)
mysid msvc --errors "nmake /f Makefile.win"
mysid emsdk emcc main.cpp -O2 -o out.js        # Emscripten (emcc, em++, emmake, emcmake); `mysid wasm` is the same
mysid exec --env msvc "cl /?"                  # the long form
```

- **msvc** finds the newest Visual Studio that has the C++ tools (`vswhere`, then the usual folders), then runs `call vcvars64.bat && <command>` in cmd.exe. Windows only.
- **emsdk** finds the SDK from `EMSDK`, then `%USERPROFILE%\emsdk`, `C:\emsdk`, `D:\emsdk` (`emsdk_env.sh` elsewhere). Git Bash's `MSYSTEM` / `SHELL` variables are removed for the call, because `emsdk_env.bat` otherwise prints bash `export` lines instead of setting the environment.
- Not found: `NOT RUN:` and where it looked. The same flags as `exec` apply (`--timeout`, `--out`, `--tail`, `--errors`).

### Which shell `exec` uses

`mysid exec` runs the command in `cmd.exe` on Windows (`sh` elsewhere). `&&`, `|` and `>` work there; `;` does not, so a command with a `;` outside quotes is run in PowerShell instead and the answer says so. Choose explicitly with `--shell cmd|powershell|bash` (`bash` is Git for Windows' bash, not the WSL launcher). `--out <file>` must be inside the workspace: a path such as `../../x.log` is refused and the command is not run.
