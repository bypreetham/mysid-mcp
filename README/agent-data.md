# Where mysid keeps its data

mysid stores two kinds of data: **per project** (in a `.mysid/` folder inside the workspace) and **per user**
(under `%LOCALAPPDATA%\mysid\`). Nothing else is written outside the files you ask it to change.

## Per project: `<workspace>/.mysid/`

Created by the first command that needs it.

| File | What | Safe to delete? |
|---|---|---|
| `index.db` (+ `index.db-wal`, `index.db-shm` while in use) | The code index: files, symbols, call sites and call edges. See [Indexer.md](Indexer.md). | Yes. Rebuilt automatically on the next command. |
| `text.db` (+ `-wal`, `-shm`) | The search index: a trigram filter per text file, so `search` reads only likely files. See [Indexer.md](Indexer.md#text-index-for-search). | Yes. Rebuilt on the next search. |
| `build.log` | Live, complete output of the current/last build. Overwritten by each build. | Yes. |
| `build.status.json` | State of the current/last build (`running` / `ok` / `failed`), current stage, timings, and the final report. | Yes (a running build rewrites it every second). |
| `build.job.json` | The command the build worker was asked to run. | Yes. |
| `build.stats.json` | Task count and duration of the last successful build per stack; gives the progress percentage. | Yes (the percentage disappears until the next successful build). |
| `debug.log` (+ `debug.log.old`) | Keyword-filtered device log lines recorded by `mysid debug`. See [android-dev.md](android-dev.md#debugging-with-device-logs). | Yes. |
| `debug.job.json`, `debug.state.json`, `debug.touch`, `debug.cursor` | The recorder's keywords, heartbeat, last use and the read position for `--new`. | Yes (stop the recorder first: `mysid debug stop`). |
| `.gitignore` | Contains `*`, so the whole folder is ignored by git. | Leave it; it is recreated if missing. |

The folder ignores itself, so it never shows up in `git status` and never needs a line in your own `.gitignore`.

Typical sizes: `index.db` is about 4 MB for a 300-file Android project; `build.log` is the size of one Gradle run.

### Rebuild or reset

```
mysid exec "rmdir /s /q .mysid"      # remove everything; the next command recreates what it needs
```

Deleting only `index.db` (and the `-wal`/`-shm` files) forces a full re-index. This also happens by itself when the
schema, a parser or the call-linking rules change, or if the file is unreadable.

## Per user: `%LOCALAPPDATA%\mysid\`

| Path | What |
|---|---|
| `bin\mysid.exe` | The installed binary (also mirrored to `~\.local\bin\mysid.exe`). Installed by `install.ps1`. |
| `workspaces.json` | The workspace registry (`registry`: folder name / alias → path), `pins` (a `mysid set` made in a folder applies to that folder and below), and the older machine-wide `current` value, now read only as a last resort. |
| `cache\cache_<hash>.json` | A per-workspace LRU (150 entries) of bare file names → relative paths, so `mysid read Foo.kt` finds the file without a full path. |

## Which workspace is used

See [workspaces.md](workspaces.md). In short, each command works out its project from its own surroundings (flag,
`MYSID_WORKSPACE`, an absolute path in its arguments, a `set_workspace` pin for the current folder, or the project around
the current folder), so several agents on several projects do not interfere. The old machine-wide saved workspace is
used only when the current folder is not inside a project.

## Not used any more

- **`agent-data/overview.json`**: an earlier per-project cache written by `mysid overview`. That feature was removed;
  the code index replaced it. An `agent-data/` folder in your project is a leftover and can be deleted (and removed
  from `.gitignore`).

## Diagnostics

| Variable | Effect |
|---|---|
| `MYSID_TIMING=1` | Prints where the time goes (file walk, index refresh, query + render) to stderr. |
