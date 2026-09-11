#!/usr/bin/env bash
set -euo pipefail

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "SKIP: WebDAV mount verification requires macOS" >&2
  exit 0
fi
if [[ ! -x /sbin/mount_webdav ]]; then
  echo "ERROR: /sbin/mount_webdav is unavailable" >&2
  exit 1
fi

work_dir="$(mktemp -d "${TMPDIR:-/tmp}/televybackup-webdav-check.XXXXXX")"
fixture_dir="$work_dir/fixture"
mount_dir="$work_dir/mount"
mkdir -p "$fixture_dir" "$mount_dir"
printf 'snapshot browse fixture\n' > "$fixture_dir/hello.txt"

port="$(python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1]); s.close()')"
python3 - "$port" "$fixture_dir" <<'PY' &
import html
import http.server
import pathlib
import sys

port = int(sys.argv[1])
root = pathlib.Path(sys.argv[2]).resolve()

class WebDAVHandler(http.server.BaseHTTPRequestHandler):
    def do_OPTIONS(self):
        self.send_response(200)
        self.send_header("DAV", "1")
        self.send_header("Allow", "OPTIONS, PROPFIND, GET, HEAD")
        self.end_headers()

    def do_PROPFIND(self):
        path = root / self.path.lstrip("/")
        if not path.exists():
            self.send_error(404)
            return
        entries = [path] if path.is_file() else [path] + sorted(path.iterdir())
        responses = []
        for entry in entries:
            href = "/" + str(entry.relative_to(root))
            if entry.is_dir():
                href += "/"
            resource_type = "<D:collection/>" if entry.is_dir() else ""
            length = entry.stat().st_size if entry.is_file() else 0
            responses.append(
                f"<D:response><D:href>{html.escape(href)}</D:href>"
                f"<D:propstat><D:prop><D:resourcetype>{resource_type}</D:resourcetype>"
                f"<D:getcontentlength>{length}</D:getcontentlength></D:prop>"
                "<D:status>HTTP/1.1 200 OK</D:status></D:propstat></D:response>"
            )
        body = ("<?xml version=\"1.0\"?><D:multistatus xmlns:D=\"DAV:\">"
                + "".join(responses) + "</D:multistatus>").encode()
        self.send_response(207)
        self.send_header("Content-Type", "application/xml")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self):
        path = root / self.path.lstrip("/")
        if not path.is_file():
            self.send_error(404)
            return
        body = path.read_bytes()
        self.send_response(200)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    do_HEAD = do_GET

    def log_message(self, *_):
        pass

http.server.ThreadingHTTPServer(("127.0.0.1", port), WebDAVHandler).serve_forever()
PY
server_pid=$!

cleanup() {
  if mount | awk -v mount_dir="$mount_dir" '$0 ~ " on " mount_dir " " { found=1 } END { exit !found }'; then
    /sbin/umount -f "$mount_dir" >/dev/null 2>&1 || true
  fi
  kill "$server_pid" >/dev/null 2>&1 || true
  wait "$server_pid" >/dev/null 2>&1 || true
  rmdir "$mount_dir" >/dev/null 2>&1 || true
  rmdir "$fixture_dir" >/dev/null 2>&1 || true
  rmdir "$work_dir" >/dev/null 2>&1 || true
}
trap cleanup EXIT

sleep 0.2
if ! /sbin/mount_webdav -S -v "TelevyBackup WebDAV Check" "http://127.0.0.1:$port/" "$mount_dir"; then
  echo "ERROR: mount_webdav could not mount the loopback fixture" >&2
  exit 1
fi
[[ -d "$mount_dir" ]]
if ! cmp -s "$fixture_dir/hello.txt" "$mount_dir/hello.txt"; then
  echo "ERROR: mounted WebDAV volume did not expose hello.txt" >&2
  ls -la "$mount_dir" >&2 || true
  exit 1
fi
echo "OK: mount_webdav listed and copied a loopback WebDAV file"
