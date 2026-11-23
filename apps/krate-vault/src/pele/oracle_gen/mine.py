import json, sys
C = []
def add_skip(*a, **k):
    pass
def add(name, src, opts=None):
    c = {'name': name, 'src': src}
    if opts: c['opts'] = opts
    C.append(c)

add('styles', '''flowchart LR
    A[Start] --> B{Choose}
    B -- yes --> C[Do it]
    B -- no --> D[Skip]
    style A fill:#f9f,stroke:#333,stroke-width:4px
    style B fill:#000,color:#fff,font-weight:bold
    style C fill:#14c,stroke-dasharray: 5 5
    style D fill:#ff0,font-size:20px,font-style:italic
''')
add('classdef', '''flowchart TD
    classDef green fill:#9f6,stroke:#333,stroke-width:2px;
    classDef orange fill:#f96,stroke:#333,stroke-width:4px,color:white;
    classDef default fill:#eee,stroke:#999;
    A:::green --> B:::orange
    B --> C
    class C,D orange
    C --> D
    D --> E((Done))
''')
add('linkstyle', '''flowchart LR
    A --> B --> C --> D
    A -.-> D
    linkStyle 0 stroke:#ff3,stroke-width:4px,color:red;
    linkStyle 1,2 stroke:blue
    linkStyle default interpolate basis stroke:#999
    linkStyle 3 interpolate linear
''')
add('markdown', '''flowchart LR
    a["`The **cat** in the _hat_
    on a mat`"] --> b["`A ***very*** long markdown label that is going to wrap around because it is longer than the width`"]
    b -- "`edge **bold**`" --> c["`snake_case_name and *x*`"]
''')
add('html-labels', '''flowchart TD
    A["<b>Bold</b> and <i>italic</i> and <code>mono</code>"] --> B["H<sub>2</sub>O and x<sup>2</sup>"]
    B --> C["<u>under</u> <span style='color:red'>span</span> List<T>"]
    C --> D["a<br/>b<br>c\\nd"]
    D --> E["&lt;tag&gt; &amp; #quot;quoted#quot; #35; &copy; &#9829;"]
''')
add('click', '''flowchart LR
    A[Google] --> B[Local]
    B --> C[Callback]
    C --> D[Note]
    click A "https://www.Google.com/search?q=a b" "Search it" _blank
    click B href "./page.html"
    click C callback "Tooltip for C"
    click D "javascript:alert(1)"
    E[My Note]:::internal-link --> A
''')
add('icons', '''flowchart TD
    A[fa:fa-car Car] --> B(fa:fa-twitter for peace)
    B --> C[fab:fa-github]
    C --> D[Text fa:fa-ban]
''')
add('shape-data', '''flowchart LR
    A@{ shape: manual-file, label: "File Handling"}
    B@{ shape: manual-input, label: "User Input"}
    C@{ shape: docs, label: "Multiple Documents"}
    D@{ shape: procs, label: "Process Automation"}
    E@{ shape: paper-tape, label: "Paper Records"}
    F@{ shape: bolt }
    G@{ shape: cloud, label: "Cloud" }
    H@{ shape: bang, label: "Boom" }
    I@{ shape: hourglass }
    J@{ shape: braces, label: "Comment" }
    K@{ shape: das, label: "Horizontal" }
    L@{ shape: lin-cyl, label: "Disk" }
    M@{ shape: curv-trap, label: "Display" }
    N@{ shape: bow-rect, label: "Stored" }
    A --> B --> C --> D --> E --> F --> G
    G --> H --> I --> J --> K --> L --> M --> N
''')
add('shape-data-2', '''flowchart TB
    a@{ shape: tag-doc, label: "Tagged doc" } --> b@{ shape: tag-rect, label: "Tagged" }
    b --> c@{ shape: lin-doc, label: "Lined doc" }
    c --> d@{ shape: delay, label: "Wait" }
    d --> e@{ shape: datastore, label: "Store" }
    e --> f@{ shape: folder, label: "Folder" }
    f --> g@{ shape: bucket, label: "Bucket" }
    g --> h@{ shape: console, label: "Console" }
    h --> i@{ shape: browser, label: "Browser" }
    i --> j@{ shape: person, label: "Person" }
    j --> k@{ shape: sm-circ } --> l@{ shape: fr-circ } --> m@{ shape: f-circ } --> n@{ shape: fork }
    n --> o@{ shape: cross-circ } --> p@{ shape: tri, label: "Extract" } --> q@{ shape: flip-tri, label: "Manual" }
    q --> r@{ shape: notch-pent, label: "Loop" } --> s@{ shape: win-pane, label: "Window" } --> t@{ shape: div-rect, label: "Divided" }
    t --> u@{ shape: lin-rect, label: "Lined" } --> v@{ shape: notch-rect, label: "Card" } --> w@{ shape: text, label: "Just text" }
    w --> x@{ shape: brace-r, label: "Right" } --> y@{ shape: brace, label: "Left" } --> z@{ shape: odd, label: "Odd" }
''')
add('front-matter', '''---
title: A titled chart
config:
  flowchart:
    curve: linear
    nodeSpacing: 70
    rankSpacing: 30
    wrappingWidth: 120
---
flowchart LR
    accTitle: The access title
    accDescr: A description
    A[A long label that wraps at one hundred twenty] --> B
    B --> C
    A --> C
''')
add('directive-step', '''%%{init: {"flowchart": {"curve": "step"}}}%%
flowchart TD
    A --> B
    A --> C
    B --> D
    C --> D
    A --> D
''')
add('stepbefore', '''---
config:
  flowchart:
    curve: stepBefore
---
flowchart LR
    A --> B & C
    B --> D
    C --> D
''')
add('stepafter', '''---
config:
  flowchart:
    curve: stepAfter
---
flowchart TD
    A --> B & C & E
    B --> D
    C --> D
    E --> D
''')
add('edge-ids', '''flowchart LR
    A e1@--> B
    B e2@==> C
    e1@{ animate: true }
    e2@{ curve: linear }
    C ~~~ D
    D --o E
    E x--x F
    F o--o G
    G <==> H
    H -.- I
    I ----> J
    J -..-> K
    K ====> L
''')
add('loops', '''flowchart LR
    A --> A
    A -->|again| A
    A -- third --> A
    A --> B
    B --> B
''')
add('loops-td', '''flowchart TD
    A -->|self| A
    A --> B
    B --> B
    B --> B
''')
add('subgraph-dir', '''flowchart LR
    subgraph TOP
        direction TB
        subgraph B1
            direction RL
            i1 -->f1
        end
        subgraph B2
            direction BT
            i2 -->f2
        end
    end
    A --> TOP --> B
    B1 --> B2
''')
add('subgraph-edges', '''flowchart TB
    c1-->a2
    subgraph one
    a1-->a2
    end
    subgraph two
    b1-->b2
    end
    subgraph three
    c1-->c2
    end
    one --> two
    three --> two
    two --> c2
''')
add('subgraph-styled', '''flowchart TB
    subgraph S1 [First group with a long title]
        A --> B
    end
    subgraph S2 ["Second `md` title"]
        C --> D
    end
    A --> C
    B --> D
    style S1 fill:#ffd,stroke:#cc0
    style S2 fill:#023,color:#fff
    classDef blue fill:#00f
    class S1 blue
''')
add('fills-casings', '''flowchart LR
    subgraph G [Group]
        A --> B
    end
    X --> A
    B --> Y
    X --> Y
    style G fill:#222,stroke:#000
''')
add('ampersand', '''flowchart TD
    a & b --> c & d
    c & d --> e
    e --> f & g & h
''')
add('escapes', '''flowchart LR
    A["Quote #quot; and <script>alert(1)</script>"] --> B["Amp & ampersand"]
    B --> C["Tab\there"]
    C --> D(["Stadium & co"])
''')
add('wrap', '''flowchart TD
    A[Supercalifragilisticexpialidocious_and_more_without_any_spaces_at_all] --> B[A short one]
    B --> C[中文的标签会在汉字之间换行，这样就不会太宽了。还有更多的文字]
    C --> D[Soft­hy­phen­ated­words­that­wrap­somewhere­in­the­middle­of­all­this]
    D --> E[zero​width​space​breaks​are​hints​too​for​wrapping​long​things]
''')
add('comments-ok', '''%% a comment at the top
flowchart TD
    %% a comment
    A --> B
      %% indented comment
    B --> C
''')
add('comments', '''%% a comment at the top
flowchart TD
    %% a comment
    A --> B %% not a comment?
    B --> C
''')
add('bt-subgraph', '''graph BT
    subgraph one [One]
        a --> b
    end
    b --> c
    c --> a
''')
add('rl-loop-labels', '''flowchart RL
    A -->|to b| B
    B -->|back| A
    A -->|self| A
    B ---|plain| C
    C -.->|dotted| A
''')
add('font-size', '''flowchart TD
    A[Big] --> B[Small]
    style A font-size:24px
    style B font-size:10px,font-weight:600
''')
add('collapsed', '''flowchart LR
    subgraph G [Group]
        A --> B
    end
    G@{ view: collapsed }
    X --> A
    B --> Y
''')
add('vertex-props', '''flowchart LR
    A[|borders:lt|Props] --> B
''')
add('unicode', '''flowchart LR
    Ä --> é
    é --> 日本
    日本 --> Ωmega
''')
add('semicolons', '''graph TD;
    A-->B;
    B-->C;C-->D;
''')
add('no-dir', '''graph
    A --> B
''')
add('dir-symbols', '''graph >
    A --> B
''')
add('title-only', '''---
title: Only a title over a long chart that goes on
---
flowchart LR
    A
''')
add('err-syntax', '''flowchart TD
    A --> --> B
''')
add('err-lexical', '''flowchart TD
    A[unterminated
''')
add('err-linkstyle', '''flowchart TD
    A --> B
    linkStyle 5 stroke:red
''')
add('err-shape', '''flowchart TD
    A@{ shape: nope }
''')
add_skip('unsupported', '''sequenceDiagram
    A->>B: hi
''')
add('obsidian-opts', '''flowchart LR
    A[Obsidian] --> B{Options}
    B --> C[Done]
''', {'fontFamily': 'var(--font-text)', 'config': {'startOnLoad': False, 'securityLevel': 'strict', 'themeVariables': {'fontFamily': 'var(--font-mermaid)'}, 'flowchart': {'useMaxWidth': False}, 'sequence': {'useMaxWidth': False}, 'er': {'useMaxWidth': False}}})
add('maxwidth-turn', '''flowchart LR
    A[First step] --> B[Second step] --> C[Third step] --> D[Fourth step] --> E[Fifth step]
''', {'maxWidth': 300})
add('maxwidth-tight', '''flowchart TD
    A --> B & C & D & E & F & G
''', {'maxWidth': 200})
add('font-family', '''flowchart TD
    A --> B
''', {'fontFamily': 'Inter, "Segoe UI", sans-serif', 'fontFamilyMono': 'Menlo', 'fontSize': 14, 'padding': 12})
add('bad-font', '''flowchart TD
    A --> B
''', {'fontFamily': 'x;} body{', 'responsive': False})
add('id-prefix', '''flowchart TD
    accTitle: Title & <more>
    accDescr {
      Multi
      line
    }
    A --> B
''', {'idPrefix': 'diag"1'})
add('images', '''flowchart TD
    A@{ img: "https://example.com/a.png", label: "Image", pos: "t", w: 60, h: 60, constraint: "on" }
    B@{ img: "file:///etc/passwd", label: "Blocked" }
    C@{ icon: "fa:user", form: "square", label: "User Icon", pos: "b", h: 60 }
    D@{ icon: "fa:car", form: "circle" }
    E@{ icon: "fa:bus", form: "rounded", label: "" }
    A --> B --> C --> D --> E
''')
add('nested-deep', '''flowchart LR
    subgraph a [A]
        subgraph b [B]
            subgraph c [C]
                x --> y
            end
            y --> z
        end
        z --> w
    end
    w --> x
    q --> x
''')
add('minlen', '''flowchart TD
    A --> B
    A -------> C
    A -.....-> D
    A ======> E
    B --> F
    C --> F
''')
add('text-edge-forms', '''flowchart LR
    A -- "quoted text" --> B
    B -. "dotted quoted" .-> C
    C == "thick quoted" ==> D
    D -->|"pipe quoted"| E
    E <-- both --> F
    F x-- cross --x G
    G o-. dots .-o H
''')
add('class-on-edge', '''flowchart LR
    A e1@--> B
    classDef animate stroke-dasharray: 9,5,stroke-dashoffset: 900,animation: dash 25s linear infinite;
    class e1 animate
''')
add('special-ids', '''flowchart LR
    A-1 --> B.2
    B.2 --> C_3
    C_3 --> D/4
    D/4 --> end1
    end1 --> default1
''')
icons = {'fa:fa-car': '<path d="M1 1h22"/>', 'fa:user': '<circle cx="12" cy="12" r="4"/>', 'fab:fa-github': '<path d="M2 2"/>', 'fa:bus': '<rect width="4" height="4"/>'}
add('icons-resolved', '''flowchart LR
    A[fa:fa-car Car and fa:fa-unknown x] --> B(fab:fa-github)
    B --> C@{ icon: "fa:user", form: "square", label: "User", pos: "t" }
    C --> D@{ icon: "fa:bus", form: "circle", label: "Bus" }
    D --> E@{ icon: "fa:user" }
    E -->|fa:fa-car drive| F[end fa:fa-car]
''', {'iconsMap': icons})
add('icons-wrap', '''flowchart TD
    A[fa:fa-car one two three four five six seven eight nine ten eleven twelve thirteen fourteen] --> B
''', {'iconsMap': icons})
json.dump(C, open(sys.argv[1], 'w'))
print(len(C))
