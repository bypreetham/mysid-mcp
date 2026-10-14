# Workspaces and several agents

Several agents often run at once, each on its own project. mysid therefore decides the workspace **per command, from that
command's own surroundings**. No file, variable or process shared between agents can send one of them to the wrong project.

## How a command finds its project

The first rule that matches wins:

| # | Rule | Example |
|---|---|---|
| 1 | `--workspace_root <path or name>` (or `--project_path`) | `mysid build --workspace_root D:\apps\web-ui`; a registered name such as `web-ui` also works |
| 2 | `MYSID_WORKSPACE` environment variable | a launcher can set it per agent or per terminal |
| 3 | An absolute path in the arguments | `mysid read D:\apps\web-ui\src\App.tsx#L1-40` runs in `web-ui`. If the command names files in two different projects, nothing is guessed. |
| 4 | A `set` pin made in this folder (or one above it) **by this session** | `mysid set D:\apps\web-ui` (same as `set_workspace`) pins only the folder it was run from and the folders below it, for the agent or terminal that ran it (see below) |
| 5 | The project around the current folder: the nearest ancestor with `.mysid`, `.git`, `package.json`, `*.sln`, a Gradle file, `Cargo.toml`, `pom.xml`, `pyproject.toml`, `go.mod`, `CMakeLists.txt` or `Package.swift`; otherwise the current folder itself | an agent started in `web-ui\src` works on `web-ui` |
| 6 | The old machine-wide saved workspace | only when the current folder is no project (a drive root, the home folder, a system folder). The first line says `[workspace: ... (saved earlier; this folder is not in a project)]`. |

In a monorepo the nearest marker wins: a `package.json` in `web-ui` beats the repository's `.git` above it.

## Pins belong to a session

An agent's shell tool starts a new shell for every command and often resets the working folder to the same place, so
neither the folder nor the shell says who is asking. mysid files a pin under the **session**: the nearest ancestor process
that is not a shell (`claude.exe`, the terminal program, an editor), found by walking up the process tree. The pin key is
`claude:18892|d:\work`.

- Two agents that share one folder each keep their own pin: one agent's `mysid set` never changes the other's project.
- An agent's pin survives its shell being reset, because the session (the `claude.exe` process) is the same for every command.
- Pins of sessions that have ended are dropped the next time anyone runs `mysid set`, so a reused process id cannot inherit one.
- Before writing, `set` merges the pins already on disk, so two agents setting at the same moment lose nothing.
- Pins made by an older mysid (keyed by folder alone) are used only when no session can be identified.
- `mysid get_workspace` says `chosen from: a mysid set pin for this folder (made in this session)`.

## What this means for an agent

- `--workspace <path or name>` is accepted as a spelling of `--workspace_root`. `mysid get_workspace` shows the project and what it was
  chosen from (`the project around the current folder`, `a mysid set pin for this folder`, `the MYSID_WORKSPACE environment variable`, ...).
- Run mysid from inside the project folder and do nothing else. `set_workspace` is optional (aliases, or working from
  elsewhere). The skills say so.
- Over MCP the server uses the folder the client started it in; `set_workspace` there applies to that server process only.
- `--workspace_root` accepts a path or a name from the registry (`%LOCALAPPDATA%\mysid\workspaces.json`, filled as projects are used).

## What is isolated, what is shared

| Per project (in its own `.mysid/` and git repo) | Shared on the machine |
|---|---|
| code index, text index, build worker and log, debug recording, checkpoints | `workspaces.json` (aliases and pins), written atomically and only when something changed; the file-name cache (one file per project) |

Two agents on the **same** project share one WAL-mode database safely.

## Still shared: the phone

Two agents debugging different projects on one phone share one logcat, and `mysid debug` clears it. Use `ANDROID_SERIAL` to give
each agent its own device. A per-device lock is not built yet.

## Notes

- A build's `.mysid/build.log` is per project, so builds in different projects never mix their output.
- The old `current` entry in `workspaces.json` is only read (rule 6); `set_workspace` no longer writes it.

## Orientation: what `mysid set` prints, `mysid tree`, `overview --depth`

`mysid set .` answers with the map an agent needs next, not a directory listing:

```
Workspace set to 'G:\products-c1\TwoScansAway\working' (...)
source root: src/ (TypeScript/React, 118 code files) · entry: src/main.tsx
  src/components (70) · src/pages (7) · src/lib (12) · src/hooks (5) · src/contexts (2)
other: plan/ (4), public/ (6 files) · +14 config files (--all to list)
next: mysid list src/<folder>   then mysid overview <file>, mysid read <file>#L1-60, mysid graph <Symbol>   (mysid tree for the whole map)
```

- The **source root** is the top-level folder with the most code files; build output, `node_modules`, `.git` and copied libraries never count. The language comes from the file extensions (and `package.json`, `gradlew`, `pubspec.yaml` for the framework); the **entry** is the first of `main.tsx`, `index.ts`, `main.rs`, `MainActivity.kt`, `main.dart`, `Program.cs` ... found there.
- The second line is the **next level**. When one folder holds nearly all the code the line follows it down (`app/src/main/java/com/x/ui (40) · .../data (12)`) until the code splits.
- Other folders are ranked by code files; folders without code show `(6 files)`. Folders are never cut with "+N more"; configuration files (lockfiles, `package.json`, configs, README) collapse into `+14 config files`. `mysid set . --all` lists them and the dot-folders.
- `mysid tree [folder] [--depth 2]` is the cheap folder map (folders and code counts only, up to 120 lines). `mysid overview <folder> --depth 1` gives the same for one folder, and `mysid overview <folder> --match playlist` keeps only the files whose path, class or function contains the word.

The flow: `mysid set .` -> `mysid list <folder>` -> `mysid overview <file>` -> `mysid read <file>#L10-60` -> `mysid graph <Symbol>`.
