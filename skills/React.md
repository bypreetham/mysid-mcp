# React / Vite / TypeScript MCP Cheatsheet

Use the updated `mysid` CLI for highly-optimized, token-efficient MCP operations.

## 1. Setup Workspace (Run Once Per Project)

Sets the workspace to the current directory:
```bash
mysid set_workspace .
```

## 2. Search / Read Code

Read exact symbols (definitions + callers) or file slices:
```bash
mysid read App
mysid read filename#L12-L23
```

Fast regex/text grep with context:
```bash
mysid search "useAuth"
```

## 3. Build & Type-Check (Token-Optimized)

Run the build process (runs `tsc` then `npm run build`):
```bash
mysid build
```
> **Note**: This command is aggressively token-optimized for LLMs. If the build succeeds, it only returns `"success"`. If it fails, it automatically strips the compilation noise and returns **only** the relevant `tsc` or Vite error lines.

## 4. Code Modification

Global search-and-replace across files:
```bash
mysid replace "oldValue" "newValue"
```

Atomic multi-file batch patching or surgical single-file replacements:
```bash
mysid patch src/App.tsx "old" "new"
mysid patch_batch --json patch.json
```

## 5. Architecture & Dependency Flow

Generate a graph for a specific symbol or view architecture flows:
```bash
mysid graph "Symbol"
mysid graph --mode overview
```
