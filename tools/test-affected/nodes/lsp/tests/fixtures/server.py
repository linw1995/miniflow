import json
import os
from pathlib import Path
import sys
import time

mode, log = sys.argv[1:]
Path(log + '.pid').write_text(str(os.getpid()))


def send(message):
    payload = json.dumps({'jsonrpc': '2.0', **message}).encode()
    if mode == 'duplicate_length':
        sys.stdout.buffer.write(b'Content-Length: wrong\r\n')
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


if mode == 'write_blocked':
    time.sleep(60)

while True:
    request = receive()
    method = request.get('method')
    if not method:
        if mode == 'client_requests':
            if request['id'] == 1002:
                assert request['result'] == [True, {'fixture': {'enabled': True}}]
            elif request['id'] == 1003:
                assert request['result'][0]['uri'].startswith('file:///')
            elif request['id'] == 1004:
                assert request['result'] == {'applied': False}
            elif request['id'] == 1005:
                assert request['error']['code'] == -32601
        continue
    with open(log, 'a') as stream:
        stream.write(json.dumps(request) + '\n')
    if mode == 'eof':
        sys.exit(0)
    if mode in ('large_header', 'unterminated_header', 'large_message', 'invalid_json'):
        raw = {'large_header': b'X-Padding: ' + b'x' * 9000 + b'\r\nContent-Length: 2\r\n\r\n{}',
               'unterminated_header': b'x' * 9000,
               'large_message': b'Content-Length: 1000000\r\n\r\n',
               'invalid_json': b'Content-Length: 1\r\n\r\n{'}[mode]
        sys.stdout.buffer.write(raw)
        sys.stdout.buffer.flush()
        time.sleep(60)
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
        if mode == 'no_symbols':
            capabilities['documentSymbolProvider'] = False
        if mode == 'bad_encoding':
            capabilities['positionEncoding'] = 'bytes'
        if mode == 'bad_capabilities':
            capabilities = None
        send({'id': request['id'], 'result': {'capabilities': capabilities, 'serverInfo': {'name': 'fixture'}}})
    elif method == 'initialized':
        if mode == 'unhealthy':
            send({'method': 'experimental/serverStatus', 'params': {'quiescent': True, 'health': 'error'}})
        if mode == 'client_requests':
            send({'id': 1002, 'method': 'workspace/configuration', 'params': {'items': [{'section': 'fixture.enabled'}, {'section': 'rust-analyzer'}]}})
            send({'id': 1003, 'method': 'workspace/workspaceFolders', 'params': {}})
            send({'id': 1004, 'method': 'workspace/applyEdit', 'params': {}})
            send({'id': 1005, 'method': 'client/registerCapability', 'params': {}})
        send({'id': 1000, 'method': 'window/workDoneProgress/create', 'params': {'token': 'load'}})
        send({'id': 1001, 'method': 'workspace/configuration', 'params': {'items': [{'section': ''}]}})
        send({'method': '$/progress', 'params': {'token': 'load', 'value': {'kind': 'begin', 'title': 'loading'}}})
    elif method == 'textDocument/documentSymbol':
        source_range = {'start': {'line': 0, 'character': 3}, 'end': {'line': 0, 'character': 9}}
        symbol = {'name': 'chosen', 'kind': 12, 'range': source_range, 'selectionRange': source_range}
        result = [symbol]
        if mode == 'flat':
            result = [{'name': 'chosen', 'kind': 12, 'location': {'uri': request['params']['textDocument']['uri'], 'range': source_range}}]
        elif mode == 'children':
            symbol['children'] = [dict(symbol)]
        elif mode == 'null_symbols':
            result = None
        elif mode == 'bad_symbols':
            result = {}
        elif mode == 'missing_range':
            result = [{'name': 'chosen'}]
        elif mode == 'reversed_range':
            symbol['range'] = {'start': source_range['end'], 'end': source_range['start']}
        elif mode == 'document_limit':
            result = [symbol, dict(symbol)]
        send({'id': request['id'], 'result': result})
    elif method == 'textDocument/references':
        if mode in ('rpc_error', 'missing_result', 'bad_array'):
            response = {'id': request['id']}
            if mode == 'rpc_error':
                response['error'] = {'code': -32603, 'message': 'fixture query failure'}
            elif mode == 'bad_array':
                response['result'] = {}
            send(response)
            continue
        if mode == 'null_references':
            send({'id': request['id'], 'result': None})
            continue
        if mode in ('document_limit', 'snapshot_limit'):
            send({'id': request['id'], 'result': ['x' * 40000]})
            continue
        if mode == 'partial_limit':
            for _ in range(2):
                send({'method': '$/progress', 'params': {'token': request['params']['partialResultToken'], 'value': ['x' * 40000]}})
            continue
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
