import json, random, sys
random.seed(int(sys.argv[3]))
base = json.load(open(sys.argv[1]))
srcs = [c['src'] for c in base]
alphabet = list('[](){}|<>"`@:;,&*#-=.~ox/\\\n\t ') + ['-->', '---', '==>', '-.->', '%%', '%%{init: {"flowchart": {"curve": "linear"}}}%%\n', '#quot;', '&amp;', ' ', '　', 'é', '中', '😀', '​', '­', 'end', 'subgraph x\n', 'direction LR\n', 'style A fill:#f00;', 'classDef c fill:#0f0;', 'click A "http://x"', 'A@{ shape: rect }', 'e1@', '\r\n', '<br>', '**', '__', 'fa:fa-x ', 'accTitle: t\n', '---\ntitle: T\n---\n']
out = []
for k in range(int(sys.argv[4])):
    s = random.choice(srcs)
    for _ in range(random.randint(1, 4)):
        i = random.randint(0, len(s))
        r = random.random()
        if r < 0.5:
            s = s[:i] + random.choice(alphabet) + s[i:]
        elif r < 0.8:
            s = s[:i] + s[i + random.randint(1, 4):]
        else:
            j = random.randint(0, len(s))
            s = s[:i] + s[j:j + random.randint(1, 10)] + s[i:]
    out.append({'name': 'mut-%s-%04d' % (sys.argv[3], k), 'src': s})
json.dump(out, open(sys.argv[2], 'w'))
