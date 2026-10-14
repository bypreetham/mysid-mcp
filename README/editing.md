# Editing without shell quoting

`mysid patch File "old" "new"` is fine for a short change. Code with quotes, `$`, backticks or backslashes does not survive
a shell: the shell rewrites the text before mysid sees it. These forms deliver the text without shell parsing.

## `mysid apply`: many edits, one call

```
mysid apply --file edit.txt            # write edit.txt with a file tool, then apply it (no quoting at all)

mysid apply <<'EOF'                    # bash / Git Bash: the quotes around EOF stop $ and backticks expanding
Page.tsx
<<<<<<< SEARCH
const msg = "hi"
=======
const msg = `Hello ${user.name}, cost $5 \n "quoted" 'single'`
>>>>>>> REPLACE
cfg.txt
<<<<<<< SEARCH
path = "C:\old"
=======
path = "C:\new\dir"
>>>>>>> REPLACE
EOF

@'
...blocks...
'@ | mysid apply                       # PowerShell: single-quoted here-string
```

- **Blocks.** The file name is the line before `<<<<<<< SEARCH` (also accepted: `**file**`, `File: file`, a code fence above).
  Prose around the blocks is ignored. Without a name, `--to <file>` supplies one.
- **Bare file names** work, and an ambiguous name lists the choices, as everywhere else.
- **Matching** is the engine of `patch`: exact text, then the same ignoring CRLF/LF differences, then ignoring surrounding
  whitespace; a block that matches 0 or 2+ places is refused (`--replace_all` allows several). CRLF files stay CRLF.
- **Order.** Blocks for one file apply in order, each to the result of the one before.
- **All or nothing.** Nothing is written unless every block applies. A failure starts with `NOT APPLIED - nothing was changed.`,
  names the block and file, shows the first line searched, and exits with status 1.
- **`--dry-run`** previews each change and writes nothing.
- **New files.** An empty SEARCH on a file that does not exist creates it (an empty SEARCH on an existing file is refused).

## `patch --old-file / --new-file`

```
mysid patch Page.tsx --old-file a.txt --new-file b.txt
```

One replacement with the text read from files. One trailing newline in each file is dropped (the usual editor artifact),
so a snippet saved with a final newline still matches.

## Over MCP

The `apply` tool takes the blocks as its `text` parameter, so no shell is involved there.

## New files and appending

```
mysid write src/ui/Page.tsx --file content.txt        # or: mysid write src/ui/Page.tsx <<'EOF' ... EOF
mysid write src/ui/Page.tsx --overwrite --file new.txt   # replacing an existing file needs --overwrite
mysid append Notes.md --file more.txt                  # added on its own line; the file is found by name; --create makes it
```

- `write` creates the file and its folders in one step, with the text exactly as given. It refuses to replace an existing file
  (`NOT WRITTEN - ... already exists`) and to leave the workspace with `..`. Replacing a CRLF file keeps it CRLF.
- `append` keeps the file's line endings and puts the new text on its own line even if the file did not end with one.
- Text from stdin must arrive within 10 seconds, so a caller that leaves stdin open cannot hang the command.
- A failed `patch` now starts with `NOT PATCHED - <reason>` and exits with status 1, like `NOT APPLIED` and `NOT WRITTEN`.

## Large text for `patch`

`mysid patch File.ts --old-from a.txt --new-from b.txt` (same as `--old-file/--new-file`), or
`mysid patch File.ts --stdin <<'EOF' ...SEARCH/REPLACE block... EOF`, which hands the block to `apply` for that file.

## `mysid file new` (older form)

`mysid file new <path> --stdin | - | --from <file>` also takes its content from stdin or a file, and piped input works without a
flag. Unlike `write`, `file new` replaces an existing file silently (that is its documented behaviour); prefer `write`.

## `mysid help --claude`

Prints the command list followed by a table from Claude Code's own tools to mysid (Read, Grep, Glob, Edit, MultiEdit, Write, Bash).
`--codex` / `--antigravity` / `--gemini` are accepted but only say their guide is not written yet.

## Not yet

`insert --after "anchor"`, a line-range `edit`, and the Codex `apply_patch` / unified-diff input formats (the parser says so when it sees one).

### When a patch does not match

`NOT PATCHED` / `NOT APPLIED` now say why, not only that:

```
NOT PATCHED - old_string block could not be found in target file.
this file has MIXED line endings (2 CRLF lines, 2 LF lines): a block that spans both kinds cannot match as written; patch the lines of one kind at a time
closest line 2: `return compute(1, 2);` (searched for `return compute(1, 3);`; they first differ at column 19)
text typed on a command line can be altered by the shell (quotes, backslashes, %, $): --old-file/--new-file, --stdin or mysid apply send it untouched
```

The closest line is the file line most like the first line of the searched text (character-pair similarity, at least 0.6), with the column of the first difference. When nothing resembles it, the answer says the text may belong to another file or the file changed. Mixed line endings are counted and named. Text with quotes, backslashes, `%%` or `$` should go through `--old-file/--new-file`, `--stdin` or `mysid apply`, never inline.

### The shortest multi-line patch

`mysid patch File.kt --stdin` also accepts just the old text, a line containing only `=======`, and the new text. No markers, no quoting:

```
mysid patch Player.kt --stdin <<'EOF'
    val retries = 3
=======
    val retries = 5
EOF
```

For several changes or several files use `mysid apply` (SEARCH/REPLACE blocks).

`mysid search "text" --group` prints each file name once and `  line: text` under it, which is shorter for big result sets. A very long line (a minified bundle) is shown as a window around the match, with the number of characters cut.

## Three ways to change code

| you know | use | example |
|---|---|---|
| the exact old text | `mysid patch` | `mysid patch Foo.kt "old" "new"` |
| the line numbers (from `mysid read`) | `mysid edit` | `mysid edit Foo.kt#L10-20 --file new.txt` |
| a change that repeats in many places | `mysid replace` | `mysid replace "OldName" "NewName" --dry-run` |

`mysid edit <file>#L10-20 "new text"` replaces lines 10 to 20 (`#L10` is one line). `--insert-after` and `--insert-before` add the text beside
a line, `--delete` removes the lines. The text comes from the last argument, `--file`, or stdin, so quotes never matter.
`--expect "text"` makes the edit refuse unless the first named line contains that text: use it when the file may have changed since
it was read, because line numbers move and a wrong range would otherwise change the wrong lines. The answer lists the removed (`-`)
and added (`+`) lines and the new line numbers; CRLF files stay CRLF and a last line without a line ending keeps none. All or nothing.
