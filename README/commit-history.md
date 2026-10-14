# Commit and history

`mysid commit "message"` saves a snapshot of the workspace with git; `mysid history` lists the latest ones. Both call
the `git` command line (git must be installed), so everything they make is ordinary git that you can inspect, amend or
push yourself.

```
mysid commit "Add usb debug recorder"
  -> 79ad4ff Add usb debug recorder - 4 files, +37 -9 [main]
     committed locally; no remote configured, nothing pushed

mysid history            # last 5
mysid history --past 10  # also: history 10, history -10, history -past 10 (max 100)
  -> 2 commit(s), newest first:
     c8f721b  2 hours ago  Add B  (1 file, +1 -0)
     79ad4ff  3 hours ago  Initial snapshot  (2 files, +2 -0)
```

## What `commit` does

1. Stages every change under the workspace folder (new, modified, deleted), respecting `.gitignore`.
2. **No repository?** It runs `git init` (branch `main`), writes local-only ignore rules to `.git/info/exclude` (build
   output such as `build/`, `.gradle/`, `target/`, `node_modules/`, `.mysid/`, `*.log`, and secret patterns; nothing in the
   project is modified), and makes the initial commit. It will not create a repository in a drive root or in your home
   folder.
3. **Secrets guard.** Files that look like secrets are taken back out of the commit and listed:
   `*.jks`, `*.keystore`, `*.pem`, `*.p12`, `*.pfx`, `*.key`, `.env`, `.env.*` (not `.env.example`),
   `keystore.properties`, `local.properties`, `id_rsa`, `id_ed25519`, `credentials.json`. A secret file that is already
   tracked keeps its committed version.
4. Commits with your git identity. If none is configured it commits as `mysid <mysid@localhost>` and says so.
5. Prints `hash message - N files, +added -removed [branch]`; `nothing to commit` when nothing changed.

It **never pushes** and adds **no attribution lines** to the message. A new repository has no remote, so the initial commit
stays local; add a remote and push yourself when you want it published.

## Setting up a repository: bootstrap, .gitignore, commit

```
mysid bootstrap                       # git init (branch main) and nothing else: nothing staged, nothing committed
mysid bootstrap --gitignore           # also write a .gitignore for this stack (never overwrites one)
# edit .gitignore: what must not be tracked
mysid commit --branch main --message "Initial commit"
mysid status
```

- **bootstrap** only initializes. A repository already here: it says `git repository already exists here: nothing was changed (branch main, 12 commit(s), origin ...)`. Inside a parent repository: it names it and creates nothing. Never in a drive root or the home folder.
- **commit** takes named arguments (`mysid commit "msg"` still works): `--message`, `--branch`, `--paths`.
  - `--branch X`: with no commits yet, the initial commit is made on `X`; already on `X`, nothing happens; `X` exists, mysid switches to it (git refuses when your changes would be overwritten, and the answer says so); `X` is missing, it is created from the current commit. A name git does not accept stops the commit.
  - `--paths a.kt b.kt`: only these are committed; other changes stay uncommitted. Without it, every change under the workspace.
  - The answer lists the files that went in (`files: a.kt, b.kt`), so nothing unrelated slips by.
- **.gitignore only affects untracked files.** A file that is already committed stays tracked after you add it to `.gitignore`; mysid notes it (`2 tracked file(s) match .gitignore but stay tracked ...: git rm --cached <file>`). It is also not a secret scanner: mysid still leaves secret-looking files out of a commit and refuses to `publish` a history that contains them.
- After `bootstrap`, the first `commit` and `status` the answer ends with a `next:` line when the state suggests one (no `.gitignore` yet, no commit yet, no remote yet). `mysid vcs` prints the workflow and the command list.

### Removing git: `mysid git purge`

The opposite of `bootstrap`: removes git from a project so it is as if git had never been used there. Irreversible, so it **previews unless `--yes`**.

```
mysid git purge                       # PREVIEW: what goes, what stays, and a warning for commits that exist on no remote
mysid git purge --yes                 # remove .git/
mysid git purge --gitignore --yes     # .git/ and .gitignore
mysid git purge --all --yes           # .git/ and every optional item below
```

| Removed | When |
|---|---|
| `.git/`: commits, branches, tags, stashes, the index, the repository config, remote URLs, mysid's checkpoints (`refs/mysid/...`) | always (with `--yes`) |
| `.gitignore` | `--gitignore` (it may hold project rules worth keeping) |
| `.gitattributes`, `.gitmodules` | `--gitattributes`, `--gitmodules` |
| `.git-blame-ignore-revs`, `.gitkeep` files | `--extras` |
| repositories inside subfolders (their own `.git`) | `--nested` |
| `--all` | every flag above |

Never touched: source code, documents, build files and every other project file; a copy on GitHub. The preview counts what the repository holds, lists the items found but kept (with the flag that removes each), masks any credential in a remote URL (`https://***@github.com/...`), and warns when commits exist only in this repository (they cannot be recovered). It refuses in a drive root or the home folder, and inside a parent repository (run it where the `.git` is). Git marks its object files read-only; mysid clears that before deleting, so it also works on Windows. A link is removed as a link, never entered. If a file is held open by another program the answer says `PURGE INCOMPLETE` with what is left; run it again. Afterwards `mysid bootstrap` starts a fresh repository. An agent must ask the owner before `--yes`.

### How the code is organised

`rust-mcp/src/tools/vcs/` is one folder with one registry. `mod.rs` maps each command name to its handler (`COMMANDS`: bootstrap, commit, status, branch, history, publish, checkpoint, diff, restore) and `main.rs` only asks `vcs::dispatch(name, ...)`. The handlers: `initializer.rs` (bootstrap, repository setup), `committer.rs` (commit, branch switching), `tracker.rs` (status, branch, history), `publisher.rs` (publish), `purge.rs` (git purge), `checkpoint.rs` (checkpoint, diff, restore). `flow.rs` is the flow mapper: a table of (command, repository state) -> next step. A new version-control command is one handler plus one line in the registry.

## Publish: the first upload to GitHub

`mysid commit` never pushes. `mysid publish` is the one deliberate exception, and it needs `--yes`.

```
mysid publish https://github.com/owner/repo.git       # PREVIEW: nothing is changed or uploaded
mysid publish owner/repo --yes                        # do it
```

Create the repository on GitHub first (empty, without a README). With `--yes` it:

1. creates the repository and an initial commit ("Initial commit", secret-looking files and build output left out) when there is none;
2. adds the remote `origin` (`--name up` for another name; an existing remote with another address is refused);
3. runs `git push -u origin <branch>` (3 minute limit).

The answer is `PUBLISHED https://github.com/owner/repo (N commits, branch main)`, or `NOT PUBLISHED:` and what to do. Failures are explained: repository not found (create it first), cannot sign in (git's credential manager once in a terminal, or the SSH address), the remote already has commits (mysid never force-pushes, so it stops).

Safety: sign-in is git's own, mysid never sees a password or token. It refuses when secret-looking files (`.env`, `*.jks`, `*.pem`, ...) are anywhere in the repository's history, preview included, because a push uploads the whole history. Uncommitted changes are not part of the push (the preview counts them). **An agent must ask the owner before `--yes`**: it sends the code to the internet. A local `file://` address works as a remote too (a backup, or a test).

## Checkpoint, diff, restore

The review-and-undo half of the flow. Checkpoints are git commits stored under `refs/mysid/checkpoints/<n>`: they never move a
branch, never touch the index or `HEAD`, and do not appear in `git log`.

```
mysid checkpoint "before refactor"   # #1: every file under the workspace, new files included
  ... edits, build ...
mysid diff                           # vs the latest checkpoint (or HEAD if none); `mysid diff 2`, `mysid diff HEAD`
mysid restore                        # PREVIEW: lists what would be restored, changes nothing
mysid restore --yes                  # apply; `mysid restore 1 --yes` for a specific checkpoint
mysid checkpoint list
```

```
diff vs checkpoint #1 "before refactor" (2 minutes ago): 3 file(s), +2 -1
 M  Main.kt  +1 -0
 A  New.kt  +1
 D  notes.txt  -1
--- Main.kt
@@ -1,2 +1,3 @@
 class Main
+fun extra() {}
```

- **No repository yet.** The first `mysid checkpoint` in a folder without git runs `git init`, `git add` and an initial commit (message = the checkpoint label, secrets and ignored files left out), then saves checkpoint #1. Nothing is pushed.
- **What a checkpoint holds.** The working tree under the workspace: modified and **new** files, minus ignored files and
  secret-looking files (same list as `commit`). Built in a private temporary index, so a staged-but-uncommitted state is not disturbed.
  No repository yet? One is created first, as `commit` does.
- **`diff`** compares the working tree with a checkpoint, a number, `HEAD`, or any git revision. It prints the file list (`A`
  added, `M` modified, `D` deleted, `+/-` counts), then the first 30 diff lines per file and 150 in total.
- **`restore`** previews by default. With `--yes` it first saves a safety checkpoint (`auto: before restore`), then writes the old
  content of modified files back, recreates deleted ones, removes files added since, and verifies that the tree now matches.
  The line `undo: mysid restore <n> --yes` brings back the state before the restore. Ignored and secret-looking files are
  never touched, and neither is anything outside the workspace folder (a workspace inside a bigger repo only sees its own files).
- **Exact bytes.** Files are stored as they are on disk, without git's line-ending or `.gitattributes` conversion, so a checkpoint
  never fails on `core.autocrlf` / `core.safecrlf` settings (a file with bare LF made `git add` die with "LF would be replaced
  by CRLF") and a restore returns every file byte for byte, mixed line endings included. Tested with `safecrlf=true`,
  `autocrlf=true` and `*.cs text eol=crlf`.
- **Limits.** Files over 5 MB are left out and named in the output (they are also ignored by `diff` and `restore`). More than
  20,000 files stops with a message: run it on the part you are changing with `--workspace_root <subfolder>` (the same folder for
  `diff` and `restore`).
- **Speed.** About 0.6-0.8 s for 77 files, almost all of it starting `git` processes (~60 ms each on Windows), so it is
  roughly constant for small projects.
- **"Latest"** means the latest checkpoint you made; the automatic "before restore" ones are only used when you name them.
- **Where they live.** `.git/refs/mysid/checkpoints`. Delete one with `git update-ref -d refs/mysid/checkpoints/<n>`.

## Where it fits

```
checkpoint -> edits -> mysid build -> mysid diff -> approve -> mysid commit
                                          \-> not happy: mysid restore --yes
```

The approval step stays with the human or the agent's user: mysid only performs each step.
