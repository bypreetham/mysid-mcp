# Installing mysid

mysid is one prebuilt program (`mysid` / `mysid.exe`). No Rust, no clone, no build is needed. The latest build is the GitHub
release tagged `latest`: https://github.com/bypreetham/mysid-mcp/releases/tag/latest

This file is written for people and for AI agents: every step has the exact command, what a good result looks like, and what to
do when it fails.

## 1. Check whether it is already installed

```
mysid help
```
Good: the first line is `mysid: code intelligence for LLM agents. Eleven commands cover most work:` (the number of commands may
change; the words "code intelligence for LLM agents" stay). If you see that, skip to step 4.

## 2. Install

| System | Command | What it does |
|---|---|---|
| Windows (PowerShell) | `irm https://raw.githubusercontent.com/bypreetham/mysid-mcp/main/install.ps1 \| iex` | downloads `mysid.exe`, checks that it runs, copies it to `%LOCALAPPDATA%\mysid\bin`, adds that folder to the user PATH |
| macOS, Linux | `curl -fsSL https://raw.githubusercontent.com/bypreetham/mysid-mcp/main/install.sh \| sh` | picks the build for the system, checks its SHA-256, checks that it runs, copies it to `~/.local/bin`, adds that folder to the shell profile when it is not on PATH |

Builds exist for Windows x64, Linux x64 and arm64, macOS Intel and Apple Silicon. Both scripts also register the MCP server
(step 3). Nothing is replaced unless the downloaded file runs and prints the banner above.

Options (Windows): the script takes switches when started as `& ([scriptblock]::Create((irm <url>))) -NoPath -NoMcp`, or the
environment variables `MYSID_NO_PATH=1`, `MYSID_NO_MCP=1` before `| iex`. macOS/Linux: `MYSID_NO_PATH=1`, `MYSID_NO_MCP=1`,
`MYSID_INSTALL_DIR=<folder>`, `MYSID_BASE_URL=<mirror>`.

## 3. Register the MCP server (the installers do this; by hand if they could not)

| Client | What to do |
|---|---|
| Claude Code | `claude mcp add --scope user mysid -- mysid`. Without the `claude` command, the installers edit `~/.claude.json` (only if it exists, with a backup `~/.claude.json.mysid-backup`) |
| Claude Desktop | the installers add `mysid` to `claude_desktop_config.json`; restart the app |
| Antigravity / Gemini | the installers add `mysid` to `~/.gemini/config/mcp_config.json` |
| Any other client | add `{ "mcpServers": { "mysid": { "command": "mysid", "args": [] } } }` to its MCP config |

The installers never touch other MCP servers in those files.

## 4. Verify (in a NEW terminal: an old one keeps the old PATH)

```
mysid help
```
Then restart the client (Claude Code, Claude Desktop, the editor) so it loads the MCP server, and in a project folder run
`mysid set .`. Over MCP the client should list the tools (38 at the time of writing).

## When something fails

| Symptom | Cause | Fix |
|---|---|---|
| `mysid: command not found` / "not recognized" right after installing | the terminal was open before the install, so it has the old PATH | open a new terminal; or add `%LOCALAPPDATA%\mysid\bin` (Windows) or `~/.local/bin` (macOS/Linux) to PATH |
| `ERROR: could not download …/mysid-linux-x64` | no build for this system, or no network | check https://github.com/bypreetham/mysid-mcp/releases/tag/latest; otherwise build from source (below) |
| `checksum mismatch` | corrupted or incomplete download | run the installer again; nothing was installed |
| `the downloaded file did not behave like mysid` | the download was not the program (a proxy page, an error) | run again; check the network or proxy |
| Windows: `Could not update …mysid.exe (file locked)` | a running copy (an MCP client) holds the file | the installer moves the running copy aside; if it still fails, close the client and run it again |
| macOS: "cannot be opened because the developer cannot be verified" | the file was downloaded by a browser (quarantined) | install with the `curl` command above (it is not quarantined), or `xattr -d com.apple.quarantine <file>` |
| Windows: SmartScreen warning | the program is not code-signed | choose "Run anyway", or install with the PowerShell command (a file fetched that way is normally not marked as downloaded from the internet) |
| Claude Code does not list mysid | it was running during the install | restart it; check with `claude mcp list` |
| `Could not edit ~/.claude.json` | the file is large or not valid JSON | run `claude mcp add --scope user mysid -- mysid` once the `claude` command works |

## Build from source (only when no prebuilt file exists)

```
git clone https://github.com/bypreetham/mysid-mcp.git
cd mysid-mcp/rust-mcp
cargo build --release
```
Copy `target/release/mysid` (`mysid.exe` on Windows) to a folder on PATH, then do step 3.

## Update and remove

- Update: run the install command again (it replaces the running copy safely) and restart the client.
- Remove: delete the file (`%LOCALAPPDATA%\mysid\bin\mysid.exe` or `~/.local/bin/mysid`), remove that folder from PATH if you
  want, and remove the `mysid` entry from the MCP configs in step 3 (`claude mcp remove mysid` for Claude Code).
