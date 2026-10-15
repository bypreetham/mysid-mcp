
use crate::help::*;
use crate::tools;

pub(crate) fn get_tool_help(tool: &str) -> Option<&'static str> {
    match tool.to_ascii_lowercase().trim_start_matches('-') {
        "remaining" | "rest" | "more" => Some(get_remaining_help()),
        "symbols" => Some("mysid symbols: the symbols of one file, one per line\n\nUsage:\n  mysid symbols <file>      [KIND] name line N, for every class, function and method (file name only: Foo.kt)\n\nCheaper than mysid overview when only the names and line numbers matter; overview adds signatures and parameters.\n"),
        "all" | "full" | "commands" => Some(get_full_help()),
        "replace" => Some(
            "mysid replace: Global search-and-replace refactoring\n\n\
Description:\n  \
  Replaces occurrences of a string across the entire workspace, or within a specific file.\n  \
  Automatically skips binary files, respects .gitignore, and preserves line endings.\n\n\
Usage:\n  \
  mysid replace <old_term> <new_term> [options]         Replace across the entire workspace\n  \
  mysid replace <file> <old_term> <new_term> [options]  Target a specific file\n\n\
Options:\n  \
  --types <ext1,ext2>   Filter by file extensions (e.g. --types kt,cpp,rs)\n  \
  --path <subfolder>    Restrict replacements to a specific subdirectory\n  \
  --dry-run, --preview  Preview changes without modifying files\n\n\
Examples:\n  \
  mysid replace \"StreamResolver\" \"AudioStreamResolver\"\n  \
  mysid replace \"old_api\" \"new_api\" --types kt,java\n  \
  mysid replace \"AudioEngine\" \"NativeAudioEngine\" --path app/src/main/cpp\n  \
  mysid replace \"versionCode = 44\" \"versionCode = 45\" --dry-run\n"
        ),
        "read" => Some(
            "mysid read: the code of a file, or a symbol's definition\n\n\
Description:\n  \
  The code of a file. A range (File.kt#L10-60), --full or an offset always returns code. A bare read of a LONG\n  \
  source file (over 150 lines) returns its outline instead, with the exact command for a slice, so it never floods.\n  \
  The full outline is `mysid overview <file>`; a folder's entries are `mysid list <folder>`.\n\n\
Files are named by file name only (Foo.kt, not app/src/.../Foo.kt): mysid finds them anywhere in the\n  \
  workspace and prints short names too. Add a folder (ui/Foo.kt) only if two files share a name.\n\n\
Usage:\n  \
  mysid read <file>                   A file up to 150 lines whole. A longer source file: its outline (line numbers) and the command for a slice. Other text (logs): the first 120 lines and the command for the next\n  \
  mysid read <file#Lstart-end>        A slice (e.g. File.kt#L10-50 or File.kt:10-50); up to 200 lines, 600 with --full\n  \
  mysid read <slice1> <slice2> ...    Several slices in ONE call (Zero Peeking Rule)\n  \
  mysid read <symbol>                 A symbol's definition and its top callers / call sites\n  \
  mysid read <folder>                 Not a file: the reply points to `mysid list <folder>` and `mysid graph <folder>`\n\n\
Examples:\n  \
  mysid read StreamResolver\n  \
  mysid read MusicService.kt\n  \
  mysid read \"MusicService.kt#L100-150\"\n  \
  mysid read \"AudioEngine.cpp#L40-80\" \"Player.kt#L200-240\"\n"
        ),
        "search" => Some(
            "mysid search: Fast multithreaded codebase grep\n\n\
Description:\n  \
  High-speed regex/string search across the workspace respecting .gitignore.\n  \
  Returns token-lean 1-line hits (Foo.kt:12: snippet); pass those file names to read/patch.\n\n\
Usage:\n  \
  mysid search <query> [options]\n\n\
Options:\n  \
  --types <ext1,ext2>   Filter by file extensions (e.g. --types kt,cpp)\n  \
  --max <n>             Maximum results to return (default: 15, max: 500)\n  \
  --case                Case-sensitive search (default: false)\n  \
  --regex               Treat query as regular expression (default: false)
  --count               Per-file summary: how many matching lines in each file (most first), no hit text
  --offset <n>          Skip the first n matches (the next page of a long result)
  --group               Group hits by file (file name once, then '  line: text'): fewer tokens
  --all                 Also search md, txt and rst files (left out by default so prose does not drown code hits)
  --vendor              Also search external/, third_party/, vendor/ (copied libraries: left out by default)\n  \
  --context <n>         Show n context lines around matches (max: 2)\n\n\
Examples:\n  \
  mysid search \"pref_bluetooth\"\n  \
  mysid search \"resolveAudioStream\" --types kt\n  \
  mysid search \"Java_com_hypersonus\" --types cpp --max 20\n"
        ),
        "mkdir" => Some(
            "mysid mkdir: Recursive directory creation\n\n\
Description:\n  \
  Creates a new directory and all necessary parent directories in the workspace.\n  \
  Never fails with missing parent path errors.\n\n\
Usage:\n  \
  mysid mkdir <path>\n\n\
Examples:\n  \
  mysid mkdir app/src/main/assets/presets\n  \
  mysid mkdir tools/cache\n"
        ),
        "patch" => Some(
            "mysid patch: Surgical search-and-replace modification\n\n\
Description:\n  \
  Safely replaces exact target string in a file with new content.\n  \
  Matches exact text, then text differing only in CRLF/LF line endings, then text ignoring surrounding\n  whitespace; everything outside the match stays byte-identical. Refuses (changing nothing) if the target is\n  not found or matches more than once: the error gives the match count and line numbers. Add context to make\n  it unique, or pass --replace_all.\n\n\
Usage:\n  \
  mysid patch <file> <old_string> <new_string> [--dry-run] [--replace_all] [--show]\n  \
  mysid patch <file> --old-file a.txt --new-file b.txt   text from files (no shell quoting)\n  \
  mysid patch <file> --stdin           stdin: the old text, a line with only =======, the new text (no markers);\n                                       or SEARCH/REPLACE blocks (see mysid apply)\n\n\
Examples:\n  \
  mysid patch build.gradle.kts \"versionCode = 44\" \"versionCode = 45\"   (file name only; ui/Foo.kt if two share a name)\n"
        ),
        "patch_batch" => Some(
            "mysid patch_batch: several replacements in ONE file, in one call\n\n\
Usage:\n  \
  mysid patch_batch <file> --stdin              snippets on stdin, applied in order, ALL or NOTHING [--show] [--dry-run] [--replace_all]\n  \
  mysid patch_batch <file> --file snippets.txt  the same from a file\n  \
  mysid patch_batch --json batch.json           several files from a JSON list (applied in order; stops at the first failure,\n                                                earlier patches stay: use apply for all-or-nothing across files)\n\n\
Each snippet is the old text and the new text between markers (the text is sent on stdin, so quotes, $ and backslashes never matter):\n\n\
<<<<<<< SEARCH\n\
old text 1\n\
=======\n\
new text 1\n\
>>>>>>> REPLACE\n\
<<<<<<< SEARCH\n\
old text 2\n\
=======\n\
new text 2\n\
>>>>>>> REPLACE\n\n\
A snippet whose old text is missing, or matches more than once, stops everything with NOT APPLIED and names the snippet; no file is touched.\n\
--show lists the removed (-) and added (+) lines of every snippet after writing, so the file need not be read again.\n\
Several files in one call: mysid apply (put the file name on the line before each <<<<<<< SEARCH).\n"
        ),
        "file" => Some(
            "mysid file: file operations, by what they do\n\n\
CREATE\n  \
  mysid file new <path> --stdin          new file, content on stdin (folders are made). If the file exists it stops with\n                                         \"already exists\": add --overwrite to replace it  [also: --from <file>, or the text as an argument]\n  \
  mysid write <path> --stdin             the same, and never replaces an existing file\n  \
  mysid append <path> --stdin            add text to the end of a file, on its own line  [--create makes the file when missing]\n  \
  mysid file mkdir <path>                a folder, with its parents\n\n\
MOVE / COPY\n  \
  mysid file mv <old> <new>              rename or move a file or folder         (also: rename, move)\n  \
  mysid file cp <src>... <dest>          copy files or folders, many at once; --overwrite replaces   (also: copy)\n\n\
REMOVE\n  \
  mysid file rm <path> [--recursive]     delete a file, or a folder with --recursive   (also: delete, remove)\n\n\
LOOK\n  \
  mysid file ls [path] [-L<depth>]       what is in a folder, same as mysid list   (also: list, dir)\n\n\
Examples:\n  \
  mysid file new config/settings.json --stdin\n  \
  mysid file mv old_name.kt new_name.kt\n  \
  mysid file cp ../other/src src/vendor --overwrite\n  \
  mysid file rm temp_cache --recursive\n\n\
Changing the inside of a file is not here: mysid patch, edit, replace, patch_batch (see mysid help).\n"
        ),
        "graph" => Some(
            "mysid graph: callers and callees of a symbol

Usage:
    mysid graph <symbol>                  Callers + symbol + callees (1 hop)
    mysid graph <symbol> --depth N        Callers and callees up to N hops (max 5)
    mysid graph <symbol> --flow           Same data drawn as UPSTREAM / TARGET / DOWNSTREAM trees
    mysid graph <symbol> --flow --depth 2 Trees, 2 hops
    mysid graph File.kt                   A file: what it defines, which files it calls into (JNI included), which call it
    mysid graph [dir]                     Map of the workspace or of a directory

<symbol> is a function, class, or Class.method (also file:Name when a name is ambiguous).

Examples:
    mysid graph useDarkMode
    mysid graph StreamResolver --flow --depth 2
    mysid graph src/Pages
"
        ),
        "build" => Some(
            "mysid build: Build the project (the stack is detected)\n\n\
Description:\n  \
  Detects the stack (Gradle/Android, Cargo, npm, Python, or a project run.bat) and builds it in a\n  \
  background worker. Prints each stage as it starts, then a short result: errors only when it fails,\n  \
  warnings grouped by message when it succeeds. The full output is always in .mysid/build.log.\n\n\
Usage:\n  \
  mysid build                Build; at a terminal waits for the result; piped (LLM) waits up to 20s, then reports progress and keeps running\n  \
  mysid build status         Progress, or the final report (waits up to 20s for a change)\n  \
  mysid build stop           Cancel the running build\n  \
  mysid build --fix-stale    Gradle: a moved project leaves caches (app/.cxx) naming the old folder, or local.properties names a missing SDK; the build stops with the cause. --fix-stale deletes the stale caches and builds, --ignore-stale builds anyway\n  \
  mysid build clean          Clean, then rebuild (Gradle)\n  \
  mysid build --wait 600     Wait up to 600s before returning a progress line (also for status)\n  \
  mysid build --release      Release profile (Cargo)\n\n\
Examples:\n  \
  mysid build\n  \
  mysid build status --wait 120\n"
        ),
        "release" | "aab" => Some(
            "mysid release: production release (Android bundle, or web deploy)\n\n\
Web project (package.json): runs the build script, then `netlify deploy --prod`. Needs netlify-cli, a login and\n\
`netlify link` once. Progress is a few timed lines; success prints RELEASE OK and the URL, failure only the errors\n\
and the failed step. Everything is in .mysid/release.log (follow live: Get-Content .mysid\\release.log -Wait).\n\n\
Android project:\n  \
  Raises versionCode by 1 (build.gradle.kts / build.gradle / manifest), then runs bundleRelease in the\n  \
  background with progress, and verifies app-release.aab and the R8 mapping.txt. If the build fails or is\n  \
  cancelled the versionCode is restored; if it succeeds the new number stays. A release already running\n  \
  is attached to, not bumped again. Each step is reported as a stage: preflight (JDK, signing env), version bump, Gradle startup, the build stages, then verifying (bundle zip, signature, R8 mapping). Uses the signing environment variables your build.gradle reads.\n\n\
Usage:\n  \
  mysid release                  Bump, build, and wait up to 20s for the result\n  \
  mysid build status             Progress, or the final report (waits up to 20s)\n  \
  mysid build stop               Cancel and restore versionCode\n  \
  mysid release --wait 600       Wait longer before returning a progress line\n\n\
Example output:\n  \
  RELEASE OK (Android, 4m10s) - versionCode 37 -> 38, 660 tasks (31 executed), 2 warning(s)\n  \
  AAB: app/build/outputs/bundle/release/app-release.aab (28.4 MB)\n  \
  R8 mapping: present\n"
        ),
        "install" => Some(
            "mysid install: put the built result where it is used\n\n\
Description:\n  \
  Android project: finds the latest built debug APK and deploys it to the device via ADB.\n  \
  Rust (Cargo) project: builds the release binary and installs it to %LOCALAPPDATA%\\mysid\\bin (~/.local/bin elsewhere).\n  \
  A running copy is moved aside to <name>.exe.old, so installing works while the MCP server is up.\n  \
  Node project: npm / yarn / pnpm install.\n\n\
Usage:\n  \
  mysid install\n\n\
Examples:\n  \
  mysid install\n"
        ),
        "debug" => Some(tools::build::debug::HELP),
        "dev" => Some(tools::dev::HELP),
        "test" => Some(tools::test_run::HELP),
        "status" => Some("mysid status: what changed, in git terms\n\nUsage:\n  mysid status     branch, upstream (ahead/behind), changed files (staged / unstaged / untracked), last commit\n  mysid branch     local branches with their last commit\n\nRead only. Secret-looking files are marked: mysid commit leaves them out.\n"),
        "branch" => Some("mysid branch: local branches with their last commit (read only). mysid status shows the changed files.\n"),
        "publish" => Some("mysid publish: the first upload of the project to GitHub (or any git remote)\n\nUsage:\n  mysid publish <repo-url>          PREVIEW: what would happen; nothing is changed or uploaded\n  mysid publish <repo-url> --yes    do it: create the repository and the first commit if there are none, add the remote\n                                    (origin), and git push -u origin <branch>\n  mysid publish owner/repo          shorthand for https://github.com/owner/repo.git\n  mysid publish <url> --name up     use a remote name other than origin\n\nCreate the repository on GitHub first (empty, no README). Sign-in is git's own (credential manager, or an SSH key); mysid\nnever sees a password or token. It never forces: a remote that already has other commits is left alone and the answer\nsays so. It refuses when secret-looking files (.env, keystores, .pem ...) are in the repository's history, and uncommitted\nchanges are not part of the push (mysid commit first). This sends the code to the internet: an agent must ask the\nowner before using --yes.\n"),
        "copy" | "cp" => Some("mysid copy: copy files or folders into the workspace (same as mysid file copy)\n\nUsage:\n  mysid copy <source> <destination>\n  mysid copy <source1> <source2> ... <destination-folder>\n  mysid copy ../other/src src/vendor --overwrite\n\nSources may be anywhere (a sibling project, a bare file name); the destination must be inside the workspace. Folders are\ncopied whole except .git, node_modules and .mysid. If any source is missing or any target exists, nothing is copied\n(--overwrite replaces).\n"),
        "deps" => Some(tools::deps::HELP),
        "doctor" => Some(tools::doctor::HELP),
        "clean" => Some("mysid clean: delete the node_modules of a Node / React project, so npm install can rebuild them\n\nUsage:\n  mysid clean [folder]          PREVIEW: every node_modules found, its size, and what owns it; nothing is deleted\n  mysid clean [folder] --yes    delete them and report the space freed and the install command to run\n\nOnly folders named exactly node_modules that have a package.json beside them are removed, only inside the folder you give\n(a folder of many projects works: up to 12 levels down); symbolic links are not followed. The listing is largest first, 40 lines\n(--max N for more). node_modules inside .netlify or .aws-sam are left alone, because the Netlify CLI and sam build recreate\nthem and npm install does not (--include-tools deletes those too). package.json and the lock file stay, so\nthe reinstall brings back the same versions (npm install, yarn install, pnpm install or bun install, whichever lock file exists).\nIf a folder cannot be removed (a dev server or editor holds files in it) the others are still deleted and it is reported.\nThis cannot be undone: an agent should show the preview to the owner before using --yes.\n"),
        "health" => Some("mysid health: where the space goes in a folder (read only)\n\nUsage:\n  mysid health [folder]              path, total size, the 5 largest folders and the 5 largest files\n  mysid health <folder> --top 10     more entries in both lists (up to 50)\n  mysid health <folder> --depth 2    let folders up to 2 levels deep compete (default 1: the direct sub-folders)\n\nSizes are real bytes on disk, node_modules, build output and .git included. Folders are labelled by name: [build output], [dependencies], [git history: keep],\n[mysid cache], [IDE settings]; a folder inside a src tree is never labelled.\nSymbolic links are not followed. Nothing is written or deleted.\n"),
        "tree" => Some(tools::scanner::orient::TREE_HELP),
        "new" => Some(
            "mysid new: create a new project\n\n\
Usage:\n  \
  mysid new react <name> [--dir <folder>] [--template react-ts|react|react-swc-ts|react-swc]\n\n\
React: runs the official Vite generator (npm create vite@latest <name> -- --template react-ts), then npm install.\n  \
  Needs Node.js and network access; it downloads and runs create-vite from npm.\n  \
  <name>: lower case letters, digits, - _ .  The project goes to <dir>/<name> (dir defaults to the current folder).\n  \
  It only creates in a missing or empty folder and never overwrites anything.\n\n\
Android: not available yet.\n"
        ),
        "skill" => Some(
            "mysid skill: workflow guides that teach an LLM how to use mysid\n\n\
Usage:\n  \
  mysid skill                     list the skills (base, android, react)\n  \
  mysid skill <name>              print one: steps plus small command examples\n  \
  mysid skill install [name]      write them as Claude Code skills into .claude/skills/ (base is always included)\n  \
  mysid skill install --global    write them to ~/.claude/skills/ instead\n"
        ),
        "apply" => Some(
            "mysid apply: apply edits from text that never goes through shell quoting

Usage:
    mysid apply <<'EOF'            (bash; in PowerShell use a single-quoted here-string @'...'@ piped in)
    <file name>
    <<<<<<< SEARCH
    <exact text to find>
    =======
    <replacement text>
    >>>>>>> REPLACE
    EOF
    mysid apply --file edit.txt    read the blocks from a file (write it with your file tool; no quoting at all)

Options: --dry-run (preview, write nothing), --replace_all (a block may match several places), --to <file> (blocks without a file name)

One call may hold many blocks for many files. Matching is as in `mysid patch` (exact, then line-ending tolerant, then
whitespace tolerant; refused on 0 or 2+ matches). Blocks for a file apply in order. All blocks apply or none is written;
a failure prints NOT APPLIED and names the block. An empty SEARCH on a missing file creates it. Bare file names work.
Quote the heredoc delimiter ('EOF'), or the shell will expand $ and backticks inside the blocks.
"
        ),
        "checkpoint" => Some(
            "mysid checkpoint: save the working tree so it can be reviewed and rolled back\n\n\
Usage:\n  \
  mysid checkpoint [\"label\"]   snapshot every file under the workspace (new files included; ignored files and\n                           secret-looking files left out). Branch, index and history are untouched.\n  \
  mysid checkpoint list      the saved checkpoints, newest first\n\n\
With no git repository it runs git init, git add and an initial commit (message = the label) first. Checkpoints live under refs/mysid/checkpoints and never appear in git log.\n"
        ),
        "check" => Some(
            "mysid check: fast type-check, errors only

Usage:
    mysid check [--workspace <path>]

Runs the TypeScript compiler (tsc --noEmit) in the workspace and prints only the errors, as File.ts:line: message.
Use it after each edit in a React/TypeScript project; use mysid build for a full production build.
"
        ),
        "list" => Some(
            "mysid list: what is in a folder

Usage:
    mysid list [folder]               sub-folders ranked by code (with their code-file counts), code files with line counts,
                                      other files collapsed; dot-folders and build output named once at the end
    mysid list [folder] --depth 1     every entry, plain (D folder / F file), also with -L<depth> or --full for the whole tree

It never shows file content: use mysid overview <file> for the outline, mysid read <file>#L10-60 for code, mysid graph <symbol>
for callers and callees. Respects .gitignore.
"
        ),
        "write" => Some(
            "mysid write: create a new file without shell quoting

Usage:
    mysid write <path> --stdin | --from <source>   content from stdin (heredoc / pipe) or a file

Refuses to replace an existing file (NOT WRITTEN, exit 1): use mysid apply or patch to change one, mysid file new to overwrite.
Parent folders are created.
"
        ),
        "edit" => Some(
            "mysid edit: change a file by line numbers

Usage:
  mysid edit <file>#L10-20 \"new text\"          replace lines 10 to 20 with the text   (file#L10 = one line)
  mysid edit <file>#L10-20 --file new.txt       the text from a file; or pipe it in (--stdin)
  mysid edit <file>#L10 --insert-after \"text\"   add lines after line 10   (--insert-before: above it)
  mysid edit <file>#L10-12 --delete             remove lines 10 to 12
  mysid edit <file>#L10-20 --file new.txt --expect \"fun load\"   refuse unless line 10 contains that text

Numbers are the ones mysid read shows (Foo.kt#L121-220). Use --expect when the file may have changed since you read it: the
edit then stops with NOT EDITED instead of changing the wrong lines. The answer lists the removed (-) and added (+) lines and
the new line numbers. All or nothing; CRLF files stay CRLF. Choose by what you know: the exact old text -> mysid patch; the line
numbers -> mysid edit; the same change in many places -> mysid replace.
"
        ),
        "append" => Some(
            "mysid append: add text to the end of a file without shell quoting

Usage:
    mysid append <path> --stdin | --from <source>

The file must exist. A line break is inserted first if the file does not end with one.
"
        ),
        "diff" => Some(
            "mysid diff: what changed since a checkpoint\n\n\
Usage:\n  \
  mysid diff            vs the latest checkpoint (or HEAD if there is none)\n  \
  mysid diff 2          vs checkpoint #2        mysid diff HEAD   /   mysid diff <any git revision>\n\n\
Prints a file list (A added, M modified, D deleted, with +/- line counts) and the first lines of each change.\n"
        ),
        "restore" => Some(
            "mysid restore: put the files back to a checkpoint\n\n\
Usage:\n  \
  mysid restore [target]          PREVIEW only: lists what would change (target: checkpoint number, last, HEAD, git revision)\n  \
  mysid restore [target] --yes    apply it\n\n\
Modified files get their old content back, deleted files are recreated, files added since are removed.\n\
A safety checkpoint is saved first (\"auto: before restore\"), so a restore can itself be undone. Ignored and\n\
secret-looking files are never touched.\n"
        ),
        "git" | "purge" => Some(tools::vcs::PURGE_HELP),
        "msvc" | "cl" => Some("mysid msvc: run a command with the Visual Studio C++ environment (vcvars64) set up\n\nUsage:\n  mysid msvc cl /nologo main.cpp          the compiler is on PATH\n  mysid msvc nmake /f Makefile.win        also link, lib, msbuild, ...\n  mysid msvc --errors \"nmake\"             only the error lines (error, fatal, LNK1xxx, [exit N])\n\nFinds Visual Studio with vswhere (the Desktop C++ workload), then runs `call vcvars64.bat && <command>` in cmd.exe.\nThe same flags as exec: --timeout N, --out file, --workspace_root.\n"),
        "emsdk" | "wasm" => Some("mysid emsdk: run a command with the Emscripten SDK environment set up\n\nUsage:\n  mysid emsdk emcc main.cpp -O2 -o out.js        emcc / em++ / emmake / emcmake on PATH\n  mysid wasm emmake make                         (wasm is the same command)\n  mysid emsdk --errors \"emmake make\"             only the error lines\n\nFinds the SDK from EMSDK, then %USERPROFILE%\\emsdk, C:\\emsdk, D:\\emsdk, and runs `call emsdk_env.bat && <command>`\n(`. emsdk_env.sh` elsewhere). The same flags as exec: --timeout N, --out file, --workspace_root.\n"),
        "bootstrap" | "init" => Some(
            "mysid bootstrap: initialize a local git repository, and nothing else\n\n\
Usage:\n  \
  mysid bootstrap                   git init (branch main) unless a repository is here; nothing is staged or committed\n  \
  mysid bootstrap --branch trunk    name the initial branch\n  \
  mysid bootstrap --gitignore       also write a .gitignore for this stack (never overwrites one)\n\n\
Already a repository: it says so and changes nothing. Inside a parent repository: it says which, and creates nothing.\n\
Never in a drive root or the home folder. Build output and secret-looking files are hidden locally through\n\
.git/info/exclude. .gitignore only affects untracked files: a file that is already committed stays tracked until\n\
`git rm --cached <file>`. Then: write or review .gitignore, and mysid commit --branch main --message \"Initial commit\".\n"
        ),
        "commit" => Some(
            "mysid commit: record the changes with git, locally\n\n\
Usage:\n  \
  mysid commit --message \"Add authentication\"        commit on the current branch\n  \
  mysid commit \"Add authentication\"                  the same, message as a plain argument\n  \
  mysid commit --branch main --message \"Initial commit\"   make sure the work lands on branch main (see below)\n  \
  mysid commit --message \"Fix validation\" --paths a.kt b.kt   commit only these paths\n\n\
What it does:\n  \
  - stages every change under the workspace (or only --paths) and commits it locally; the answer lists the files\n    \
    that went in, so nothing unrelated slips by unseen\n  \
  - --branch: no commits yet -> the initial commit is made on that branch; already on it -> nothing; it exists ->\n    \
    switch to it (git refuses when your changes would be overwritten); it does not exist -> create it from here\n  \
  - no repository yet: runs git init first (see mysid bootstrap for init alone)\n  \
  - leaves out files that look like secrets (*.jks, *.keystore, *.pem, .env, keystore.properties,\n    \
    local.properties, ...) and says which\n  \
  - notes tracked files that a .gitignore rule matches: they stay tracked until git rm --cached\n  \
  - never pushes (mysid publish does, with --yes), and adds no attribution lines to the message\n  \
  - prints: <hash> <message> - N files, +added -removed [branch]\n"
        ),
        "history" => Some(
            "mysid history: the last commits, compactly\n\n\
Usage:\n  \
  mysid history              last 5 commits\n  \
  mysid history --past 10    last 10 (also: history 10, history -10); at most 100\n\n\
Each line: <hash>  <age>  <subject>  (N files, +added -removed)\n"
        ),
        "android" | "adb" => Some(
            "mysid android: the Android / device toolkit\n\n\
Start here:\n  \
  1. mysid devices                              list the connected devices\n  \
  2. mysid wifi [--ip <ip>] [--port <port>]     connect a device over Wi-Fi\n  \
  3. mysid install                              put the built app on the device (Android: debug APK; Rust: the built exe)\n  \
  4. mysid debug \"usb, router\" [--seconds 40]   record only the device-log (logcat) lines you care about, then read them back; mysid help debug\n  \
  5. mysid android goto \"USB Drive\" \"Durandhar\"   TV: D-pad to each label, then OK  [--no-ok] [--nth 2]\n  \
  6. mysid android scenario steps.json | mysid android focus   scripted run with checks (pass/fail, cold start, jank, crashes) | the focused element\n  \
  7. mysid android keyevent --keycode <back|home|dpad_right|KEYCODE_..> [--times 20 --fast]   send keys\n  \
  8. mysid release                              Android: raise versionCode and build a local release AAB (web project: Netlify deploy); mysid help release\n\n\
Also in the toolkit (details below): launch_app, logcat, crashes, clear_logcat, tap, swipe, longpress, type, scroll, screenshot, ui_dump, batch.\n\n\
Description:\n  \
  Device controls, logs, Wi-Fi connectivity, and UI input simulation.\n\n\
Usage:\n  \
  mysid android devices                       List connected ADB devices\n  \
  mysid android connect_wifi [--ip <ip>]      Pair/connect device over Wi-Fi\n  \
  mysid android install                       Deploy built debug APK to device\n  \
  mysid android launch_app                    Launch main application activity\n  \
  mysid android logcat [--lines <n>]          Dump recent logcat entries\n  \
  mysid android crashes                       Filter logcat for fatal exceptions & crashes\n  \
  mysid android clear_logcat                  Flush ADB logcat buffer\n  \
  mysid android tap --x <x> --y <y>           Tap, then return a screenshot  (also: tap 540 1200)\n  \
  mysid android tap --text \"Apply\"            Tap the element with that text (--desc \"Open menu\", --id <resource-id>, --index <n>)\n  \
  mysid android swipe --x1 --y1 --x2 --y2     Swipe [--duration ms]\n  \
  mysid android longpress --x <x> --y <y>     Press and hold [--duration ms]\n  \
  mysid android type --text <text>            Type text (newline = Enter), then screenshot\n  \
  mysid android keyevent --keycode <code|key> Send a key (back, home, enter, KEYCODE_..., 66) [--times n], then screenshot\n  \
  mysid android keyevent dpad_right --times 20 --fast   Many presses in ONE adb call (also --keys \"dpad_right dpad_right enter\", --gap ms)\n  \
  mysid android scroll down | up | left | right   Scroll [to \"text\"] (scrolls until the text is on screen)\n  \
  mysid android screenshot                    Just the screen\n  \
  mysid android scenario steps.json           Scripted run with checks: pass/fail per step, cold start, janky frames, crashes (below)\n  \
  mysid android focus [--list]                Text only: the focused element (label, class, bounds, path); --list adds every focusable one\n  \
  mysid android goto \"USB Drive\" \"Durandhar\"  TV / D-pad: press the D-pad until each label has focus, then OK [--no-ok] [--max 30] [--nth 2]\n  \
  mysid android ui_dump                       Visible elements: [i] text | desc | id | clickable | center(x,y)\n  \
  mysid android batch --json steps.json       Many steps, one call: [{\"action\":\"tap\",\"x\":540,\"y\":1200,\"shot\":true},{\"action\":\"wait\",\"ms\":500}]\n\n\
Every action waits --wait ms (default 600), captures the screen and prints:\n  \
  tap(540,1200) ok | device dee50561 | screen 1080x2412 | image 1000x2243 | scale 1.080\n\
CLI saves the PNG to .mysid/shots/<ms>.png (last 20 kept); over MCP the image comes back as image content.\n\
Coordinates are DEVICE pixels. The image is smaller: device = image x scale. --space image takes coordinates read off the\n\
image instead and converts them. --grid overlays a labelled 100 px grid and a red crosshair where the action landed.\n\
Every result also says `screen changed` or `screen UNCHANGED` (so you know the action did something; --no-diff skips the\n\
extra capture), the foreground app, and the workspace. `--app <package>` refuses to act when another app is in front.\n\
`--expect \"text\"` waits for that text to appear (--timeout ms, default 5000) instead of a fixed wait. In a batch, a step stops\n\
the run when its expectation fails, when it errors, or with \"require_change\": true when the screen did not change.\n\
Options: --device <serial> (when several), --scale <0.1-1>, --wait <ms>, --grid, --space image. Every adb call times out after 10 s.\n\
TV / no touch: a key press waits until the screen stops changing (two equal captures; --settle-timeout ms, default 2000)\n\
instead of a fixed 600 ms, unless --wait is given. `keyevent dpad_right --no-shot` answers with text only: the result and the\n\
focus line (`focus: \"Play all\" [Button] center (x,y) bounds ... path: A > B`); `--focus` adds that line to a screenshot answer.\n\
`goto` reads the accessibility tree before every press, picks the key towards the target, and stops with a reason when focus\n\
does not move, jumps the wrong way, loops, or the target never appears (then with a screenshot). Also usable as a batch step.\n\
scenario file: {\"app\":\"com.x.tv\",\"steps\":[\"launch --cold\",\"goto 'USB Drive'\",\"expect text 'Play all'\",\"expect focus 'Play all'\"]}\n\
  steps: launch [--cold] [pkg] | key <k> [--times n] | goto <labels> | tap, swipe, type, scroll, wait ... | expect text|focus|gone|app <v> [--timeout ms]\n\
  Report: PASS/FAIL per step (a screenshot of the first failure only), cold start (am start -W), janky frames (gfxinfo), and every\n\
  crash / ANR / exception of the app under the step that was running; a crash fails its step. --keep-going continues after a failure;\n\
  \"max_cold_start_ms\" / \"max_janky_percent\" in the file make those numbers pass/fail. Exit status is non-zero on FAIL.\n\
Actions are logged to .mysid/touch.log.\n"
        ),
        "devices" => Some(
            "mysid devices: List connected ADB devices\n\n\
Usage:\n  \
  mysid devices\n"
        ),
        "wifi" | "connect_wifi" => Some(
            "mysid wifi: Connect to Android device over Wi-Fi\n\n\
Usage:\n  \
  mysid wifi [--ip <ip_address>] [--port <port>]\n"
        ),
        "set_workspace" | "set" => Some(
            "mysid set: choose the project for commands run from this folder\n\n\
Description:\n  \
  Optional: mysid already finds the project from the folder you run it in. `set` pins a project (a path or a\n  \
  registered name) for commands run from THIS folder and the ones below it, by THIS session (the agent or terminal\n  \
  that ran it: another agent sharing the folder keeps its own choice, and yours survives the shell being reset).\n  \
  Prints the project's top-level entries. (`set_workspace` is the same command.)\n\n\
Usage:\n  \
  mysid set <path | name>\n\n\
Examples:\n  \
  mysid set E:\\Hypersonus\\v44\n"
        ),
        "get_workspace" => Some(
            "mysid get_workspace: Get currently active workspace path\n\n\
Usage:\n  \
  mysid get_workspace\n"
        ),
        "exec" | "run" => Some(
            "mysid exec: Run a shell command in the workspace\n\n\
Usage:\n  \
  mysid exec <command...>              Everything after `exec` is the command; -A, -m, | pass through\n  \
  mysid exec \"<command>\"               A single quoted string works too (pipes, redirects)\n  \
  mysid exec --timeout 120 <command>   Seconds before the command is killed (default 30)\n  \
  mysid exec --out run.txt <command>   Also save everything the command printed to a file, even if it was killed\n  \
  mysid exec --tail 25 --errors -- <command>   mysid flags before the command; `--` ends them (--head/--tail/--grep/--grep-v/--errors filter the output)\n  \
  mysid exec \"<command>\" --timeout 120 --tail 30   after ONE quoted command string the flags are mysid's too\n  \
  mysid exec --env msvc|emsdk ...      set up Visual Studio C++ / Emscripten first (the same as mysid msvc ... / mysid emsdk ...)\n\n\
Notes:\n  \
  Runs through cmd.exe on Windows (sh elsewhere): && | > work, ; does not, so a command with ; is run in PowerShell\n  \
  (the answer says so). Choose with --shell cmd|powershell|bash (bash = Git Bash). --out must be inside the workspace.\n  \
  Exit code 1 with no output usually means \"no match\"\n  \
  (grep/findstr), not a failure of mysid. Prefer `mysid search` and `mysid read` for code.\n\n\
Examples:\n  \
  mysid exec git status\n  \
  mysid exec \"adb devices\"\n"
        ),
        "overview" | "signatures" => Some(
            "mysid overview: Extract method signatures and parameter outlines across languages\n\n\
Description:\n  \
  High-speed signature extractor for Java, Kotlin, C/C++, Rust, Python, TypeScript/JS, Go, Swift, and C#.\n  \
  Returns clean outlines of classes, methods, and full parameter lists without wasting context.\n\n\
Usage:\n  \
  mysid overview <file>                Extract method signatures from a single file\n  \
  mysid overview <file1> <file2> ...   Extract signatures from multiple files in one turn\n  \
  mysid overview <directory>           Extract signatures from all source files in a folder\n  \
  mysid overview <directory> --depth 1 Only the folder names and their code-file counts (choose a folder first)\n  \
  mysid overview <directory> --match word   Only files whose path, class or function contains the word\n\n\
Options:\n  \
  --format json                        Return structured JSON instead of human-readable outline\n\n\
Examples:\n  \
  mysid overview MusicService.kt\n  \
  mysid overview FlacDecoder.cpp VolumeManager.kt\n  \
  mysid overview app/src/main/java/com/hypersonus/player/volume\n"
        ),
        _ => None,
    }
}
