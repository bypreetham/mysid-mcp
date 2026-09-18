# Implementation Plan: `inspect_symbol` Tool for rust-mcp

This plan introduces the `inspect_symbol` tool into `rust-mcp`. The goal is to provide a zero-guesswork, instant definition + call-site extractor for C++, Java/Kotlin, Rust, and Python code.

## User Review Required
> [!NOTE]
> `inspect_symbol` will directly return:
> 1. The **exact definition block** (source file, line range, and body up to 60 lines max).
> 2. The **call sites / usages** (top 5 references with file:line and snippet).
> This replaces multi-step `search` -> `peek` roundtrips with a single compact response.

## Proposed Changes

### 1. New Tool: `src/tools/inspect_symbol.rs`
Create [`src/tools/inspect_symbol.rs`](file:///e:/Orchestra/local_tool_server/rust-mcp/src/tools/inspect_symbol.rs):
- Accept parameters:
  - `name`: Symbol name (e.g. `evaluateRoute`, `AudioRouter::evaluateRoute`, `mBitPerfect`, or `dispatchUsbPacket`).
  - `max_definition_lines`: Default 60 lines.
  - `max_callers`: Default 5.
- Logic:
  1. Scan workspace using ripgrep (`ignore::WalkBuilder`) for definitions across C/C++ (`.cpp`, `.h`), Java/Kotlin (`.java`, `.kt`), Rust (`.rs`), and Python (`.py`).
  2. Parse the opening signature and match balanced braces `{ ... }` or indentation to extract the exact function/method/class body.
  3. Scan for top callers (lines where the symbol is invoked/referenced outside its definition).
  4. Format into a clean, token-efficient summary:
     ```text
     [DEFINITION] app/src/main/cpp/engine/AudioRouter.cpp:112-148 (37 lines)
     RouteDecision AudioRouter::evaluateRoute(...) {
       ...
     }

     [CALLERS] (5 found)
     • AudioEngine.cpp:387: RouteDecision decision = mAudioRouter.evaluateRoute(...)
     • AudioEngine.cpp:601: handleRouteTransition()
     ```

### 2. Register Tool in `src/tools/mod.rs` and `src/main.rs`
- Add `pub mod inspect_symbol;` to [`src/tools/mod.rs`](file:///e:/Orchestra/local_tool_server/rust-mcp/src/tools/mod.rs).
- Add dispatch `"inspect_symbol" => tools::inspect_symbol::execute_inspect_symbol(&request.params, default_root_ref),` to [`src/main.rs`](file:///e:/Orchestra/local_tool_server/rust-mcp/src/main.rs).

## Verification Plan

### Automated / Smoke Tests
1. Compile `rust-mcp` with cargo release profile.
2. Deploy the binary to `E:\Orchestra\local_tool_server\rust-mcp\target\release\rust-mcp.exe`.
3. Query `inspect_symbol` with:
   - `AudioRouter::evaluateRoute`
   - `dispatchUsbPacket`
   - `setPureIntegerPath`
4. Confirm it returns the full definition and top call sites in under 50ms without extra line-reading tool calls.
