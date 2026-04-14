# Copyright (c) 2026 nilbox
"""
Sample MCP Server listening on TCP port 9001

Usage (in VM):
    python3 mcp-server-sample.py
    python3 mcp-server-sample.py --port 9002

Provides sample tools:
    - echo: Echo back the input
    - read_file: Read a file from the VM filesystem
    - list_dir: List directory contents
    - run_command: Run a shell command
"""

import argparse
import json
import socket
import sys
import os
from typing import Any

DEFAULT_PORT = 9001

# MCP Server metadata
SERVER_INFO = {
    "name": "nilbox-sample-mcp",
    "version": "1.0.0"
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
                    "description": "Message to echo"
                }
            },
            "required": ["message"]
        }
    },
    {
        "name": "read_file",
        "description": "Read contents of a file",
        "inputSchema": {
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "File path to read"
                }
            },
            "required": ["path"]
        }
    },
    {
        "name": "list_dir",
        "description": "List directory contents",
        "inputSchema": {
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Directory path to list"
                }
            },
            "required": ["path"]
        }
    },
    {
        "name": "run_command",
        "description": "Run a shell command and return output",
        "inputSchema": {
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "Shell command to execute"
                }
            },
            "required": ["command"]
        }
    }
]


def handle_tool_call(name: str, arguments: dict) -> dict:
    """Execute a tool and return the result."""
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
                    "isError": True
                }
            with open(path, "r") as f:
                content = f.read()
            return {
                "content": [
                    {"type": "text", "text": content}
                ]
            }

        elif name == "list_dir":
            path = arguments.get("path", ".")
            if not os.path.isdir(path):
                return {
                    "content": [
                        {"type": "text", "text": f"Error: Not a directory: {path}"}
                    ],
                    "isError": True
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
            import subprocess
            command = arguments.get("command", "")
            result = subprocess.run(
                command,
                shell=True,
                capture_output=True,
                text=True,
                timeout=30
            )
            output = result.stdout
            if result.stderr:
                output += f"\n[stderr]\n{result.stderr}"
            if result.returncode != 0:
                output += f"\n[exit code: {result.returncode}]"
            return {
                "content": [
                    {"type": "text", "text": output}
                ]
            }

        else:
            return {
                "content": [
                    {"type": "text", "text": f"Unknown tool: {name}"}
                ],
                "isError": True
            }

    except Exception as e:
        return {
            "content": [
                {"type": "text", "text": f"Error: {str(e)}"}
            ],
            "isError": True
        }


def handle_request(request: dict) -> dict:
    """Handle a JSON-RPC request and return response."""
    method = request.get("method", "")
    req_id = request.get("id")
    params = request.get("params", {})

    # Initialize
    if method == "initialize":
        return {
            "jsonrpc": "2.0",
            "id": req_id,
            "result": {
                "protocolVersion": "2024-11-05",
                "capabilities": {
                    "tools": {}
                },
                "serverInfo": SERVER_INFO
            }
        }

    # Initialized notification (no response needed)
    if method == "notifications/initialized":
        return None

    # List tools
    if method == "tools/list":
        return {
            "jsonrpc": "2.0",
            "id": req_id,
            "result": {
                "tools": TOOLS
            }
        }

    # Call tool
    if method == "tools/call":
        tool_name = params.get("name", "")
        arguments = params.get("arguments", {})
        result = handle_tool_call(tool_name, arguments)
        return {
            "jsonrpc": "2.0",
            "id": req_id,
            "result": result
        }

    # Ping
    if method == "ping":
        return {
            "jsonrpc": "2.0",
            "id": req_id,
            "result": {}
        }

    # Unknown method
    return {
        "jsonrpc": "2.0",
        "id": req_id,
        "error": {
            "code": -32601,
            "message": f"Method not found: {method}"
        }
    }


def handle_connection(conn: socket.socket, addr):
    """Handle a single MCP client connection."""
    print(f"[MCP] Connection from {addr}", file=sys.stderr)

    buffer = b""

    try:
        while True:
            data = conn.recv(4096)
            if not data:
                break

            buffer += data

            # Process complete lines (JSON-RPC messages are newline-delimited)
            while b"\n" in buffer:
                line, buffer = buffer.split(b"\n", 1)
                if not line.strip():
                    continue

                try:
                    request = json.loads(line.decode("utf-8"))
                    print(f"[MCP] <- {request.get('method', 'unknown')}", file=sys.stderr)

                    response = handle_request(request)

                    if response is not None:
                        response_bytes = json.dumps(response).encode("utf-8") + b"\n"
                        conn.sendall(response_bytes)
                        print(f"[MCP] -> response (id={response.get('id')})", file=sys.stderr)

                except json.JSONDecodeError as e:
                    print(f"[MCP] JSON parse error: {e}", file=sys.stderr)
                except Exception as e:
                    print(f"[MCP] Error handling request: {e}", file=sys.stderr)

    except Exception as e:
        print(f"[MCP] Connection error: {e}", file=sys.stderr)
    finally:
        conn.close()
        print(f"[MCP] Connection closed", file=sys.stderr)


def main():
    parser = argparse.ArgumentParser(description="MCP Server on TCP")
    parser.add_argument("--port", type=int, default=DEFAULT_PORT, help=f"TCP port (default: {DEFAULT_PORT})")
    parser.add_argument("--host", default="0.0.0.0", help="Bind address (default: 0.0.0.0)")
    args = parser.parse_args()

    sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    sock.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    sock.bind((args.host, args.port))
    sock.listen(5)

    print(f"[MCP] Listening on {args.host}:{args.port}", file=sys.stderr)
    print(f"[MCP] Server: {SERVER_INFO['name']} v{SERVER_INFO['version']}", file=sys.stderr)
    print(f"[MCP] Tools: {', '.join(t['name'] for t in TOOLS)}", file=sys.stderr)

    while True:
        conn, addr = sock.accept()
        handle_connection(conn, addr)


if __name__ == "__main__":
    main()
