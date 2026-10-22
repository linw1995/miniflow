"""Serve declarative GitHub fixtures; selection policy belongs to the Flow."""
import http.server
import io
import json
from pathlib import Path
import sys
import zipfile
from urllib.parse import parse_qs, urlsplit

config, log = map(Path, sys.argv[1:])


class Handler(http.server.BaseHTTPRequestHandler):
    def log_message(self, *args):
        pass

    def do_GET(self):
        state = json.loads(config.read_text())
        split = urlsplit(self.path)
        path, query = split.path, parse_qs(split.query)
        with log.open('a') as output:
            output.write(self.path + '\n')
        status = state.get('statuses', {}).get(path, 200)
        if status != 200:
            body = {'message': 'fixture unavailable'}
        elif '/attempts/' in path:
            body = state['current']
        elif '/workflows/' in path and path.endswith('/runs'):
            # Default-branch push runs publish coverage graphs; PR runs publish
            # validation records.
            body = {'workflow_runs': state.get('graph_runs', []) if query.get('event') == ['push']
                    else state['runs']}
        elif '/pulls/' in path:
            body = state['pull']
        elif '/runs/' in path and path.endswith('/artifacts'):
            run = path.split('/')[-2]
            body = {'artifacts': ([{'id': int(run) * 10 + 1,
                                    'name': f"pr-validation-{run}-1", 'expired': False}]
                                   if run in state['records'] else [])
                    + ([{'id': int(run) * 10 + 2,
                         'name': f"coverage-graph-{run}-1", 'expired': False}]
                       if run in state.get('graphs', {}) else [])}
        elif '/artifacts/' in path and path.endswith('/zip'):
            artifact = int(path.split('/')[-2])
            run = str(artifact // 10)
            archive = io.BytesIO()
            with zipfile.ZipFile(archive, 'w', compression=zipfile.ZIP_DEFLATED) as zipped:
                if artifact % 10 == 2:
                    graph = state['graphs'][run]
                    zipped.writestr('graph.sqlite', Path(graph['sqlite']).read_bytes())
                    zipped.writestr('graph.metadata.json', json.dumps(graph['metadata']))
                else:
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
