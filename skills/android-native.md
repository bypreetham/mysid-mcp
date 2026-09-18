# Android & mysid Tooling Reference

### Core Tools (7)
1. `mysid set_workspace <path>`: Set active workspace root (returns L1 directory tree).
2. `mysid read <symbol | fileName#Lstart-end...>`: AST symbol lookup or batch file slice inspection (bare filenames are enough).
3. `mysid search <query> [--types kt,cpp]`: High-speed regex/string discovery respecting `.gitignore`.
4. `mysid replace <old> <new> [--types ...] [--dry-run]`: Global workspace or single-file search-and-replace refactoring.
5. `mysid build [clean]` | `install` | `release`: Gradle compile with token-filtered errors, ADB deploy, or AAB release.
6. `mysid android [devices | connect_wifi | install | launch_app | logcat | crashes | clear_logcat | tap | type | keyevent]`: Android device & ADB utilities.
7. `mysid help [command]`: Display full command overview or detailed flag syntax and usage examples.

### Essential Directives
1. **Tool Invocation**: Always invoke `mysid` directly from any directory (sub-20ms native Rust, zero path prefixes).
2. **Zero Single-Turn Peeking**: Bundle ALL file slices into one turn: `mysid read "FileA.kt#L10-30" "FileB.cpp#L40-80"`.
3. **Bare File Paths**: Do not write full directory paths for reading; provide bare names: `mysid read "FileName.kt#L1-40"`.
