//! The sample vault, and the `quick` run that exercises the app on it.
//!
//! The sample is held in memory: a person trying Vault without a folder of
//! their own sees every feature with real notes, and nothing touches their
//! disk.

use crate::app::{mem_vault, App, Cmd, Hit, Mods};
use crate::vault::Vault;
use alloc::string::String;

pub const FILES: &[(&str, &str)] = &[
    (
        "Start here.md",
        "# Start here\n\
This is a **vault**: a folder of plain Markdown notes. Every note is a `.md` file you can open anywhere.\n\
\n\
## Find your way around\n\
- The **file tree** on the left holds every note and folder.\n\
- Press `Cmd+O` to jump to any note by name, `Cmd+P` for every command.\n\
- `Cmd+Shift+F` searches all notes at once.\n\
- Notes link to each other with double brackets: see [[Weekly review]] or [[Projects/Garden plan|the garden plan]].\n\
\n\
## Things to try\n\
- [x] Open this note\n\
- [ ] Type `[[` and pick a note to link to\n\
- [ ] Press `Cmd+E` to switch to the reading view\n\
- [ ] Look at **Backlinks** on the right: the notes that link here\n\
\n\
> Notes are yours. They stay plain text, in your folder.\n\
\n\
#welcome #howto\n",
    ),
    (
        "Weekly review.md",
        "# Weekly review\n\
Every Friday, look back at the week.\n\
\n\
1. Clear the inbox\n\
2. Read [[Journal/2026-11-20]] and [[Journal/2026-11-21]]\n\
3. Update [[Projects/Garden plan]] and [[Projects/Reading list]]\n\
4. Pick three things for next week\n\
\n\
Back to [[Start here]]. #routine\n",
    ),
    (
        "Projects/Garden plan.md",
        "# Garden plan\n\
Spring beds, from the *south* fence to the shed.\n\
\n\
## Beds\n\
| Bed | Crop | Sown |\n\
| --- | --- | --- |\n\
| 1 | Tomatoes | March |\n\
| 2 | Beans | April |\n\
\n\
## To do\n\
- [x] Order seeds\n\
- [ ] Build the second bed\n\
- [ ] Ask about the ==water butt==\n\
\n\
Linked from [[Weekly review]]. See also [[Recipes/Tomato sauce]]. #project #garden\n",
    ),
    (
        "Projects/Reading list.md",
        "# Reading list\n\
- *The Pragmatic Programmer*\n\
- ~~A book I gave up on~~\n\
- Something from [[Journal/2026-11-21]]\n\
\n\
#project #books\n",
    ),
    (
        "Recipes/Tomato sauce.md",
        "# Tomato sauce\n\
From the beds in [[Projects/Garden plan]].\n\
\n\
```\n\
2 kg tomatoes\n\
1 onion, 3 cloves garlic\n\
olive oil, salt, basil\n\
```\n\
\n\
Simmer for **forty minutes**. #recipe\n",
    ),
    (
        "Journal/2026-11-20.md",
        "# Thursday\n\
Planted garlic. Wrote the [[Weekly review]] template.\n\
\n\
#journal\n",
    ),
    (
        "Journal/2026-11-21.md",
        "# Friday\n\
Finished a chapter from the [[Projects/Reading list]].\n\
Idea: a [[Seed swap]] in spring.\n\
\n\
#journal #idea\n",
    ),
];

pub fn vault() -> Vault {
    mem_vault("Sample vault", FILES)
}

/// The `quick` run: open the sample, use it the way a person would, and
/// report what the app holds afterwards.
pub fn quick(app: &mut App) -> String {
    let none = Mods::default();
    let cmd = Mods {
        primary: true,
        ..Mods::default()
    };
    app.open_vault(vault());
    if let Some(v) = app.vault.as_mut() {
        v.index_some(&mut || true);
    }
    app.open("Start here.md", false);
    let backlinks_start = app.backlinks().len();
    // The quick switcher, fuzzy.
    app.key("o", cmd);
    app.text("gardn");
    app.key("Enter", none);
    let switched = app.doc().map(|d| d.title.clone()).unwrap_or_default();
    let backlinks_garden = app.backlinks().len();
    let outline = app.outline().len();
    // Type a link with completion, then save.
    if let Some(d) = app.doc_mut() {
        let end = d.buf.end();
        d.place(end, false);
    }
    app.text("\nNext: [[tomato");
    let suggested = app.completion.as_ref().map_or(0, |c| c.items.len());
    app.key("Enter", none);
    app.run(Cmd::Save);
    // Search the vault.
    app.search = crate::app::Input::with("tomatoes");
    app.run_search();
    app.tick(&mut || true);
    // Follow a link that has no note yet.
    app.click(Hit::LinkNote(String::from("Seed swap")), none);
    let made = app.doc().map(|d| d.path.clone()).unwrap_or_default();
    app.navigate(false);

    let mut out = String::new();
    let mut put = |k: &str, v: &str| {
        out.push_str(k);
        out.push(':');
        out.push_str(v);
        out.push('\n');
    };
    let num = crate::text::num;
    put("switched_to", &switched);
    put("backlinks_start_here", &num(backlinks_start));
    put("backlinks_garden_plan", &num(backlinks_garden));
    put("outline_headings", &num(outline));
    put("link_suggestions", &num(suggested));
    put("created_from_link", &made);
    out.push_str(&app.report());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_quick_run_uses_every_part() {
        let mut app = App::new();
        let out = quick(&mut app);
        assert!(out.contains("switched_to:Garden plan\n"), "{out}");
        assert!(out.contains("backlinks_start_here:1\n"), "{out}");
        assert!(out.contains("backlinks_garden_plan:3\n"), "{out}");
        assert!(out.contains("outline_headings:3\n"), "{out}");
        assert!(out.contains("link_suggestions:1\n"), "{out}");
        assert!(out.contains("created_from_link:Seed swap.md\n"), "{out}");
        assert!(out.contains("search_notes:2\n"), "{out}");
        assert!(out.contains("note:Projects/Garden plan.md\n"), "{out}");
        let saved = app
            .vault
            .as_mut()
            .and_then(|v| v.fs.read("Projects/Garden plan.md").ok())
            .unwrap_or_default();
        assert!(saved.contains("Next: [[Tomato sauce]]"), "{saved}");
    }
}
