import json, random, sys
random.seed(int(sys.argv[2]))
N = int(sys.argv[3])
words = "alpha beta gamma delta build test ship review deploy check the a of and to cache queue worker node service API db user login error retry done start end 中文 标签 日本語 Ωmega naïve café über".split()
shapes = ['[%s]','(%s)','([%s])','[[%s]]','[(%s)]','((%s))','>%s]','{%s}','{{%s}}','[/%s/]','[\\%s\\]','[/%s\\]','[\\%s/]','(((%s)))']
links = ['-->','---','-.->','==>','--o','--x','<-->','-.-','===','~~~','x--x','o--o','---->','-..->','==>']
labeled = ['-- %s -->','-. %s .->','== %s ==>','-->|%s|','---|%s|','-.->|%s|','==>|%s|']
dirs = ['TD','TB','LR','RL','BT']
newshapes = ['manual-file','manual-input','docs','procs','paper-tape','cloud','bang','das','lin-cyl','curv-trap','bow-rect','tag-doc','tag-rect','lin-doc','delay','datastore','folder','bucket','console','browser','person','tri','flip-tri','notch-pent','win-pane','div-rect','lin-rect','notch-rect','text','brace','brace-r','braces','odd','hex','stadium','cyl','circle','dbl-circ','diam','rounded','rect','sl-rect','st-rect','doc','flag','h-cyl','cross-circ','bolt','hourglass','fork','sm-circ','fr-circ','f-circ']
def label():
    n = random.choice([1,1,2,3,5,9,14])
    t = ' '.join(random.choice(words) for _ in range(n))
    r = random.random()
    if r < 0.1: t = '<b>' + t + '</b>'
    elif r < 0.15: t = t.replace(' ', '<br>', 1)
    elif r < 0.2: t = '"' + t + ' & more"'
    elif r < 0.3: t = '"`' + t.replace(' ', ' **', 1).replace(' ', '** ', 1) + ' _it_`"'
    elif r < 0.35: t = t + ' #quot;q#quot; #35; &copy;'
    elif r < 0.4: t = 'H<sub>2</sub>O ' + t + ' x<sup>2</sup> <i>' + t + '</i> <code>c</code>'
    elif r < 0.45: t = t.replace(' ', '\u00ad') + 'verylongwordwithoutanyspacesatallinsideit'
    elif r < 0.5: t = 'fa:fa-car ' + t
    return t
cases = []
for k in range(N):
    d = random.choice(dirs)
    n = random.randint(2, 14)
    ids = ['n%d' % i for i in range(n)]
    lines = []
    if random.random() < 0.15:
        cv = random.choice(['linear','step','stepBefore','stepAfter','basis'])
        lines.append('---')
        if random.random() < 0.5: lines.append('title: ' + label().replace('"','').replace('<b>','').replace('</b>','').replace('<br>',' '))
        lines.append('config:\n  flowchart:\n    curve: %s' % cv)
        if random.random() < 0.5: lines.append('    nodeSpacing: %d\n    rankSpacing: %d' % (random.randint(10,90), random.randint(10,90)))
        lines.append('---')
    lines.append('flowchart ' + d)
    declared = set()
    def node(i):
        nid = ids[i]
        if nid in declared or random.random() < 0.3:
            return nid
        declared.add(nid)
        if random.random() < 0.15:
            return '%s@{ shape: %s, label: "%s" }' % (nid, random.choice(newshapes), label().replace('"',''))
        return nid + (random.choice(shapes) % ('"' + label().replace('"','') + '"' if random.random()<0.3 else label().replace('"','').replace('[','').replace(']','').replace('(','').replace(')','').replace('{','').replace('}','').replace('|','').replace('/','').replace('\\','').replace('>','')))
    groups = []
    m = random.randint(n - 1, n + 6)
    body = []
    for e in range(m):
        a = random.randrange(n); b = random.randrange(n)
        if random.random() < 0.25:
            lk = random.choice(labeled) % random.choice(words + ['two words', 'a longer edge label here'])
        else:
            lk = random.choice(links)
        body.append('    %s %s %s' % (node(a), lk, node(b)))
    if random.random() < 0.4:
        g = random.sample(range(len(body)), k=min(len(body), random.randint(1, 4)))
        inner = [body[i] for i in sorted(g)]
        rest = [body[i] for i in range(len(body)) if i not in g]
        sub = ['    subgraph s1 [%s]' % random.choice(words)]
        if random.random() < 0.5: sub.append('      direction ' + random.choice(['TB','LR','RL','BT']))
        sub += ['  ' + x for x in inner]
        if random.random() < 0.4 and len(rest) > 2:
            sub.append('      subgraph s2 [inner %s]' % random.choice(words))
            sub.append('    ' + rest.pop())
            sub.append('      end')
        sub.append('    end')
        body = rest[:len(rest)//2] + sub + rest[len(rest)//2:]
        if random.random() < 0.5: body.append('    s1 --> %s' % ids[random.randrange(n)])
    lines += body
    if random.random() < 0.3:
        lines.append('    style %s fill:%s,stroke:#333,stroke-width:%dpx' % (ids[random.randrange(n)], random.choice(['#f9f','#000','#fff','#4af','#222','#ffe']), random.randint(1,4)))
    if random.random() < 0.2:
        lines.append('    classDef hot fill:#f66,color:#fff,font-weight:bold')
        lines.append('    class %s hot' % ','.join(random.sample(ids, k=min(3, n))))
    if random.random() < 0.15:
        lines.append('    linkStyle 0 stroke:#f00,stroke-width:3px')
    if random.random() < 0.15:
        lines.append('    linkStyle default interpolate %s' % random.choice(['linear','step','basis','stepAfter']))
    if random.random() < 0.2:
        x = ids[random.randrange(n)]
        lines.append('    %s --> %s' % (x, x))
        lines.append('    %s -->|loop label| %s' % (x, x))
    if random.random() < 0.15:
        lines.append('    classDef big font-size:22px,font-style:italic,stroke-width:3px,fill:#333')
        lines.append('    class %s big' % ids[random.randrange(n)])
    if random.random() < 0.1:
        lines.append('    click %s "https://example.com/%s" "tip"' % (ids[random.randrange(n)], random.choice(words)))
    cases.append({'name': 'rich-%s-%03d' % (sys.argv[2], k), 'src': '\n'.join(lines) + '\n'})
json.dump(cases, open(sys.argv[1], 'w'))
