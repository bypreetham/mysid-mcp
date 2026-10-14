# File names instead of paths

Every mysid command that takes a file accepts just its **name**; mysid finds it anywhere in the workspace. Its output
uses the same short names, so what a model reads is what it types back.

```
mysid read MusicService.kt#L100-150        (not app/src/main/java/com/.../MusicService.kt)
mysid patch FlacDecoder.cpp "old" "new"
mysid search "TAG =" --types kt            ->  MusicService.kt:114: private val TAG = ...
```

## Resolving a name

`resolve_file_in_workspace` (`tools/workspace.rs`), in order:

1. The text as a path relative to the workspace (or absolute), if it exists.
2. A per-workspace cache of bare names (`%LOCALAPPDATA%\mysid\cache`), for names without a folder.
3. The workspace file list (`index::scan::list_files`: gitignore-aware, minus build output and vendored dirs).
   `Foo.kt` matches any `.../Foo.kt`; `ui/Foo.kt` matches only `.../ui/Foo.kt`; case is ignored if nothing matches exactly.

One match is used. Several are never guessed; the error shows how to disambiguate:

```
'Foo.kt' is ambiguous - 2 files; repeat with a folder, e.g. one of:
  ui/Foo.kt   (app/src/ui/Foo.kt)
  data/Foo.kt   (app/src/data/Foo.kt)
```

Used by `read`, `patch`, `overview`, `search` (file arguments), `symbols`, `replace --file`, `lint`, and the source of
`file rename`. `delete` and creating files still take real paths on purpose.

## Short names in output

`search`, `read`, `graph` and `overview` print each file as the shortest trailing part of its path that is unique
(`Bar.kt`, or `ui/Foo.kt` when two files share the name), via `tools/paths.rs` `Shortener`. Every short form resolves
back to exactly its file (covered by a test). Directories in the `graph` map are left as they are.

## Telling the model

- MCP tool descriptions and `mysid <command> --help` say "file name only" with bare-name examples.
- A project can add one line to its `CLAUDE.md` / `AGENTS.md`; Hypersonus has a "Use file names, not paths" section.
