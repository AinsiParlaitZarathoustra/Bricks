"""A local HTTP server for the shell tests, without any name resolution.

`http.server.HTTPServer.server_bind()` calls `socket.getfqdn(host)`, a
reverse DNS lookup that can hang for a long time on some machines (even for
127.0.0.1). This server binds through `socketserver.TCPServer.server_bind()`
and fills `server_name` / `server_port` from the bound address instead.

It listens on 127.0.0.1 with a port chosen by the system, announces it on
stdout (unbuffered) in http.server's own words — "Serving HTTP on 127.0.0.1
port N (...)" — and serves the current directory until it is stopped.

`--forbid-getfqdn` replaces `socket.getfqdn` with a function that fails at
once: the server must start, answer and stop without calling it.
Test fixture only; nothing in Bricks depends on it.
"""

import http.server
import socket
import socketserver
import sys


class Server(http.server.ThreadingHTTPServer):
    def server_bind(self):
        socketserver.TCPServer.server_bind(self)
        host, port = self.server_address[:2]
        self.server_name = host
        self.server_port = port


def main():
    if "--forbid-getfqdn" in sys.argv[1:]:
        def forbidden(*_args, **_kwargs):
            raise RuntimeError("socket.getfqdn must not be called")

        socket.getfqdn = forbidden
    with Server(("127.0.0.1", 0), http.server.SimpleHTTPRequestHandler) as httpd:
        host, port = httpd.server_address[:2]
        print(
            f"Serving HTTP on {host} port {port} (http://{host}:{port}/) ...",
            flush=True,
        )
        httpd.serve_forever()


if __name__ == "__main__":
    main()
