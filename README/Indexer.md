# Indexer

The indexer turns source code into a small database that every code-aware command reads: `graph` today,
`read` for symbol lookups, and `search`/others next. It lives in `src/index/` and knows nothing about the tools
that use it.

```
parsers ──► extract ──► domain model ──► DAOs ──► SQLite  <workspace>/.mysid/index.db
                              │                      ▲
                              └─► resolve (HashMaps) ┘   edges
                                                     │
                      Index facade ◄─────────────────┘
                        candidates · neighborhood · members · has_path · snapshot
                              │
                              ▼
                  graph / read  (they build their own result objects)
```

## The idea

Parsing a project on every command is slow and wasteful. The index parses each file **once**, stores what was
found, and keeps it current by re-parsing **only files that changed**. Commands then ask small indexed questions
("who calls `X`?") instead of scanning files, so a lookup costs the same on a 300-file project and a 30,000-file one.

It is a **cache**: everything in it can be rebuilt from the source, so any mismatch or corruption simply rebuilds it.

## Roles of each layer

| Layer | Role | Where |
|---|---|---|
| **Domain model** | mysid's own concepts, independent of storage | `model.rs` |
| **DAO** | the only code that speaks SQL; maps rows to model types | `dao/` |
| **HashMaps** | in-memory accelerators during resolution; never persisted | `resolve.rs`, `mod.rs` |
| **DTO / results** | shapes made for a consumer (graph's partial graph, `Neighborhood`, `Snapshot`) | `mod.rs`, `tools/graph/build.rs` |

Keeping these apart is what lets the storage change without touching the tools, and the other way round.

## Data model

```
File     id, path, language, mtime, size, lines
Symbol   id, file_id, name, kind, detail, owner, start_line, end_line
Edge     from, to, kind
```

| Type | Notes |
|---|---|
| `File.path` | workspace-relative, `/` separators. `mtime` (ms) + `size` drive change detection. |
| `File.language` | which parser read it (`Kotlin`, `Java`, `C/C++`, ...). Used to keep calls inside a language family. |
| `Symbol.kind` | `class`, `function` or `method`. |
| `Symbol.detail` | the parser's own kind: `interface`, `struct`, `arrow_function`, ... |
| `Symbol.owner` | the owning type for a method (`Foo` in `Foo.bar`); empty for classes and free functions. |
| `Symbol.end_line` | the **body** end (brace block or indentation block), not just the signature. |
| `Edge.kind` | `CALLS` today. `DEFINES` is derived from `owner` when a graph is built. |
| `Located` | a symbol plus its file path: what lookups return. |

SQLite tables (schema in `dao/db.rs`):

```sql
files      (id, path UNIQUE, language, mtime, size, lines)
symbols    (id, file_id → files ON DELETE CASCADE, name, name_lower, kind, detail, owner,
            start_line, end_line,  UNIQUE(file_id, owner, name))
call_sites (symbol_id → symbols ON DELETE CASCADE, callee, dotted)     -- raw calls found in bodies
edges      (from_id → symbols, to_id → symbols, kind,  PRIMARY KEY(from_id, to_id, kind))
signatures (file_id → files ON DELETE CASCADE, ord, row_kind, name, kind, signature, params,
            return_type, start_line, end_line, parent)   -- every type and method the parser reported, overloads included
```

`signatures` keeps the parser output as is (one row per type and per method, in source order), because `symbols` collapses
overloads. `overview` answers from it: a named file is refreshed on its own (`refresh_paths`, no tree walk) and read with one query; a
directory refreshes the index once. A file the index does not hold (Markdown, skipped folders) is parsed directly. Output is
identical to parsing (checked on five Hypersonus files, text and JSON); speed is about the same on a small project and
better on large directories.

Indexes: `symbols(name)`, `symbols(name_lower)` (case-insensitive lookup), `call_sites(symbol_id)`,
`call_sites(callee)` (finding callers affected by a change), `edges(to_id)` (callers). Deleting a file cascades to its
symbols, call sites and every edge touching them. `UNIQUE(file_id, owner, name)` collapses overloads and
`struct Foo` + `impl Foo` into one symbol.

`call_sites` holds *unresolved* names; `edges` holds the *resolved* links. Edges depend on the whole workspace (is the
name unique?), so they are recomputed; call sites depend on one file only, so they are cached.

## Indexing flow

1. **Scan** (`scan.rs`). A parallel, `.gitignore`-aware walk. Directories are skipped by the **stack adapter**
   (`build`, `node_modules`, `.gradle`, vendored `external/` / `third_party/` / `vendor/`, and path rules such as an
   Android `src/main/cpp/include`). Only files a parser handles are `stat`-ed.
2. **Change detection.** A file is dirty when it is new or its mtime/size differ from the stored row; stored files no
   longer on disk are removed. If nothing is dirty the refresh finishes without taking a write lock.
3. **Extract** (`extract.rs`). The language parser returns types and methods; each function body (its real block) is
   scanned for calls (`name(`, and `<Component` in JS/TS). Calls are de-duplicated per function.
4. **Write.** One `BEGIN IMMEDIATE` transaction for the whole batch: delete the old rows of changed files, insert files,
   symbols and call sites.
5. **Resolve edges** (`resolve.rs`), one shared rule for full and incremental runs: same-language candidates only;
   prefer the same file; otherwise link only if the name is unique; dotted calls to very common method names link only
   within the file.

### Full vs incremental resolution

| Situation | What is re-resolved |
|---|---|
| First index, or at least 1/4 of the files changed | everything, in memory, in one pass |
| A few files changed | only the **affected callers**: symbols in the changed files, plus any symbol anywhere whose call sites mention a name that the change added or removed (adding a second `f3` makes every call to `f3` ambiguous; removing it makes it unique again) |

A test asserts that after additions, deletions, rewrites, path-only refreshes and directory removals the incremental
edges are **identical** to a from-scratch resolution.

## Reads (the `Index` facade)

| Method | Use |
|---|---|
| `candidates(query)` | symbols a query could mean: exact (any case) → `Class.method` → `path:Name` → fuzzy |
| `neighborhood(seeds, depth)` | symbols within `depth` calls, both directions, with the connecting edges |
| `members(class)` | methods of a class, in source order |
| `has_path(prefix)` | is this an indexed file or directory |
| `snapshot()` | everything (files, symbols, edges) for views that need it, such as the map |

## Refresh APIs

- `refresh(root)` walks the workspace and applies what changed. Called by `Index::open`, i.e. on every command that needs the index.
- `refresh_paths(root, paths)` re-indexes exactly the named files or directories (new → parsed, gone → removed;
  a directory removal drops everything under it). Built for a file watcher. It applies the adapter's skip rules but not
  `.gitignore`, so a watcher must filter events itself.

## Robustness

- **Versioning.** The database stores `SCHEMA_VERSION * 1000 + PARSER_VERSION`. Changing the schema, a parser, or the
  call-linking rules bumps it and the index is rebuilt on the next run.
- **Fallbacks.** An unreadable or corrupt `index.db` (and its `-wal`/`-shm`) is deleted and recreated; if the folder is
  not writable the index lives in memory for that run.
- **Concurrency.** WAL journal, `synchronous=NORMAL`, a 5 s busy timeout, and write transactions that start with
  `BEGIN IMMEDIATE`, so two mysid processes can use one workspace.
- **Cached statements** (`prepare_cached`) for the hot inserts.

## Text index (for `search`)

A second, independent cache in `<workspace>/.mysid/text.db` (`src/textindex/`) answers "which files can contain this
text?" so `search` reads only those files.

- **What is stored.** One row per searchable text file: `path, mtime, size` and a **bloom filter** of the file's
  lower-cased 3-byte sequences (trigrams), about 6 bits per distinct trigram. Binary files (by extension, by a NUL in the
  first 8 KB) and files over 1.5 MB get an empty filter and are never searched, as before.
- **Query.** The regex is parsed (`regex-syntax`) and the literals every match must start with are extracted
  (`buffer_length|ctx->buffer|isIso` gives `buffer_length`... as prefixes). A file is a candidate when all trigrams of any one
  literal are present in its filter. The candidates are then scanned with the normal regex, so results are identical to a
  full scan; a filter can only keep an extra file, never drop a real one.
- **Fallback.** A pattern with no literal of 3+ ASCII characters (`ab`, `\w+Service`, non-ASCII text), a named file or
  directory, or any error uses the previous full walk. `MYSID_NOINDEX=1` forces it.
- **Filled lazily, not up front.** Nothing is indexed until a search needs it. Each search walks the tree for metadata
  only and sorts files into: *fresh filter* (same mtime and size: read only if the filter allows the pattern) and
  *unknown or changed* (read and searched, and the filter is saved from that same read, in batches of 500). So the first
  search of an area costs a normal scan, later ones read only likely files, and the index grows with use. A
  search that stops early (result cap) simply leaves the rest unindexed; a `--types kt` search only learns `.kt` files.
- **Always correct.** A row stores the mtime and size seen *before* the file was read. If the file changes meanwhile, the
  row stops matching and the file is read again next time; rows are only written if the file still has that mtime and
  size. Deleted files' rows are dropped at the end of a search. Results equal a full scan (checked on queries with a
  partial index).
- **Cost.** The tree walk is the floor (about 12 ms on 300 files, about 320 ms on 65k files). On Hypersonus: first
  search 74 ms (reads and learns everything), later searches about 13 ms against 17 ms for a full scan. On a 65,674-file
  tree a full scan took 365-405 s every time, an indexed repeat search about 470 ms. `MYSID_TIMING=1` prints
  `search (indexed|full scan)`.
- **Next.** A background worker that fills the index while idle (pausing whenever a tool runs), and a watcher that removes
  the tree walk. Neither exists yet.

## Where it is stored

`<workspace>/.mysid/index.db` (code index) and `.mysid/text.db` (text index), plus `-wal` and `-shm` while in use.
See [agent-data.md](agent-data.md).

## Tests

`src/index/mod.rs` covers: incremental add/update/delete, incremental edges equal to a full resolution, `refresh_paths`
(modified, deleted, added, directory removal), candidates (exact, qualified, `path:Name`, fuzzy, literal `_`/`%`),
neighborhood depth, language-family linking, and the DAO caller/callee queries.

## Not done yet

- **File watcher.** A background process using `refresh_paths` so a command can skip the file-tree walk (the remaining
  ~10 ms above process start). The build worker's detached-process, heartbeat and status-file pattern is reusable.
- Targeted reads for the workspace map.
- Indexing `AndroidManifest.xml` components and ProGuard keep rules; a cross-language binding check (JNI and similar).
