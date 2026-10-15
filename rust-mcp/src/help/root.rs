
use crate::dispatch::*;
use crate::help::*;

/// `mysid help --claude`: the command list followed by a table from Claude Code's own tools to mysid commands.
pub(crate) fn client_help(client: &str) -> Option<String> {
    let section = match client {
        "claude" => r#"
For Claude Code: your tools, in mysid
=====================================
Run mysid through the Bash tool, from inside the project folder. Use file NAMES (Foo.kt), not paths.

  Your tool                         mysid
  --------------------------------  ---------------------------------------------------------------
  Read(file_path, offset, limit)    mysid read Foo.kt#L40-90        (several slices in ONE call)
                                    mysid read Foo.kt               a short file whole; a long source file: its outline + the command for a slice
                                    mysid read SymbolName           definition and call sites
                                    mysid overview Foo.kt           outline only: types and signatures with line numbers
  Grep(pattern, glob, -C)           mysid search "pattern" --types kt --context 1
  Glob(pattern)                     mysid list src/ui      every entry of a folder
                                    mysid graph            where the code lives (one call, with counts);  mysid graph src/ui = a folder's map
  Edit(old_string, new_string)      mysid patch Foo.kt "old" "new"            [--replace_all] [--dry-run]
    ...text with quotes, $, `, \     mysid patch Foo.kt --old-file a.txt --new-file b.txt
  MultiEdit(edits[])                mysid apply --file edits.txt      (SEARCH/REPLACE blocks, many files, all-or-nothing)
                                    mysid apply <<'EOF' ... EOF       (the quotes around EOF are required)
  Write(file_path, content)         mysid write src/New.kt --file content.txt
                                    mysid write src/New.kt <<'EOF' ... EOF    (refuses to replace a file: add --overwrite)
  (append to a file)                mysid append Notes.md --file more.txt   [--create]
  Bash(command)                     mysid exec "cmd"                   (30 s; --timeout N)
  Bash: build / test                mysid build   (errors only; full log in .mysid/build.log)
                                    mysid test [name]   (one line when green, failing tests when red)
  Bash: git status / cp             mysid status   (branch, changed files)    mysid copy <src>... <dest>
  Bash: dev server                  mysid dev   (background, returns the URL)   mysid dev log | stop

Edit semantics match yours: the text must match exactly once or nothing changes (NOT PATCHED / NOT APPLIED names why).
CRLF files stay CRLF. Trim any output with --head N, --tail N, --grep <regex>, --grep-v <regex> instead of piping.

Safe loop:  mysid checkpoint "label"  ->  edits  ->  mysid build  ->  mysid diff  ->  mysid commit "msg"
            not happy: mysid restore (preview), mysid restore --yes
More: mysid skill base   (the full workflow guide)
"#,
        "codex" | "antigravity" | "gemini" => {
            return Some(format!(
                "{}\n\n(`mysid help --{}`: the guide for this client is not written yet. Its native edit format is being added to `mysid apply`; until then the Claude Code section above shows the commands: `mysid help --claude`.)",
                get_root_help(),
                client
            ))
        }
        _ => return None,
    };
    Some(format!("{}\n{}", get_root_help(), section))
}

/// What `mysid help` shows: the seven commands that cover most work. `mysid help all` has every command.
pub(crate) fn get_root_help() -> &'static str {
    r#"mysid: code intelligence for LLM agents. Eleven commands cover most work:

   1. mysid set .                      pick the project; prints its source root, language, entry and the folders to look at
   2. mysid list <folder>              what is in a folder: sub-folders ranked by code, code files with line counts
   3. mysid overview <file.ext>        classes, functions and signatures with line numbers (no code)
   4. mysid read <file.ext>#L10-60     the code snippet (a SymbolName instead of a file works too)
   5. mysid search "text" [--types kt,ts] [--regex] [--context 1] [--group] [--count] [--max N] [--offset N]
                                       where is it? Shows 15 hits; if cut, the last line gives the --offset N for the next page (--max N shows more). --count = matches per file; --all also searches md/txt
   6. mysid graph <symbol> [--depth N]   callers and callees; --depth N = N hops (max 5, default 1); a file name: what it calls and who calls it
   7. change code, any of three:
        mysid patch <file> --stdin [--show]   exact old text. ALWAYS give code via --stdin (old text, a line with only =======, new text) or --old-file a --new-file b: quotes, $ and backslashes then never matter. --show = also print the removed (-) and added (+) lines, so you need not re-read the file
        mysid edit <file>#L10-20 "new text"   you know the line numbers (from read) [--insert-after | --insert-before | --delete | --expect "text"]
        mysid replace "old" "new"             the same change in many places        [--dry-run first]
   8. mysid patch_batch <file> --stdin [--show]   several replacements in ONE file, all or nothing: repeat <<<<<<< SEARCH / old text / ======= / new text / >>>>>>> REPLACE on stdin
   9. mysid file new dir/sub-dir/file.ext --stdin   create a new file, content on stdin (folders are made). If the file exists it stops with "already exists": add --overwrite to replace it
  10. mysid build                      build; errors only (the full log is .mysid/build.log)
  11. mysid get_workspace              CONTINUING earlier work? Run this first: which project this folder uses, and why (then set . if it is the wrong one)

Flow: (continuing earlier work: get_workspace) set . -> list <folder> -> overview <file> -> read <file>#L10-60 -> search "text" -> graph <symbol> -> patch | edit | replace | patch_batch | file new -> build.
One command in detail: mysid help <command>.   The other commands: mysid help remaining.
"#
}

pub(crate) fn get_full_help() -> &'static str {
    r#"mysid: code intelligence for LLM agents (command line and MCP server)

THE USUAL FLOW
   1. mysid set <path>                 optional: mysid finds the project from the folder you run it in
   2. mysid list <folder>              what is in a folder (every entry).  mysid graph = where the code lives, in one call
   3. mysid overview <file>            the outline: classes, functions, signatures with line numbers (no code)
   4. mysid read <file | symbol>       the code: Foo.kt#L121-220 (a slice), a SymbolName; a bare long file gives its outline; several in ONE call
   5. mysid search "terms"             where is it?        [--types kt,ts] [--context 1] [--regex] [--group] [--all = also md/txt]
   6. mysid graph <symbol | File.kt>   callers and callees (JNI bridges included); a file: what it calls and who calls it  [--depth 2] [--flow]
   7. mysid apply                      edit: many changes at once from SEARCH/REPLACE blocks on stdin or --file
   8. mysid patch <file> "old" "new"   edit: one exact replacement  [--old-file a --new-file b | --stdin for big text]
   9. mysid file new <path>            new file; content from stdin (--stdin) or --from <file>
                                       (mysid write <path> does the same but never overwrites; mysid append <path> adds)
  10. mysid build                      verify: errors only. mysid check = fast TypeScript type-check
  11. mysid help --claude              this flow in Claude Code's own tool names  (--codex, --antigravity: coming)
  12. mysid skill android              the Android workflow guide
  13. mysid skill react                React project: components, hooks, tsc, lint, dev server, Netlify release
  14. mysid skill base                 stack unknown: the general workflow
  15. mysid file <action>              more file commands: new, copy, rename, delete, mkdir, list  ('file --help')

SAFE EDITING
  mysid checkpoint "label"  ->  edits  ->  mysid build  ->  mysid diff  ->  mysid commit "message"
  not happy: mysid restore (preview) / mysid restore --yes.   mysid history = the last commits.

MORE
  mysid replace <old> <new>            search-and-replace across files  [--types] [--dry-run]
  mysid patch_batch --json <file>      atomic multi-file patches
  mysid test [name]                    run the tests: one line when green, failing tests with messages when not
  mysid bootstrap                      git init only (nothing staged or committed); --gitignore writes a .gitignore for the stack
  mysid vcs                            the version-control workflow: bootstrap -> .gitignore -> commit -> status -> publish
  mysid git purge  (or: mysid purge)   remove git from the project (preview; --yes does it; irreversible, ask the owner)
  mysid status                         git: branch, ahead/behind, changed files   (mysid branch lists branches)
  mysid publish <repo-url>             first upload to GitHub: shows the plan; add --yes to push (ask the owner first)
  mysid copy <src>... <dest>           copy files or folders (no overwrite unless --overwrite)
  Any command takes --head N, --tail N, --grep <regex>, --grep-v <regex> to trim its output
  mysid dev                            start the dev server in the background, get its URL; dev log | status | stop
  mysid deps                           npm packages the code imports that package.json is missing
  mysid exec <command>                 run a shell command  [--timeout N] [--out file] [--shell cmd|powershell|bash] [--errors] [-- ends mysid flags]
  mysid msvc <command>                 run with the Visual Studio C++ environment (vcvars64) set up;  mysid emsdk <command>: the Emscripten SDK
  mysid new react <name> [--dir D]     create a React (Vite + TypeScript) project

mysid <command> --help gives the details of any command.
"#
}

/// The commands `mysid help` lists; `mysid help remaining` is everything else.
pub(crate) const SHORT_HELP_COMMANDS: &[&str] = &["set", "list", "overview", "read", "search", "graph", "patch", "edit", "replace", "patch_batch", "file", "build", "get_workspace", "getworkspace", "help"];

/// The Android / device commands: not listed in `mysid help remaining`; `mysid android help` is their one page.
pub(crate) const ANDROID_COMMANDS: &[&str] = &["android", "adb", "devices", "wifi", "connect_wifi", "debug", "release", "aab"];

/// The file operations: not listed in `mysid help remaining`; `mysid file help` is their one page.
pub(crate) const FILE_COMMANDS: &[&str] = &["write", "append", "mkdir", "copy", "cp", "rename", "mv", "delete", "rm", "create_file", "list_dir", "ls", "dir", "listdir"];

/// The reading utilities, with their full syntax: shown at the top of `mysid help remaining`.
pub(crate) const READING_UTILITIES: &str = "UTILITIES: look at the code (the short help has the everyday forms; these are all the options)
  mysid read <file>                      a file up to 150 lines whole; a longer source file gives its outline and the command for a slice
  mysid read <file>#L10-60               a slice, up to 200 lines (600 with --full); File.kt:10-50 also works
  mysid read <slice1> <slice2> ...       several slices in ONE call
  mysid read <Symbol>                    a symbol's definition and its top callers
  mysid tree [folder] [--depth 1-4] [--all]
                                         the folder map: folders with their code-file counts, depth 2 by default; --all adds dot-folders
  mysid overview <file> [<file2> ...]    classes, functions and signatures with line numbers (no code)
  mysid overview <folder> [--depth 1] [--match word] [--format json]
                                         outlines of every file in a folder; --depth 1 = folder names and counts only; --match keeps files
                                         whose path, class or function contains the word
  mysid search \"text\" [--types kt,ts] [--regex] [--case] [--context 1] [--group] [--count] [--max N] [--offset N] [--all] [--vendor]
                                         --count = matching lines per file; --offset N = next page; --max N up to 500; --group = file name once;
                                         --all also searches md/txt; --vendor also searches external/ and third_party/
  mysid symbols <file>                   a flat list of the symbols in one file: [KIND] name line N (cheaper than overview)
Files are named by name only (Foo.kt); add a folder (ui/Foo.kt) only if two files share a name.
";

/// Commands shown in the utilities block instead of the one-line list.
pub(crate) const UTILITY_COMMANDS: &[&str] = &["tree", "symbols"];

/// One line per command that is not in the short help: its name and the first line of its own help page.
pub(crate) fn get_remaining_help() -> &'static str {
    static TEXT: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    TEXT.get_or_init(|| {
        let mut lines: Vec<String> = Vec::new();
        let mut seen: Vec<&'static str> = Vec::new();
        let mut without_page: Vec<&str> = Vec::new();
        for name in KNOWN_COMMANDS {
            if SHORT_HELP_COMMANDS.contains(name) || ANDROID_COMMANDS.contains(name) || FILE_COMMANDS.contains(name) || UTILITY_COMMANDS.contains(name) {
                continue;
            }
            match get_tool_help(name) {
                Some(page) => {
                    let first = page.lines().next().unwrap_or("").trim();
                    if seen.contains(&first) {
                        continue;
                    }
                    seen.push(first);
                    let what = first.split_once(": ").map_or(first, |(_, w)| w);
                    lines.push(format!("  {:<14}{}", name, what));
                }
                None => without_page.push(name),
            }
        }
        let mut out = format!("mysid: the commands that are not in `mysid help`\n\n{}\nOTHER COMMANDS ({} with a help page)\n\n{}\n", READING_UTILITIES, lines.len(), lines.join("\n"));
        if !without_page.is_empty() {
            out.push_str(&format!("\nAlso accepted (aliases and small helpers): {}\n", without_page.join(", ")));
        }
        out.push_str("\nFile operations (new, write, append, mkdir, mv, cp, rm, ls): mysid file help\n");
        out.push_str("Android / device commands (devices, wifi, install, debug, release, goto, scenario, keyevent ...): mysid android help\n");
        out.push_str("\nOne command in detail: mysid help <command>.   The commands in the short list: mysid help.\n");
        out
    })
}
