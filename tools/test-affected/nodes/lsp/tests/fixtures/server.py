import json
import os
from pathlib import Path
import sys
import time

mode, log = sys.argv[1:]
Path(log + '.pid').write_text(str(os.getpid()))


def send(message):
    payload = json.dumps({'jsonrpc': '2.0', **message}).encode()
    sys.stdout.buffer.write(f'Content-Length: {len(payload)}\r\n\r\n'.encode() + payload)
    sys.stdout.buffer.flush()


def receive():
    fields = {}
    while True:
        line = sys.stdin.buffer.readline()
        if not line:
            sys.exit(0)
        if line == b'\r\n':
            break
        name, value = line.split(b':', 1)
        fields[name.lower()] = value.strip()
    return json.loads(sys.stdin.buffer.read(int(fields[b'content-length'])))


while True:
    request = receive()
    method = request.get('method')
    if not method:
        continue
    with open(log, 'a') as stream:
        stream.write(json.dumps(request) + '\n')
    if mode == 'idle':
        time.sleep(60)
    if mode == 'malformed':
        sys.stdout.buffer.write(b'Content-Length: wrong\r\n\r\n')
        sys.stdout.buffer.flush()
        time.sleep(60)
    if method == 'initialize':
        capabilities = {'documentSymbolProvider': True, 'positionEncoding': 'utf-16'}
        if mode != 'limited':
            capabilities |= {'referencesProvider': True, 'callHierarchyProvider': True}
        send({'id': request['id'], 'result': {'capabilities': capabilities, 'serverInfo': {'name': 'fixture'}}})
    elif method == 'initialized':
        send({'id': 1000, 'method': 'window/workDoneProgress/create', 'params': {'token': 'load'}})
        send({'id': 1001, 'method': 'workspace/configuration', 'params': {'items': [{'section': ''}]}})
        send({'method': '$/progress', 'params': {'token': 'load', 'value': {'kind': 'begin', 'title': 'loading'}}})
    elif method == 'textDocument/documentSymbol':
        send({'id': request['id'], 'result': [{'name': 'chosen', 'kind': 12,
             'range': {'start': {'line': 0, 'character': 3}, 'end': {'line': 0, 'character': 9}},
             'selectionRange': {'start': {'line': 0, 'character': 3}, 'end': {'line': 0, 'character': 9}}}]})
    elif method == 'textDocument/references':
        send({'method': '$/progress', 'params': {'token': request['params']['partialResultToken'],
             'value': [{'uri': 'file:///partial', 'range': {'start': {'line': 0, 'character': 0}, 'end': {'line': 0, 'character': 1}}}]}})
        send({'id': request['id'], 'result': [{'uri': 'file:///final', 'range': {'start': {'line': 0, 'character': 0}, 'end': {'line': 0, 'character': 1}}}]})
    elif method == 'textDocument/prepareCallHierarchy':
        send({'id': request['id'], 'result': [{'name': 'chosen', 'data': {'opaque': 42}}]})
    elif method == 'callHierarchy/incomingCalls':
        assert request['params']['item']['data'] == {'opaque': 42}
        send({'id': request['id'], 'result': [{'from': {'name': 'test_chosen'}, 'fromRanges': []}]})
    elif method == 'shutdown':
        send({'id': request['id'], 'result': None})
    elif method == 'exit':
        sys.exit(0)
    else:
        send({'id': request['id'], 'error': {'code': -32601, 'message': 'not supported'}})
