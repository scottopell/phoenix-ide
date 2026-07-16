#!/usr/bin/env python3
"""Deterministic Phoenix-like runtime for deployment integration tests."""

import argparse
import json
import os
import signal
import socket
import socketserver
import sys
import time
from http.server import BaseHTTPRequestHandler, HTTPServer

DEFAULT_VERSION = "2.0.0"
DEFAULT_GIT_SHA = "bbbbbbbbbbbb"


class Handler(BaseHTTPRequestHandler):
    version = DEFAULT_VERSION
    git_sha = DEFAULT_GIT_SHA

    def do_GET(self):
        if self.path == "/api/version":
            body = json.dumps({"version": self.version, "git_sha": self.git_sha}).encode()
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
        elif self.path == "/version":
            body = f"phoenix-ide {self.version}\n".encode()
            self.send_response(200)
            self.send_header("Content-Type", "text/plain")
        else:
            body = b"not found\n"
            self.send_response(404)
            self.send_header("Content-Type", "text/plain")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, format, *args):
        pass


class ActivatedHTTPServer(HTTPServer):
    def __init__(self, inherited_socket):
        super().__init__(("", 0), Handler, bind_and_activate=False)
        self.socket = inherited_socket
        self.server_address = inherited_socket.getsockname()

    def server_bind(self):
        pass

    def server_activate(self):
        pass


class ReusableHTTPServer(HTTPServer):
    allow_reuse_address = True

    def server_bind(self):
        socketserver.TCPServer.server_bind(self)
        self.server_name = self.server_address[0]
        self.server_port = self.server_address[1]


def inherited_systemd_socket():
    if os.environ.get("LISTEN_PID") != str(os.getpid()):
        raise RuntimeError("LISTEN_PID does not name this process")
    if os.environ.get("LISTEN_FDS") != "1":
        raise RuntimeError("fixture requires exactly one systemd socket")
    inherited = socket.fromfd(3, socket.AF_INET6, socket.SOCK_STREAM)
    os.set_inheritable(inherited.fileno(), False)
    return inherited


def parse_args():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--build-identity", action="store_true")
    parser.add_argument("--version", default=os.environ.get("FIXTURE_VERSION", DEFAULT_VERSION))
    parser.add_argument("--git-sha", default=os.environ.get("FIXTURE_GIT_SHA", DEFAULT_GIT_SHA))
    parser.add_argument("--report-version")
    parser.add_argument("--report-git-sha")
    parser.add_argument("--host", default="127.0.0.1")
    parser.add_argument("--port", type=int, default=0)
    parser.add_argument("--socket-activation", action="store_true")
    parser.add_argument("--startup-delay", type=float, default=0.0)
    parser.add_argument("--crash", action="store_true")
    parser.add_argument("--ready-file")
    return parser.parse_args()


def main():
    args = parse_args()
    if args.build_identity:
        print(json.dumps({"version": args.version, "git_sha": args.git_sha}, sort_keys=True))
        return 0
    if args.crash:
        return 23
    if args.startup_delay < 0:
        raise SystemExit("--startup-delay must be non-negative")
    time.sleep(args.startup_delay)

    Handler.version = args.report_version or args.version
    Handler.git_sha = args.report_git_sha or args.git_sha
    if args.socket_activation:
        server = ActivatedHTTPServer(inherited_systemd_socket())
    else:
        server = ReusableHTTPServer((args.host, args.port), Handler)

    stopping = False

    def stop(_signum, _frame):
        nonlocal stopping
        stopping = True

    signal.signal(signal.SIGTERM, stop)
    signal.signal(signal.SIGINT, stop)
    server.timeout = 0.1
    if args.ready_file:
        with open(args.ready_file, "w", encoding="utf-8") as stream:
            stream.write(f"{server.server_address[1]}\n")
            stream.flush()
            os.fsync(stream.fileno())
    else:
        print(server.server_address[1], flush=True)
    while not stopping:
        server.handle_request()
    server.server_close()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
