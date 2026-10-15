//! The tools the MCP server advertises: names, descriptions and input schemas.

use serde_json::{json, Value};

pub(crate) fn get_mcp_tools_list() -> Value {
    json!([
        {
            "name": "read",
            "description": "The CODE of a file: a short file whole, a longer source file returns its outline with line numbers and the exact command for a slice (so a bare read never floods), a slice (Player.kt#L10-40), several slices in one call, or a symbol definition with callers. For the full outline use overview; not for folders (use list). Use just the file name (Player.kt), never the full path: mysid finds it anywhere in the workspace, and output prints short names too. Give a folder (ui/Player.kt) only when two files share a name. Bundle all file slices into a single call.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "Symbol name (e.g. 'StreamResolver') or file slices (e.g. 'Player.kt#L10-40' or 'AudioEngine.cpp#L100-140')"
                    },
                    "files": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Optional list of file slice specs to inspect together (e.g. ['FileA.kt#L1-30', 'FileB.cpp#L40-80'])"
                    }
                }
            }
        },
        {
            "name": "overview",
            "description": "Extract all method signatures, parameters, return types, and class structures across Java, Kotlin, C/C++, Rust, Python, TypeScript, Go, Swift, and C#.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "File name such as 'MusicService.kt' (no directories needed), or a directory" },
                    "files": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Optional list of files to inspect simultaneously"
                    },
                    "format": { "type": "string", "description": "Output format: 'text' (default human-readable outline) or 'json'" },
                    "depth": { "type": "integer", "description": "For a directory: folder names and code-file counts only, this many levels deep (no outlines)" },
                    "match": { "type": "string", "description": "Keep only the files whose path, or a class / function in them, contains this word" }
                }
            }
        },
        {
            "name": "tree",
            "description": "The folder map of the workspace: folders with their number of code files, depth 2 by default. Build output and node_modules are left out. Use it to pick the folder for overview.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Folder to map (default: the workspace)" },
                    "depth": { "type": "integer", "description": "Levels (default 2, max 4)" },
                    "all": { "type": "boolean", "description": "Also dot-folders" }
                }
            }
        },
        {
            "name": "search",
            "description": "High-speed multi-threaded regex/string search respecting .gitignore. Results list short file names (Foo.kt:12); pass those names straight to read/patch. md, txt and rst files are left out unless all=true or types includes them; a no-match answer says so. grep-style \\| is alternation; a very long line is shown as a window around the match.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "Search pattern or regex" },
                    "types": { "type": "string", "description": "Optional comma-separated file extensions (e.g. 'kt,cpp,rs')" },
                    "max_results": { "type": "integer", "description": "Maximum match entries to return (default 15, up to 500)" },
                    "offset": { "type": "integer", "description": "Skip the first n matches (next page)" },
                    "count": { "type": "boolean", "description": "Per-file summary: matching lines per file, most first, no hit text" },
                    "context": { "type": "integer", "description": "Context lines around each match (default: 1)" },
                    "group": { "type": "boolean", "description": "File name once, then 'line: text' under it (fewer tokens)" },
                    "all": { "type": "boolean", "description": "Also search md, txt and rst files" }
                },
                "required": ["query"]
            }
        },
        {
            "name": "replace",
            "description": "Fast global workspace search-and-replace refactoring or targeted single-file replacement.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "old": { "type": "string", "description": "Exact text to search and replace" },
                    "new": { "type": "string", "description": "Replacement text" },
                    "path": { "type": "string", "description": "Optional file name (Foo.kt) or subdirectory to restrict replacement" },
                    "types": { "type": "string", "description": "Optional comma-separated file extensions (e.g. 'kt,cpp')" },
                    "dry_run": { "type": "boolean", "description": "If true, previews matches and diffs without modifying files on disk" }
                },
                "required": ["old", "new"]
            }
        },
        {
            "name": "patch",
            "description": "Surgical search-and-replace patching in a single target file.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "file": { "type": "string", "description": "File name only, e.g. Foo.kt (no directories needed; add a folder only if two files share the name)" },
                    "old": { "type": "string", "description": "Exact original code chunk to replace" },
                    "new": { "type": "string", "description": "New replacement code chunk" },
                    "show": { "type": "boolean", "description": "Also return the removed (-) and added (+) lines, so the file need not be read again" }
                },
                "required": ["file", "old", "new"]
            }
        },
        {
            "name": "patch_batch",
            "description": "Several replacements in one call. For ONE file: path + text, where text holds SEARCH/REPLACE snippets ('<<<<<<< SEARCH', the exact old text, '=======', the new text, '>>>>>>> REPLACE', repeated); all applied or none, show=true lists what changed. For several files: patches (applied in order; stops at the first failure, earlier ones stay).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "The one file the snippets in text are for" },
                    "text": { "type": "string", "description": "SEARCH/REPLACE snippets for that file" },
                    "show": { "type": "boolean", "description": "Return the removed (-) and added (+) lines" },
                    "patches": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "file": { "type": "string" },
                                "old": { "type": "string" },
                                "new": { "type": "string" }
                            },
                            "required": ["file", "old", "new"]
                        },
                        "description": "List of patch operations across files, applied in order"
                    }
                }
            }
        },
        {
            "name": "mkdir",
            "description": "Create directory recursively, automatically creating any missing parent directories.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Directory path to create" }
                },
                "required": ["path"]
            }
        },
        {
            "name": "file",
            "description": "File system utilities: new file, rename, delete, or list.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "action": { "type": "string", "enum": ["new", "rename", "delete", "list"], "description": "Action to perform" },
                    "path": { "type": "string", "description": "File name (Foo.kt, resolved anywhere in the workspace) or a directory path" },
                    "content": { "type": "string", "description": "Content for new file" },
                    "new_path": { "type": "string", "description": "Destination path for rename" }
                },
                "required": ["action"]
            }
        },
        {
            "name": "graph",
            "description": "Callers and callees of a symbol (1 hop by default), including Kotlin/Java <-> C/C++ JNI bridges (a native method links to its Java_... function, native code calling back by method name links to the Kotlin method), calls through typed receivers and chained getters, thread entry points (&Class::loop), constructors, and base-class methods to their overrides. A FILE NAME (UsbDirectOutput.kt) gives the file card: what it defines, which files it calls into, which call it. With no symbol, a compact map of the workspace; a directory drills into the map.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "symbol": { "type": "string", "description": "Function, class, Class.method, a file name (File.kt), or a directory" },
                    "depth": { "type": "integer", "description": "Hops of callers and callees to include (default 1, max 5)" },
                    "flow": { "type": "boolean", "description": "Draw UPSTREAM / TARGET / DOWNSTREAM trees instead of the compact card" }
                }
            }
        },
        {
            "name": "build",
            "description": "Build the project (Gradle, Cargo, npm, Python). Runs in a detached worker: waits up to 20s, then returns a progress line with the current stage; call again with action 'status' to check, 'stop' to cancel. Full output is live in .mysid/build.log. Failures show only the errors.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "action": { "type": "string", "enum": ["build", "clean", "status", "stop", "install"], "description": "Build action to perform (default build)" },
                    "wait": { "type": "integer", "description": "Seconds to wait before returning a progress line (default 20)" }
                }
            }
        },
        {
            "name": "commit",
            "description": "Commit the workspace's changes locally with git (message required). Creates the repository first if none exists (never in a drive root or home folder), skips secret-looking files, never pushes, adds no attribution. Returns '<hash> <message> - N files, +added -removed [branch]'.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "message": { "type": "string", "description": "Commit message" },
                    "branch": { "type": "string", "description": "Make the commit land on this branch: no commits yet -> the initial commit is made on it; exists -> switch to it when safe; missing -> create it from the current commit" },
                    "paths": { "type": "array", "items": { "type": "string" }, "description": "Commit only these paths (default: every change under the workspace)" }
                },
                "required": ["message"]
            }
        },
        {
            "name": "history",
            "description": "Last commits of the workspace repository, one compact line each: hash, age, subject, files and lines changed.",
            "inputSchema": { "type": "object", "properties": { "past": { "type": "integer", "description": "How many commits (default 5, max 100)" } } }
        },
        {
            "name": "new",
            "description": "Create a new project. query 'react my-app' runs the official Vite generator (react-ts) in <dir>/my-app and npm installs it; needs Node and network (downloads create-vite from npm). Only creates in a missing or empty folder. Android is not available yet.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "'<stack> <name>', e.g. 'react my-app'" },
                    "dir": { "type": "string", "description": "Folder to create the project in (default: the current folder; pass it explicitly when calling over MCP)" },
                    "template": { "type": "string", "description": "React template: react-ts (default), react, react-swc-ts, react-swc" }
                },
                "required": ["query"]
            }
        },
        {
            "name": "list",
            "description": "Every entry of a folder (folders D, files F, with counts). For where the code lives use graph; for a file's outline use overview; for code use read.",
            "inputSchema": { "type": "object", "properties": { "path": { "type": "string", "description": "Folder, relative to the workspace (default: the workspace root)" } } }
        },
        {
            "name": "write",
            "description": "Create a file with the given content (parent folders are created). Refuses to replace an existing file unless overwrite=true; replacing a CRLF file keeps CRLF. For changing part of an existing file use patch or apply.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "File to create, relative to the workspace (or absolute)" },
                    "text": { "type": "string", "description": "The file content" },
                    "overwrite": { "type": "boolean", "description": "Allow replacing an existing file" }
                },
                "required": ["path", "text"]
            }
        },
        {
            "name": "edit",
            "description": "Edit a file by line numbers (the ones read shows): replace lines first-last with new text, insert after/before a line, or delete lines. Give path as Foo.kt#L10-20 (or path plus start/end). expect= refuses unless the first line named contains that text, so numbers that moved cannot damage the file. Keeps CRLF; answers with the removed and added lines.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "File, optionally with the range: Foo.kt#L10-20 or Foo.kt#L10" },
                    "start": { "type": "integer", "description": "First line (when the path has no #L range)" },
                    "end": { "type": "integer", "description": "Last line (default: start)" },
                    "text": { "type": "string", "description": "The new text (not needed for delete)" },
                    "mode": { "type": "string", "enum": ["replace", "insert_after", "insert_before", "delete"], "description": "default replace" },
                    "expect": { "type": "string", "description": "Text the first named line must contain, else nothing changes" }
                },
                "required": ["path"]
            }
        },
        {
            "name": "append",
            "description": "Add text to the end of an existing file (found by file name), on its own line, keeping the file's line endings. create=true makes the file if it is missing.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "File name, e.g. Notes.md" },
                    "text": { "type": "string", "description": "Text to add" },
                    "create": { "type": "boolean", "description": "Create the file when it does not exist" }
                },
                "required": ["path", "text"]
            }
        },
        {
            "name": "apply",
            "description": "Apply several edits in one call from SEARCH/REPLACE blocks (file name on the line before '<<<<<<< SEARCH', then the exact text, '=======', the replacement, '>>>>>>> REPLACE'). Same safe matching as patch (refuses on 0 or 2+ matches, keeps CRLF), many blocks and files per call, all applied or none (a failure prints 'NOT APPLIED' and names the block). An empty SEARCH on a missing file creates it. Preferred for multi-line code, quotes, $ and backslashes.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "text": { "type": "string", "description": "The blocks" },
                    "dry_run": { "type": "boolean", "description": "Preview, write nothing" },
                    "show": { "type": "boolean", "description": "After applying, list the removed (-) and added (+) lines of each block" },
                    "replace_all": { "type": "boolean", "description": "Let a block match several places" },
                    "to": { "type": "string", "description": "File for blocks that have no file name line" }
                },
                "required": ["text"]
            }
        },
        {
            "name": "checkpoint",
            "description": "Save a snapshot of the working tree (new files included; ignored and secret-looking files left out) under refs/mysid, without touching the branch, index or history. Use before a batch of edits. query 'list' shows saved checkpoints. Creates the git repository if there is none.",
            "inputSchema": { "type": "object", "properties": { "query": { "type": "string", "description": "Label for the checkpoint, or 'list'" } } }
        },
        {
            "name": "diff",
            "description": "Compact review of what changed since a checkpoint (default: the latest, else HEAD): files with A/M/D and +/- counts plus the first lines of each change. Run it after edits and a build, before committing.",
            "inputSchema": { "type": "object", "properties": { "query": { "type": "string", "description": "Checkpoint number, 'HEAD', or any git revision (default: latest checkpoint)" } } }
        },
        {
            "name": "restore",
            "description": "Roll files back to a checkpoint. Without yes=true it only previews the plan and changes nothing. With yes=true it first saves a safety checkpoint ('auto: before restore'), restores modified files, recreates deleted ones and removes files added since. Never touches ignored or secret-looking files.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "Checkpoint number, 'last', 'HEAD' or a git revision (default: latest checkpoint)" },
                    "yes": { "type": "boolean", "description": "Apply the restore (otherwise preview only)" }
                }
            }
        },
        {
            "name": "skill",
            "description": "Workflow guides for using mysid well. query '' lists them; 'base', 'android' or 'react' returns the steps and small command examples; 'install' writes them as Claude Code skills into .claude/skills. Read 'base' first in a new workspace.",
            "inputSchema": { "type": "object", "properties": { "query": { "type": "string", "description": "Skill name (base, android, react), 'install [name]', or empty to list" } } }
        },
        {
            "name": "release",
            "description": "Android release: bumps versionCode, builds the release bundle (bundleRelease) with stage-by-stage progress, then verifies the bundle signature and R8 mapping. Same wait/status/stop behaviour as build.",
            "inputSchema": { "type": "object", "properties": { "wait": { "type": "integer", "description": "Seconds to wait before returning a progress line (default 20)" } } }
        },
        {
            "name": "debug",
            "description": "Android device log recorder. query 'Buffer, router, usb' clears the device logcat, records only lines whose tag or text contains a keyword into .mysid/debug.log for a fixed window (seconds, default 20, 10-60) while the user reproduces the issue, then returns '20 sec log record done' with the lines (repeats collapsed). Call with no query to read the last recording again. Also 'status', 'stop', 'clear'.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "Comma-separated keywords, or status/stop/clear. Omit to show the recording." },
                    "new": { "type": "boolean", "description": "Only lines recorded since the previous debug call" },
                    "raw": { "type": "boolean", "description": "Raw mode: clear logcat, capture the WHOLE device log (all buffers, system dump) for `seconds`, keep lines containing the query terms with 3 lines above and below, save to .mysid/debug.log. Use for system/framework errors not logged by the app's own tags." },
                    "seconds": { "type": "integer", "description": "Recording window in seconds, 10-60 (default 20). The call waits for the window, then reports 'N sec log record done'." },
                    "level": { "type": "string", "description": "Minimum level to show: V D I W E" },
                    "lines": { "type": "integer", "description": "Maximum lines to show (default 60)" }
                }
            }
        },
        {
            "name": "test",
            "description": "Run the project tests (Cargo, npm/pnpm/yarn test, Gradle, pytest; detected). One line when everything passes (TESTS OK, 104 passed); on failure only the failing tests with their messages and locations. Full output is in .mysid/test.log. Optional query filters by test name.",
            "inputSchema": { "type": "object", "properties": { "query": { "type": "string", "description": "Only tests matching this name" }, "timeout": { "type": "integer", "description": "Seconds before the run is killed (default 600)" } } }
        },
        {
            "name": "git",
            "description": "Git-level operations. action=purge removes git from the project as if it had never been used: .git/ (commits, branches, tags, stashes, index, config, remote URLs, mysid checkpoints); .gitignore / .gitattributes / .gitmodules / extras / nested repositories only with their own flag. Source and every other project file are never touched. WITHOUT yes it only previews (and warns about commits that exist on no remote). IRREVERSIBLE: NEVER set yes=true unless the user explicitly told you to purge.",
            "inputSchema": { "type": "object", "properties": {
                "action": { "type": "string", "enum": ["purge"] },
                "yes": { "type": "boolean", "description": "Actually remove. Only after the user said so." },
                "gitignore": { "type": "boolean", "description": "Also remove .gitignore" },
                "gitattributes": { "type": "boolean", "description": "Also remove .gitattributes" },
                "gitmodules": { "type": "boolean", "description": "Also remove .gitmodules" },
                "extras": { "type": "boolean", "description": "Also remove .git-blame-ignore-revs and .gitkeep files" },
                "nested": { "type": "boolean", "description": "Also remove repositories inside subfolders" },
                "all": { "type": "boolean", "description": "Every optional item above" }
            }, "required": ["action"] }
        },
        {
            "name": "bootstrap",
            "description": "Initialize a local git repository and nothing else: no staging, no commit, no upload. If a repository already exists it says so and changes nothing; inside a parent repository it reports that. Optional gitignore=true writes a .gitignore for the detected stack (never overwrites). Next: review .gitignore, then commit with branch and message.",
            "inputSchema": { "type": "object", "properties": { "branch": { "type": "string", "description": "Initial branch name (default main)" }, "gitignore": { "type": "boolean", "description": "Also write a .gitignore for this stack if none exists" } } }
        },
        {
            "name": "publish",
            "description": "First upload of the project to a git remote (GitHub). WITHOUT yes it only previews (nothing changes). With yes=true it creates the repository and first commit if needed, adds the remote and pushes the current branch (never forces; refuses when secret-looking files are in the history). Uploading code is outward-facing: NEVER set yes=true unless the user has explicitly told you to publish to that repository.",
            "inputSchema": { "type": "object", "properties": { "url": { "type": "string", "description": "https://github.com/owner/repo.git, git@github.com:owner/repo.git, or owner/repo" }, "yes": { "type": "boolean", "description": "Actually push. Only after the user said so." }, "name": { "type": "string", "description": "Remote name (default origin)" } }, "required": ["url"] }
        },
        {
            "name": "status",
            "description": "Git status in one call: branch, upstream (ahead/behind), changed files (staged, unstaged, untracked), last commit. Read only. Secret-looking files are marked: mysid commit leaves them out.",
            "inputSchema": { "type": "object", "properties": {} }
        },
        {
            "name": "dev",
            "description": "Run the project's dev server (npm/pnpm/yarn run dev or start) in the background. No query: start it and wait until it prints a local URL that accepts connections, then return the URL (open it in a browser to check the app renders). Also 'log' (server errors, else last lines; lines=N for a raw tail), 'status', 'stop'. Output is in .mysid/dev.log.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "start (default), log, status or stop" },
                    "port": { "type": "integer", "description": "Port to pass to the server (--port)" },
                    "lines": { "type": "integer", "description": "log: raw tail of this many lines" },
                    "wait": { "type": "integer", "description": "start: seconds to wait for the URL (default 30)" }
                }
            }
        },
        {
            "name": "doctor",
            "description": "Project checks. sub=clean with path deletes the node_modules of a Node/React project ONLY when yes=true (otherwise it previews what would go and the install command). sub=health with path: total size, the largest folders and the largest files (top, depth). sub=compare with a (old folder) and b (new folder): totals (files, folders, size), how many files are identical, changed (lines, which side is newer), differ only in CRLF/LF, or exist in one folder only, a table per top-level folder, and a verdict on whether the old folder can be deleted. Generated folders (node_modules, build, .git ...) are left out unless everything=true.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "sub": { "type": "string", "enum": ["compare", "health", "clean"], "description": "the check to run" },
                    "path": { "type": "string", "description": "health: the folder to measure (default: the workspace)" },
                    "top": { "type": "integer", "description": "health: entries in each list (default 5, up to 50)" },
                    "yes": { "type": "boolean", "description": "clean: really delete the node_modules folders (without it, only a preview). Ask the owner first." },
                    "include_tools": { "type": "boolean", "description": "clean: also delete node_modules inside .netlify / .aws-sam (rebuilt by their tools, not by npm install)" },
                    "depth": { "type": "integer", "description": "health: how many levels of sub-folders compete (default 1)" },
                    "a": { "type": "string", "description": "compare: the old folder" },
                    "b": { "type": "string", "description": "compare: the new folder" },
                    "max": { "type": "integer", "description": "entries listed per group (default 25)" },
                    "brief": { "type": "boolean", "description": "totals, verdict and per-folder table only" },
                    "everything": { "type": "boolean", "description": "also compare generated folders" }
                },
                "required": ["sub"]
            }
        },
        {
            "name": "deps",
            "description": "npm packages the source imports that package.json does not list (with the files that need them and the install command), and listed dependencies nothing imports. Relative imports, Node built-ins and tsconfig path aliases are ignored.",
            "inputSchema": { "type": "object", "properties": {} }
        },
        {
            "name": "android",
            "description": "Android ADB device bridge and device control. tap/swipe/longpress/type/keyevent/screenshot/batch act on the device and ALWAYS return a screenshot (image) plus one line mapping image pixels to device pixels (x device = x image * scale). tap accepts text/desc/id to find an element itself; ui_dump lists visible elements.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["devices", "connect_wifi", "install", "launch_app", "logcat", "crashes", "clear_logcat", "tap", "swipe", "longpress", "type", "keyevent", "screenshot", "ui_dump", "focus", "goto", "scenario", "batch"],
                        "description": "Android device command"
                    },
                    "package_name": { "type": "string", "description": "Application package ID (optional)" },
                    "lines": { "type": "integer", "description": "Logcat line limit (default: 50)" },
                    "x": { "type": "integer", "description": "X for tap/longpress, in device pixels (or image pixels with space=image)" },
                    "y": { "type": "integer", "description": "Y for tap/longpress" },
                    "x1": { "type": "integer", "description": "swipe start X" },
                    "y1": { "type": "integer", "description": "swipe start Y" },
                    "x2": { "type": "integer", "description": "swipe end X" },
                    "y2": { "type": "integer", "description": "swipe end Y" },
                    "text": { "type": "string", "description": "type: the text to type. tap/longpress: tap the element with this text instead of coordinates" },
                    "desc": { "type": "string", "description": "tap/longpress: element content-description (Compose icons)" },
                    "id": { "type": "string", "description": "tap/longpress: element resource-id" },
                    "index": { "type": "integer", "description": "which match to use when text/desc/id matches several elements" },
                    "key": { "type": "string", "description": "Key code or name for keyevent (BACK, HOME, ENTER, KEYCODE_..., 66)" },
                    "duration": { "type": "integer", "description": "swipe (default 300) / longpress (default 800) duration in ms" },
                    "wait": { "type": "integer", "description": "ms to wait after the action before the screenshot (default 600)" },
                    "scale": { "type": "number", "description": "returned image size as a fraction of the screen, 0.1-1 (default: longest side 2000 px)" },
                    "space": { "type": "string", "enum": ["device", "image"], "description": "coordinate space of x/y: device pixels (default) or the pixels of the returned image" },
                    "grid": { "type": "boolean", "description": "overlay a labelled 100 px grid and a red crosshair at the action point" },
                    "app": { "type": "string", "description": "package that must be in the foreground; otherwise nothing is done (guards against taps landing in the wrong app)" },
                    "expect": { "type": "string", "description": "after the action wait until this text or description is on screen (up to timeout ms) instead of a fixed wait" },
                    "timeout": { "type": "integer", "description": "expect: max ms to wait (default 5000)" },
                    "times": { "type": "integer", "description": "keyevent: repeat count (up to 10; with fast up to 200)" },
                    "fast": { "type": "boolean", "description": "keyevent: send all presses in ONE adb call (each call costs ~1.7 s on a TV)" },
                    "keys": { "type": "string", "description": "keyevent: a sequence in one call, e.g. \"dpad_right dpad_right enter\" (times repeats it)" },
                    "gap": { "type": "integer", "description": "keyevent: ms between presses, run as one on-device loop" },
                    "nth": { "type": "integer", "description": "goto: pick the nth (1-based, screen order) of several elements that match the label" },
                    "to": { "type": "string", "description": "scroll: keep scrolling until this text appears" },
                    "targets": { "type": "array", "items": { "type": "string" }, "description": "goto: labels to reach with the D-pad, in order; each gets focus, then OK. Text only (no screenshot) unless it fails" },
                    "max": { "type": "integer", "description": "goto: most D-pad presses per target (default 30)" },
                    "keep_going": { "type": "boolean", "description": "scenario: run the remaining steps after a failure (default: stop at the first). steps / json give the scenario; \"app\" or a launch step names the package" },
                    "no_ok": { "type": "boolean", "description": "goto: only move focus, do not press OK" },
                    "list": { "type": "boolean", "description": "focus: also list every focusable element on screen" },
                    "no_shot": { "type": "boolean", "description": "keyevent and other actions: answer with text (result + focus line) instead of a screenshot" },
                    "focus": { "type": "boolean", "description": "add the focus line to the result of a screenshot action" },
                    "device": { "type": "string", "description": "adb serial when several devices are connected" },
                    "steps": { "type": "array", "items": { "type": "object" }, "description": "batch: list of {action, ..., shot} steps (or pass json=<file>)" },
                    "json": { "type": "string", "description": "batch: path of a steps JSON file" }
                },
                "required": ["action"]
            }
        },
        {
            "name": "set_workspace",
            "description": "Optional. The workspace is the project around the folder the server was started in, so most agents never need this. Pins a workspace by path or alias for this server session only (it does not affect other agents or terminals).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Absolute path or registered alias name of project workspace" },
                    "alias": { "type": "string", "description": "Optional custom alias name to assign to this workspace in the registry" }
                },
                "required": ["path"]
            }
        },
        {
            "name": "get_workspace",
            "description": "Retrieve the currently active workspace root path and all registered project aliases in the HashMap.",
            "inputSchema": {
                "type": "object",
                "properties": {}
            }
        },
        {
            "name": "exec",
            "description": "Execute a command inside the active project directory sandbox.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "command": { "type": "string", "description": "Shell command to execute" },
                    "timeout": { "type": "integer", "description": "Seconds before the command is killed (default 30)" },
                    "out": { "type": "string", "description": "File to save the complete output to (kept even if the command is killed at the timeout)" }
                },
                "required": ["command"]
            }
        },
        {
            "name": "check",
            "description": "Fast type-check without building. In a TypeScript project (tsconfig.json) runs tsc (tsc -b for solution-style configs) and returns 'CHECK OK' or 'CHECK FAILED - N error(s)' with the errors only; other projects fall back to lint. Use it after edits, before a full build.",
            "inputSchema": { "type": "object", "properties": { "timeout": { "type": "integer", "description": "Seconds before tsc is killed (default 120)" } } }
        }
    ])
}
