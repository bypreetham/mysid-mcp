# React / Vite / TypeScript MCP Cheatsheet

Use `rust_mcp_client.py` directly. Do NOT write manual JSON files.

```bash
# Python interpreter & client
PYTHON="E:\Orchestra\venv\Scripts\python.exe"
CLIENT="E:\Orchestra\rust_mcp_client.py"
```

## 1. Setup Workspace (Run Once Per Project)

```bash
$PYTHON $CLIENT set_workspace "C:\path\to\react_project"
$PYTHON $CLIENT get_workspace
```

## 2. Inspect Components & Symbols

Jump directly to component/hook/type definitions and callers:

```bash
# Inspect React component or hook (definition + callers)
$PYTHON $CLIENT inspect_symbol App
$PYTHON $CLIENT inspect_symbol ChatPage
$PYTHON $CLIENT inspect_symbol useInference

# List all exported symbols in a file
$PYTHON $CLIENT symbols src/pages/ChatPage.tsx
$PYTHON $CLIENT symbols src/types/index.ts
```

## 3. Search Codebase

Automatically filters out `node_modules/`, `.next/`, `dist/`, and documentation:

```bash
$PYTHON $CLIENT search "AutoModelForCausalLM"
$PYTHON $CLIENT search "useInference"
$PYTHON $CLIENT search "TextStreamer" --ctx 2
```

## 4. Read / Peek Code

Strictly reads the requested range without context bloat:

```bash
# Read specific lines (format: peek <file> <start_line> <end_line>)
$PYTHON $CLIENT peek src/pages/ChatPage.tsx 1 60
$PYTHON $CLIENT peek src/workers/inference.worker.ts 1 80
$PYTHON $CLIENT peek vite.config.ts 1 30
```

## 5. File System Operations

```bash
# List directory contents
$PYTHON $CLIENT list_dir src
$PYTHON $CLIENT list_dir src/components
```

## 6. Architecture & Dependency Flow

```bash
# Component overview / callgraph
$PYTHON $CLIENT graph --mode overview
$PYTHON $CLIENT graph --mode flow --query ChatPage --depth 3
```
