# Looking at a project: list, overview, read

One verb, one meaning. Small models pick commands by name, so a command never returns a different kind of thing depending on what
it is given.

| Command | Returns | Never |
|---|---|---|
| `mysid list <folder>` | every entry of a folder: `D` folders, `F` files, with a count line (`# 12 entries: 5 folders (D), 7 files (F)`) | file content |
| `mysid graph` / `graph <folder>` | where the code lives: indexed source folders with file and symbol counts (see [graph.md](graph.md)) | non-source files |
| `mysid overview <file>` | the outline: types and signatures with line numbers | code bodies |
| `mysid read <file>` | **code**: a file up to 150 lines, whole | |
| `mysid read BigFile.kt` | a source file over 150 lines: its **outline** (names and line numbers, at most 90 entries) plus the exact `read` command for a slice, so a bare read never floods the context | code bodies |
| `mysid read File.kt#L121-320` | the code of a slice (up to 200 lines; `--full` 600) | |
| `mysid read SymbolName` | a symbol's definition and its callers | |

## A bare `read` of a long source file

```
[Player.kt: 1840 lines. Too long to print whole, so this is the outline (line numbers in brackets). Read code with: mysid read Player.kt#L42-101]
    [L42] class Player(private val engine: Engine)
    [L58] fun play(uri: Uri)
    [L131] private fun onUnderrun()
    ...
[start of the file: mysid read Player.kt#L1-100   a symbol: mysid read <SymbolName>   search inside: mysid search "text" Player.kt]
```

Only for source files the parsers know; logs, JSON and other text keep the plain 120-line window below. A range (`#L10-60`), `--full` or an offset always returns code.

## What `read` says when there is more

```
[Big.kt:1-120/300]
1:...
120:...
[showing 1-120 of 300. More: mysid read Big.kt#L121-240; signatures only: mysid overview Big.kt]
```

A short file has no footer. A slice you ask for explicitly has no footer either.

## What `read` says for a folder

`mysid read app` is an error, not a listing: `ERROR: 'app' is a folder, not a file. Its entries: mysid list app   Where the code is:
mysid graph app`. (An earlier version answered with a source map; it looked like an outline and hid non-source files.)

## Flow

```
mysid set <path>          # optional
mysid list <folder>       # what is here
mysid overview <file>     # its shape
mysid read <file>         # its code
mysid search "terms"      # where is it   (--max 100 up to 500; --count = matching lines per file; --offset 25 = next page)
mysid graph <symbol>      # who calls it, what it calls
```
