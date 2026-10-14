# Project doctor

Read-only checks of a project folder. They never write, move or delete anything.

## `mysid doctor compare <old> <new>`

Two copies of a project (`v42`, `v43`): what changed, what did not, and whether the old folder can be deleted.

```
mysid doctor compare E:\Hypersonus\Hypersonus-mobile\v42 E:\Hypersonus\Hypersonus-mobile\v43
mysid doctor compare v42 v43 --brief          # totals, verdict and the per-folder table only
mysid doctor compare v42 v43 --max 100        # more entries per group (default 25, up to 500)
mysid doctor compare v42 v43 --everything     # also compare node_modules, build, .git, .gradle, .cxx ...
```

The first folder is the old one. The answer has:

| part | meaning |
|---|---|
| files, folders, size | for each side |
| identical | same bytes (compared byte by byte, not by date or size) |
| changed | different bytes: size change, line counts (`lines 403 -> 336`), and which side was modified later (`[newer in v43]`, a gap under 2 s counts as equal) |
| line endings only | the text is the same, only CRLF / LF differ |
| only in old / only in new | the path exists on one side only; a folder that exists on one side only is listed once (`old_only/  (12 files, 31 KB)`) |
| moved / renamed | a file missing from one place whose exact bytes sit at another path of the other folder: not lost |
| not compared | generated folders left out by default, with their file counts and sizes |
| by top-level folder | files on each side, same, differ, only-A, only-B |
| VERDICT | `NOT SAFE` when files exist only in the old folder; `CHECK` when a file in the old folder is newer; otherwise nothing is lost by deleting it |

The verdict looks at files, not at what is inside them: read the `changed` list before deleting if the old folder held edits you
want. Symbolic links are not followed or compared (they are counted). Nothing is deleted: removing the folder is up to you.
