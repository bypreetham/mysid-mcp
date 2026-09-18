# mysid Tool & MCP Commands Reference

`mysid` is a standalone, ultra-fast codebase intelligence engine and MCP server.

Run directly in any terminal:
```powershell
mysid <command> [arguments...]
mysid <command> --help    # Rich on-demand documentation and examples
```

---

### Supported Commands

| Verb | Syntax | Description |
| :--- | :--- | :--- |
| `read` | `mysid read <symbol \| file#Lstart-end ...>` | Inspect AST symbol definitions + call sites, or read single/batch file slices (supports bare filenames). |
| `search` | `mysid search <query> [--types kt,cpp] [--max 25]` | Fast multithreaded regex and text search respecting `.gitignore`. |
| `replace` | `mysid replace <old> <new> [--types ...] [--dry-run]` | Workspace-wide or single-file search-and-replace refactoring. |
| `patch` | `mysid patch <file> <old_str> <new_str>` | Surgical exact-match string replacement in a single file. |
| `patch_batch` | `mysid patch_batch --json <file.json>` | Multi-file atomic batch patching. |
| `mkdir` | `mysid mkdir <path>` | Recursively create directory trees (auto-creates parent directories). |
| `file` | `mysid file <new\|rename\|delete\|list>` | Filesystem lifecycle utilities (`mysid file --help`). |
| `graph` | `mysid graph [--mode overview\|flow\|impact]` | Architectural call graphs, dependency overviews, and caller impact analysis. |
| `build` | `mysid build [clean]` | Gradle compilation with concise, token-efficient compiler error filter. |
| `release` | `mysid release` | Production Android App Bundle (`bundleRelease`) generation and R8 mapping verification. |
| `install` | `mysid install` | Deploy built APK directly to connected Android device via ADB. |
| `devices` | `mysid devices` | List connected ADB devices. |
| `android` | `mysid android <action>` | Device actions: `logcat`, `crashes`, `clear_logcat`, `tap`, `type`, `keyevent`. |
| `set_workspace`| `mysid set_workspace <path>` | Set active project workspace (auto-returns L1 directory tree). |
| `get_workspace`| `mysid get_workspace` | Print currently active workspace path. |
| `exec` | `mysid exec <command>` | Execute shell command within workspace sandbox. |
