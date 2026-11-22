# Diagrams 2

```mermaid
pie title Pets
    "Dogs" : 386
    "Cats" : 85
    "Rats" : 15
```

```mermaid
gantt
    title Plan
    dateFormat YYYY-MM-DD
    section Build
    Design :done, des, 2026-01-01, 5d
    Code :active, after des, 10d
    section Ship
    Launch :crit, 2026-01-20, 2d
```

```mermaid
erDiagram
    CUSTOMER ||--o{ ORDER : places
    ORDER ||--|{ LINE-ITEM : contains
```
