# Tree-sitter proof of concept (Kotlin)

Question: what changes when the Kotlin parser reads a real syntax tree (tree-sitter) instead of line regexes?

This is an experiment behind the cargo feature `treesitter`. The normal build, the release workflow and the installed `mysid` do not
contain it and are unchanged.

## Run it

```
cd rust-mcp
set CARGO_TARGET_DIR=target-ts          (a separate folder: the normal target/ and the installed mysid stay as they are)
cargo build --release --features treesitter
target-ts\release\mysid.exe doctor tscompare <folder>        both parsers over every .kt file; lists where they disagree
target-ts\release\mysid.exe doctor tsdump Foo.kt --depth 6   the syntax tree of one file (node kinds, fields, lines)
cargo test --features treesitter kotlin_ts                   the 5 parser tests
```

## What is in it

- `parsers/kotlin_ts.rs`: `KotlinTsParser`, same output as the regex `KotlinParser` (types, and methods with owner, signature,
  parameters, return type), so the index above it would not change. Dependencies: `tree-sitter 0.27`, `tree-sitter-kotlin-ng 1.1`.
- `tools/doctor/tsparse.rs`: the `tscompare` and `tsdump` commands.
- Safety net: a file in which the grammar reports a syntax error (an `ERROR` or `MISSING` node) is read by the regex parser instead,
  so the result is never worse than before.

## Results on real code (Kotlin Android app, 187 files, `Hypersonus-mobile/v63/app/src`)

| | regex | tree-sitter |
|---|---|---|
| types | 271 | 272 |
| methods | 1833 | 1829 |
| parse time, all files | 249 ms | 435 ms |
| files with identical symbols | | 183 of 187 |
| files the grammar could not read (fallback used) | | 0 |
| same method, different parameters or return type | | 284 |

Binary size with the Kotlin grammar: 12.0 MB (normal build: 9.1 MB).

Two other copies of the same app (`working`, `v62-stable`) give the same picture (4 differing files of 187). The TV app (32 files) has 1 differing file. No file anywhere was unreadable by the grammar.

### Every difference, and who was right

- **284 signatures: the tree is right.** The regex parser mishandles a function with an expression body (`fun f(): Int = g()`): it reports
  `(): Int = g()` as the parameters and no return type, or `Boolean = a == b` as the return type. The tree reports `()` and `Int`. This matters
  because the call-graph resolver uses return types to follow chained calls (`a.getB().run()`).
- **19 methods: the tree is right.** In `DspManager.kt` (14) and `SessionPersistence.kt` (5) the regex parser lost the enclosing class
  and gave the methods no owner; the tree gives them the right one. A wrong owner means wrong call-graph links.
- **1 type and 1 method: the tree is right.** `inner class MusicBinder` is invisible to the regex parser (`inner` is not in its modifier
  list), so `getService` was credited to `MusicService`.
- **4 methods: the tree is right.** `VolumeLaw.kt` has a block comment of commented-out code; the regex parser reports 4 methods from it.
- **1 method: the grammar was wrong, silently.** After two stacked annotations, `@Preview(...) @Composable private fun X() { ... }` was
  read as an expression (`private`, `fun` and a call as identifiers) with no error node, which dropped the declaration. A recovery
  for that exact shape was added; the comparison then showed no missing method. Small tests cannot reproduce it (it depends on the code
  before it), so the only way to find such cases is to compare against another parser, as `tscompare` does.
- **A second grammar quirk, found with a test sample:** a body-less member of an interface nested in a class, followed by a one-line `object`
  with an expression-body function, makes the grammar lose the enclosing class (it reports an `ERROR` node). The fallback handles it.

## What this says

- Tree-sitter fixes real defects of the regex parser on real code: wrong owners, wrong signatures, commented-out code, missed
  modifiers. In the 309 differences found (284 signatures, 19 owners, 2 inner-class symbols, 4 commented-out methods), the tree was right every time except one silent grammar miss, which is now recovered.
- It does not make the grammar perfect. The grammar can fail with an error node (handled by falling back) or, rarely, silently
  (1 method in 1,833 here). Comparing against the old parser on real code is the way to catch that.
- Cost: about 3 MB per big grammar, parsing about 1.7 times slower (about 2 ms per file; parsing all 187 files takes under half a second).
- It is syntax only. Types are still inferred by the resolver; the tree only gives it better inputs (exact parameter and return types,
  exact owners, and, next, exact call sites and receivers).

## Not done yet

- Calls: the index still finds calls with a regex over the method body (`extract.rs`). The tree has `call_expression` nodes with their receiver
  expressions; using them is the larger win for the call graph.
- Base classes and interfaces (`bases_of`), `GetMethodID`-style strings, and the type environment (`val x: Foo`, parameters) from the tree.
- The other languages (Java, C/C++, TS/TSX, Rust, Python, Go, C#, Swift, Dart), each compared on real code first.
- Switching the default build: the feature is off until the comparison on more code and the call extraction justify it.
