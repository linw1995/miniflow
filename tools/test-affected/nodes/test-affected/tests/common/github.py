"""Serve declarative GitHub fixtures; selection policy belongs to the Flow."""
import http.server
import io
import json
from pathlib import Path
import sys
import zipfile
from urllib.parse import urlsplit

config, log = map(Path, sys.argv[1:])


class Handler(http.server.BaseHTTPRequestHandler):
    def log_message(self, *args):
        pass

    def do_GET(self):
        state = json.loads(config.read_text())
        path = urlsplit(self.path).path
        with log.open('a') as output:
            output.write(path + '\n')
        status = state.get('statuses', {}).get(path, 200)
        if status != 200:
            body = {'message': 'fixture unavailable'}
        elif '/attempts/' in path:
            body = state['current']
        elif '/workflows/' in path and path.endswith('/runs'):
            body = {'workflow_runs': state['runs']}
        elif '/pulls/' in path:
            body = state['pull']
        elif '/runs/' in path and path.endswith('/artifacts'):
            run = path.split('/')[-2]
            body = {'artifacts': [{'id': int(run) * 10 + 1,
                                  'name': f"pr-validation-{run}-1", 'expired': False}]
                    if run in state['records'] else []}
        elif '/artifacts/' in path and path.endswith('/zip'):
            run = str(int(path.split('/')[-2]) // 10)
            archive = io.BytesIO()
            with zipfile.ZipFile(archive, 'w', compression=zipfile.ZIP_DEFLATED) as zipped:
                zipped.writestr('validation.json', json.dumps(state['records'][run]))
            body = archive.getvalue()
        else:
            status, body = 404, {'message': 'unknown fixture path'}
        raw = body if isinstance(body, bytes) else json.dumps(body).encode()
        self.send_response(status)
        self.send_header('Content-Length', str(len(raw)))
        self.end_headers()
        try:
            self.wfile.write(raw)
        except (BrokenPipeError, ConnectionResetError):
            pass


server = http.server.ThreadingHTTPServer(('127.0.0.1', 0), Handler)
print(f'http://127.0.0.1:{server.server_port}/', flush=True)
server.serve_forever()
