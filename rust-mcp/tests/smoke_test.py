"""Smoke test: rust-mcp search + peek over this repo.

Run from repo root:
  cargo build --release --manifest-path rust-mcp/Cargo.toml
  venv\\Scripts\\python rust-mcp\\smoke_test.py
"""

from __future__ import annotations

import asyncio
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT))
sys.path.insert(0, str(ROOT / "backend"))
sys.path.insert(0, str(ROOT / "local_tool_server"))

from services.agent.rust_mcp import binary_path, client  # noqa: E402


async def main() -> int:
    exe = binary_path()
    if exe is None:
        print("FAIL: rust-mcp binary not found. cargo build --release --manifest-path rust-mcp/Cargo.toml")
        return 1
    print(f"binary: {exe}")
    mcp = client()
    search_out = await mcp.search(ROOT, query="grep_workspace", file_types=["py"], max_results=5)
    print("--- search ---")
    print(search_out[:800])
    if "ERROR:" in search_out.splitlines()[0]:
        print("FAIL: search returned error")
        return 1
    peek_out = await mcp.peek(ROOT, "services/agent/tools/base.py", offset=1, limit=8)
    print("--- peek ---")
    print(peek_out[:800])
    if not peek_out.startswith("[PEEK"):
        print("FAIL: peek did not return a PEEK header")
        return 1
    print("OK")
    await mcp.close()
    return 0


if __name__ == "__main__":
    raise SystemExit(asyncio.run(main()))
