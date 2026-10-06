#!/usr/bin/env python3
"""A local, scripted model server for demonstrating `bricks` without any key.

Speaks the `chat_completions` protocol (streamed, SSE) on 127.0.0.1:8765.
Standard library only. Replies in turn:
  1. a tool call `Glob` on `*.md` (read-only: no approval needed),
  2. a short answer,
  3. then "ok" forever.
A prompt containing "write" gets a `Write` tool call instead (it needs an
approval under the default policy), to demonstrate approvals and exit
code 3 in `--non-interactive` mode.

    python3 scripts/demo_model.py &
    BRICKS_HOME=$PWD/.demo-home bricks --providers scripts/demo_providers.toml run --json "Find the README"
"""

import json
import http.server

STEP = {"n": 0}


def sse(chunks):
    out = ""
    for c in chunks:
        out += "data: " + json.dumps(c) + "\n\n"
    return out + "data: [DONE]\n\n"


def text(t):
    return sse([
        {"choices": [{"index": 0, "delta": {"role": "assistant", "content": t}}]},
        {"choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}],
         "usage": {"prompt_tokens": 120, "completion_tokens": 12, "total_tokens": 132}},
    ])


def tool(name, args):
    return sse([
        {"choices": [{"index": 0, "delta": {"role": "assistant", "tool_calls": [
            {"index": 0, "id": "call_%d" % STEP["n"], "type": "function",
             "function": {"name": name, "arguments": json.dumps(args)}}]}}]},
        {"choices": [{"index": 0, "delta": {}, "finish_reason": "tool_calls"}],
         "usage": {"prompt_tokens": 100, "completion_tokens": 8, "total_tokens": 108}},
    ])


class Handler(http.server.BaseHTTPRequestHandler):
    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers.get("content-length", 0))) or b"{}")
        first_user = next((m.get("content") for m in body.get("messages", []) if m.get("role") == "user"), "") or ""
        if isinstance(first_user, list):
            first_user = " ".join(p.get("text", "") for p in first_user if isinstance(p, dict))
        n = STEP["n"]
        STEP["n"] += 1
        if "write" in first_user.lower() and n == 0:
            reply = tool("Write", {"file_path": "demo-output.txt", "content": "written by the demo\n"})
        elif n == 0:
            reply = tool("Glob", {"pattern": "*.md"})
        elif n == 1:
            reply = text("I listed the Markdown files. **Done** — this answer comes from the demo server.")
        else:
            reply = text("ok")
        data = reply.encode()
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def log_message(self, *args):
        pass


if __name__ == "__main__":
    http.server.ThreadingHTTPServer(("127.0.0.1", 8765), Handler).serve_forever()
