# mysid graph

`mysid graph` answers one question for an LLM or a developer: **who calls this symbol, and what does it call?**
It does so in a few lines instead of a dump of the whole codebase.

An agent only has to learn three concepts:

```
SYMBOL   what to look at
DEPTH    how many hops of callers/callees to include
FLOW     draw the same data as trees
```

## Commands

| Command | Result |
|---|---|
| `mysid graph X` | compact card: callers, `X`, callees (1 hop) |
| `mysid graph X --depth N` | callers and callees up to N hops (default 1, max 5) |
| `mysid graph X --flow` | the same relationships as `UPSTREAM / TARGET / DOWNSTREAM` trees |
| `mysid graph X --flow --depth 2` | trees, 2 hops |
| `mysid graph` | map of the workspace (directory roll-up) |
| `mysid graph src/Pages` | map of a directory, or the symbols of a file |

`--flow` changes **presentation only**. Both directions are always queried, because callers + target + callees is
what an LLM needs by default. `X` may be `Name`, `Class.method`, `Class::method`, or `path/File.kt:Name`.

### The card (default)

```
useDarkMode [function]
src/hooks/useDarkMode.ts:3

CALLERS (2)
  SettingsPage  src/Pages/SettingsPage.tsx:20
  Navbar        src/components/Navbar.tsx:18

CALLEES (0)
  none
```

With `--depth 2`, entries beyond the first hop carry a `[hop 2]` tag and are listed after the nearer ones.

### The map: where the code lives

`mysid graph` rolls files up by folder and **opens it up to where the code is**, so an agent at a
project root does not have to walk `app/` -> `src/` -> `main/` -> `java/` -> `com/` one call at a time. Single-child chains
are followed (`src/main/java/com/x/` is one row), then the biggest folder is opened, repeatedly, until about a dozen
rows show. An opened folder shows its eight biggest sub-folders and one `(+9 more folders: ...)` row, and
`(+3 files directly here)` for files lying beside them. Leaf folders stay plain rows. Android example, one call:

```
working - 303 files, 3539 symbols
  app/src/main/cpp/  113 files, 1529 symbols
  app/src/main/java/com/hypersonus/player/  (+9 more folders: 26 files, 217 symbols)
  app/src/main/java/com/hypersonus/player/ui/  54 files, 379 symbols
  app/src/main/java/com/hypersonus/player/playback/  24 files, 406 symbols
  ...
```

`mysid graph <folder>` repeats this inside that folder; `mysid graph <file>` lists its symbols; a small project (the
Vite template) stays a two-line map.

### The tree (`--flow`)

```
SettingsPage [function]  src/Pages/SettingsPage.tsx:20

UPSTREAM
  \- LaunchApp  src/components/LaunchApp.tsx:39
     +- App  src/App.tsx:15
     \- backToHome  src/App.tsx:26

TARGET
  SettingsPage

DOWNSTREAM
  +- handleLogout  src/Pages/SettingsPage.tsx:26
  |  \- signOut  src/context/AuthContext.tsx:76
  +- handleDelete  src/Pages/SettingsPage.tsx:37
  |  +- confirm  src/confirm/confirmService.ts:23
  |  \- deleteAccount  src/context/AuthContext.tsx:81
  +- confirm  src/confirm/confirmService.ts:23  (seen)
  \- useDarkMode  src/hooks/useDarkMode.ts:3
```

Connectors are plain ASCII (`+-`, `\-`, `|`): Unicode box characters garble in some Windows consoles and cost
more tokens. They are constants at the top of `view/flow.rs` if you prefer them.

## How a lookup works

```
user text ──► candidates() ──► resolve() ──► neighborhood(depth) ──► card | flow
              (SQLite)         (name rules)   (SQLite, BFS)           (render, line budget)
```

1. **Candidates.** The index (see [Indexer.md](Indexer.md)) returns the symbols the text could mean:
   exact name in any case → `Class.method` → `path:Name` → names containing the text (fuzzy, only when nothing else matched).
2. **Resolve.** One match is used. A class and its same-named constructor count as one symbol. Several matches
   are *listed*, never silently picked:
   ```
   'process' matches 21 symbols - repeat with Class.method or file:Name:
     AudioNode.process  [method]  app/.../AudioNode.h:19
   ```
   If nothing matches but the text is a directory or file path, the map is shown instead.
3. **Neighborhood.** A breadth-first walk over the `edges` table in both directions, at most `depth` hops. It loads
   only those symbols and edges, so the cost does not grow with project size.
4. **Render.** The card or the tree, within a fixed line budget.

### Depth

- Callers and callees are walked **independently**, each with its own visited set, so a symbol that is both an
  upstream caller and a downstream callee is handled correctly (cycles cannot loop).
- Edges are loaded for every node up to hop `depth − 1`; nodes at the last hop are **leaves** (their own edges are
  never read). That is what makes `--depth 5` cheap.
- Depth is clamped to 1–5.

### Tree walk

- A recursive walk over the loaded edges: the roots are the direct callers (UPSTREAM) or callees (DOWNSTREAM), and
  each node's children are its callers/callees one hop further out.
- A node already printed elsewhere in the tree is marked `(seen)` and not expanded again, which also stops cycles.
- Limits keep the output small: 8 children per node (`+N more`), 60 lines in total, then
  `... N more line(s) hidden - lower --depth or query a narrower symbol`. The card shows 12 entries per list.

### Classes

A class target stands for itself **and its methods**: callers are everything outside the class that calls any of its
methods, callees are everything its methods call, and the card adds `MEMBERS (n): ...`.

## How calls are found (and how accuracy improved)

The first version only looked for calls on the declaration line, so `flow` and `impact` returned almost nothing.
It also duplicated per-language regexes. The current logic:

| Step | What it does |
|---|---|
| **Parsers** | Symbols come from the shared language parsers (`src/parsers/`), not graph-specific regexes. |
| **Body ranges** | A function's body is its real block: the brace-matched `{ ... }`, or the indentation block for Python. Nested helpers stay inside the enclosing function, so a React component's JSX after its inner handlers is still credited to the component. |
| **Call extraction** | `name(` in the body, plus `<Component` for JS/TS (JSX usage). Language keywords and logging macros are ignored. |
| **Linking** | Same-language candidates only. Prefer a target in the **same file**; otherwise link only if the name is **unique** in the workspace. |
| **Noise filter** | A dotted call to a very common method name (`.push(`, `.get(`, `.len(` ...) only links within the same file, so `vec.push()` never points at a user method called `push`. |
| **Language families** | Calls never cross languages by plain name, except Kotlin ↔ Java (one JVM family). A C++ `start()` is not linked to a Kotlin `start()`. JNI is linked separately (below). |
| **Typed receivers** | For `negotiator.setup(...)` the declared type of `negotiator` (a property, parameter, local, `Type* p`, `shared_ptr<Type> p`, `p = Type(...)`) picks the `setup` of that type, even when the caller's own class has a `setup` too. `this.x()` is the enclosing type, `Engine.create()` / `Engine::create()` the type `Engine`; `std::...` is ignored. |
| **Declarations in headers** | For a C/C++ source, the type of a receiver is also looked up in the headers it includes (quoted includes, up to 3 levels, found next to it or in a parent folder) and its same-named header: members such as `Decoder* mDecoder;` are declared there, the calls are in the `.cpp`. `Type *p`, `Type* p` and `Type &p` are all read. |
| **Chained calls** | `a->getDecoder()->setRate(x)`: the receiver is the result of `getDecoder`, so its type is what the methods named `getDecoder` return (when they all agree, e.g. `IDecoder*` or `shared_ptr<IDecoder>`). |
| **Thread and callback entries** | `std::thread(&Streamer::loop, this)`, `std::bind(&X::f, this)` and Kotlin/Java `this::loop` / `Player::play` count as uses of the method: `start` shows `loop` as a callee and `loop` shows `start` as a caller. |
| **Own-class calls** | A plain call inside a method (`decode(5)`, no receiver) links to a method of the same class first, including a pure virtual declared in the header (`BaseDecodeWorker.loop` -> `BaseDecodeWorker.decodeAndPushChunk`). |
| **Overrides** | A base class method gets an edge to the methods that override it in derived classes (`BaseDecodeWorker.decodeAndPushChunk` -> `UsbDecodeWorker.decodeAndPushChunk` and the Oboe one), so a flow goes through the dispatch. Bases are read from `class A : public B`, Kotlin / C# / Swift `class A(x) : B(), I`, Java / TypeScript `extends` / `implements`. Only when a method has **at most 4** overrides: an interface with 29 implementations (`IDecoder`) is not fanned out. |
| **Virtual calls** | A call through a pointer to an interface (`IDecoder *d; d->setTargetSampleRate(r)`) links to the interface's method. The implementations in the subclasses are not listed (they are not found by call): `mysid graph IDecoder` shows the type, `mysid search "::setTargetSampleRate"` the overrides. |
| **libc and `std::`** | `strerror(errno)`, `memcpy`, `printf`, `std::move`, ... are never matched to a project symbol of the same name. A line that is the continuation of a call's arguments (`errno, strerror(errno));`) is no longer read as a function declaration. |
| **`new Foo(...)`** | The edge to the class `Foo` is left out of a graph view when the caller also calls methods of `Foo`; it only repeated them. |
| **Prototype vs definition** | A C++ header declaration and its `.cpp` definition (`void Engine::start()` is the method `start` of `Engine`) are one function: calls link to the definition, and `graph` shows one match, not two. |
| **JNI** | A Kotlin `external fun` / Java `native` method gets an edge to its C/C++ implementation `Java_<package>_<Class>_<method>`. A C++ `GetMethodID(cls, "onDevice", ...)` links back to the one Kotlin/Java method of that name. So both directions of a native bridge show in `graph`. |

Parser accuracy fixes that fed into this:

- **Nested types.** Kotlin, Java, C#, Swift and C/C++ used to remember only the *last* declared type, so methods after a
  nested class were attributed to it. A shared `TypeScopes` tracker follows brace nesting (strings, char literals and
  Allman-style braces included); declarations with no body (`data class P(val a: Int)`, `class Fwd;`) close immediately.
- **Stale scope** in the Rust and TypeScript/JavaScript parsers (free functions attached to a preceding type; call
  statements parsed as methods) is fixed; `impl Foo` / `impl Trait for Foo` map onto `Foo`.
- **C++ constructors and destructors.** `Foo::Foo(...)`, `Foo::~Foo()` and in-class `Foo(...)` / `~Foo()` have no return
  type, so they used to be missed, and the member-initializer line after the colon (`: mRing(cap) {`, `, mFoo(x)`) was
  read as a function named after the member, which then took the constructor's body (and its calls). Initializer lines
  are no longer functions; the constructor is a symbol (`AudioEngine.AudioEngine`) whose body starts after the list
  (brace initializers `mA{a}` do not end it early). A call `new Foo(...)` links to the constructor, not the class.
  Statement lines (`return foo(x);`, `else bar();`) are not function declarations either.
- Bumping `PARSER_VERSION` rebuilds stale indexes automatically.

### Limits

Linking is by **name** plus the receiver's declared type where the file shows it: it is for orientation, not a
compiler-grade call graph. It cannot see reflection, dynamic dispatch through interfaces with several implementations,
a receiver whose type is not declared in the same file (a chained `a.b.c()`, an inferred `val x = factory()`), or
bindings through strings other than the JNI cases above (JNI `RegisterNatives`, FFI). A native function registered
with `RegisterNatives` instead of the `Java_...` name has no link to its Kotlin declaration.

## A file as the argument

`mysid graph UsbDirectOutput.kt` (a file name, as `read` and `overview` accept) gives the file's place in the call
graph instead of one symbol:

```
UsbDirectOutput.kt  (Kotlin, 523 lines, 53 symbols)
DEFINES: UsbDirectOutput:41 (52 methods)   (outline: mysid overview UsbDirectOutput.kt)
CALLS OUT (to 5 files):
  UsbFormatNegotiator.kt: UsbFormatNegotiator.setupUsbForFormat, ...   (from UsbDirectOutput.setupUsbForFormat, ...)
  AudioEngineBridge.cpp: Java_com_acme_UsbBridge_nativeOpen (JNI)   (from UsbBridge.nativeOpen)
CALLED BY (from 3 files):
  AudioOutputFactory.kt: AudioOutputFactory.createUsbEngine -> UsbDirectOutput
calls within the file: 14
```

Two files with the same name: the answer lists them; repeat with a folder (`ui/Player.kt`).

## The map (no symbol)

`mysid graph` rolls files up by directory and skips single-child chains (`src/` → `src/Pages/`):

```
frontend - 52 files, 213 symbols
  src/  48 files, 213 symbols
next: mysid graph <dir|file> to drill down, mysid graph <Symbol> for callers/callees
```

Naming a directory lists its files with up to 5 top-level symbol names each; naming a file lists its symbols.
Output is capped at 40 lines. The map is the one view that still reads the whole index.

## Performance

Measured on an Android project (301 indexed files, 16 cores), warm index:

| Phase | Before | Now |
|---|---|---|
| Query + render | n/a (everything was loaded first) | 0.2–1 ms |
| File-tree walk for change detection | 78–104 ms | ~13 ms (parallel; only indexable files are stat-ed) |
| `mysid graph X`, end to end | 96 ms | **24 ms** (process start alone is ~14 ms) |

`MYSID_TIMING=1 mysid graph X` prints where the time goes (to stderr).

## Source map

| File | Role |
|---|---|
| `src/tools/graph/mod.rs` | entry point, the lookup flow above |
| `src/tools/graph/query.rs` | name resolution, neighbor/BFS helpers |
| `src/tools/graph/view/{card,flow,map}.rs` | the three renderers and their line budgets |
| `src/tools/graph/build.rs` | turns index rows into the in-memory graph the views read |
| `src/index/` | data behind it: [Indexer.md](Indexer.md) |
