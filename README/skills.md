# Skills: workflow guides for LLMs

A skill is a short markdown guide that tells a model *how to work* with mysid: the order of commands, the rules that
save tokens, and a few exact examples. They live in `rust-mcp/skills/*.md` and are compiled into the binary, so they
travel with every install and any LLM client can read them.

| Skill | Covers |
|---|---|
| `base` | running from the project folder (`set` is optional), file names instead of paths, `list` → `overview` → `read`, `search`, `graph`, edits (`patch`, `apply`, `write`), `check` / `build`, checkpoint → `diff` → `commit` |
| `android` | staged `build` / `status` / `stop`, `release`, `devices` / `install`, `debug` keyword capture (20 s window) and `--raw`, JNI caveat |
| `react` | `new react`, component and hook `graph` (JSX counts as a call), `search --types ts,tsx`, `build` errors as `file(line,col): error TS...`, lint and tests through `exec`, no dev server via `exec` |

Stack skills assume `base`. Planned once their tooling exists: `springboot` (needs Maven build support) and
`fullstack` (needs a cross-language endpoint search).

## Commands

```
mysid skill                    list them
mysid skill android            print one (any client: Codex, Gemini, Cursor, ...)
mysid skill install            write all as Claude Code skills: <project>/.claude/skills/mysid-<name>/SKILL.md
mysid skill install android    that one, plus base
mysid skill install --global   into ~/.claude/skills instead
```

The MCP server exposes the same thing as the `skill` tool. Claude Code reads skills at session start, so restart it
after installing. Re-running `install` overwrites the files (they are mysid's own) and reports `updated`.

## Writing or changing one

Edit the file in `rust-mcp/skills/`, keep the front matter (`name: mysid-<name>`, a `description` that says *when to
use it*), keep it around 40-60 lines, and only describe commands that exist. A test checks the front matter and that each
file's name matches its entry, and `mysid skill <name>` prints the body without the front matter.
