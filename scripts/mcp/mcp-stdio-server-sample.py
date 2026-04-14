#!/usr/bin/env python3
# Copyright (c) 2026 nilbox
"""
Sample MCP Server using stdio transport (stdin/stdout).

Usage:
    python3 mcp-stdio-server-sample.py

    # Test with mcp-stdio-proxy config:
    # /etc/nilbox/mcp-servers.json
    # {
    #   "servers": [{
    #     "name": "sample",
    #     "port": 9001,
    #     "command": ["python3", "/path/to/mcp-stdio-server-sample.py"]
    #   }]
    # }

    # Test standalone (pipe JSON-RPC via stdin):
    echo '{"jsonrpc":"2.0","method":"initialize","id":1,"params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"test"}}}' | python3 mcp-stdio-server-sample.py

Provides tools:
    - echo: Echo back the input
    - read_file: Read a file from the filesystem
    - list_dir: List directory contents
    - run_command: Run a shell command
"""

import json
import os
import subprocess
import sys


SERVER_INFO = {
    "name": "nilbox-sample-mcp-stdio",
    "version": "1.0.0",
}

TOOLS = [
    {
        "name": "echo",
        "description": "Echo back the input message",
        "inputSchema": {
            "type": "object",
            "properties": {
                "message": {
                    "type": "string",
                    "description": "Message to echo",
                }
            },
            "required": ["message"],
        },
    },
    {
        "name": "read_file",
        "description": "Read contents of a file",
        "inputSchema": {
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "File path to read",
                }
            },
            "required": ["path"],
        },
    },
    {
        "name": "list_dir",
        "description": "List directory contents",
        "inputSchema": {
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Directory path to list",
                }
            },
            "required": ["path"],
        },
    },
    {
        "name": "run_command",
        "description": "Run a shell command and return output",
        "inputSchema": {
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "Shell command to execute",
                }
            },
            "required": ["command"],
        },
    },
]


def handle_tool_call(name, arguments):
    try:
        if name == "echo":
            return {
                "content": [
                    {"type": "text", "text": arguments.get("message", "")}
                ]
            }

        elif name == "read_file":
            path = arguments.get("path", "")
            if not os.path.exists(path):
                return {
                    "content": [
                        {"type": "text", "text": f"Error: File not found: {path}"}
                    ],
                    "isError": True,
                }
            with open(path, "r") as f:
                content = f.read()
            return {"content": [{"type": "text", "text": content}]}

        elif name == "list_dir":
            path = arguments.get("path", ".")
            if not os.path.isdir(path):
                return {
                    "content": [
                        {"type": "text", "text": f"Error: Not a directory: {path}"}
                    ],
                    "isError": True,
                }
            entries = os.listdir(path)
            result = []
            for entry in sorted(entries):
                full_path = os.path.join(path, entry)
                if os.path.isdir(full_path):
                    result.append(f"[DIR]  {entry}/")
                else:
                    size = os.path.getsize(full_path)
                    result.append(f"[FILE] {entry} ({size} bytes)")
            return {
                "content": [
                    {"type": "text", "text": "\n".join(result) if result else "(empty)"}
                ]
            }

        elif name == "run_command":
            command = arguments.get("command", "")
            result = subprocess.run(
                command,
                shell=True,
                capture_output=True,
                text=True,
                timeout=30,
            )
            output = result.stdout
            if result.stderr:
                output += f"\n[stderr]\n{result.stderr}"
            if result.returncode != 0:
                output += f"\n[exit code: {result.returncode}]"
            return {"content": [{"type": "text", "text": output}]}

        else:
            return {
                "content": [{"type": "text", "text": f"Unknown tool: {name}"}],
                "isError": True,
            }

    except Exception as e:
        return {
            "content": [{"type": "text", "text": f"Error: {e}"}],
            "isError": True,
        }


def handle_request(request):
    method = request.get("method", "")
    req_id = request.get("id")
    params = request.get("params", {})

    if method == "initialize":
        return {
            "jsonrpc": "2.0",
            "id": req_id,
            "result": {
                "protocolVersion": "2024-11-05",
                "capabilities": {"tools": {}},
                "serverInfo": SERVER_INFO,
            },
        }

    if method == "notifications/initialized":
        return None

    if method == "tools/list":
        return {
            "jsonrpc": "2.0",
            "id": req_id,
            "result": {"tools": TOOLS},
        }

    if method == "tools/call":
        tool_name = params.get("name", "")
        arguments = params.get("arguments", {})
        result = handle_tool_call(tool_name, arguments)
        return {"jsonrpc": "2.0", "id": req_id, "result": result}

    if method == "ping":
        return {"jsonrpc": "2.0", "id": req_id, "result": {}}

    return {
        "jsonrpc": "2.0",
        "id": req_id,
        "error": {"code": -32601, "message": f"Method not found: {method}"},
    }


def main():
    log = lambda msg: print(f"[MCP-stdio] {msg}", file=sys.stderr)
    log(f"Server started: {SERVER_INFO['name']} v{SERVER_INFO['version']}")
    log(f"Tools: {', '.join(t['name'] for t in TOOLS)}")

    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue

        try:
            request = json.loads(line)
            log(f"<- {request.get('method', '?')}")
        except json.JSONDecodeError as e:
            log(f"JSON parse error: {e}")
            continue

        response = handle_request(request)
        if response is not None:
            out = json.dumps(response) + "\n"
            sys.stdout.write(out)
            sys.stdout.flush()
            log(f"-> response (id={response.get('id')})")

    log("stdin closed, exiting")


if __name__ == "__main__":
    main()
