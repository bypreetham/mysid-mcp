# React / Vite / TypeScript MCP Cheatsheet

Use the updated `mysid` CLI for MCP operations.

## 1. Setup Workspace (Run Once Per Project)

Sets the workspace to the current directory:
```bash
mysid set_workspace .
```

## 2. Search / Read Code

Search or read specific lines from a file:
```bash
mysid search filename#L12-L23
```

## 3. Build

Run the build process:
```bash
mysid build
```

## 4. Architecture & Dependency Flow

Generate a graph for a specific symbol:
```bash
mysid graph "Symbol"
```
