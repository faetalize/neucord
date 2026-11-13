import json, sys
pairs = lambda s: json.loads(s, object_pairs_hook=lambda p: p)
for line in open(sys.argv[1]):
    r = json.loads(line)
    if r['kind'] == 'identify':
        top = dict(pairs(r['raw']))
        d = dict(top['d'])
        print('IDENTIFY #: key order', [k for k, _ in top['d']])
        print('  capabilities', d['capabilities'], '| presence', json.dumps(dict(d['presence'])) , '| client_state', dict(d['client_state']), '| compress', d['compress'])
        print('  properties:', json.dumps(dict(d['properties'])))
        print('  property order:', [k for k, _ in d['properties']])
    elif r['kind'] == 'http':
        sp = r['super_properties']
        print('HTTP', r['method'], r['path'][:72], '| auth' if r['authorization'] else '| NO-AUTH', '| super-properties(%d keys)' % len(sp) if sp else '| NO-SUPER')
        for header in ('audit_log_reason', 'context_properties'):
            if header in r:
                print('  ', header, json.dumps(r[header]))
        if 'body' in r:
            print('   body', json.dumps(r['body']))
        elif 'body_length' in r:
            print('   body of %d bytes' % r['body_length'])
    elif r['kind'] == 'image':
        print('IMAGE', r['path'].split('/')[-1][:60])
    else:
        print(r['kind'], {k: v for k, v in r.items() if k not in ('raw', 'at', 'kind')})
