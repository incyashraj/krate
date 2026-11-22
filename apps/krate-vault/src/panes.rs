//! How the middle of the window is split into tab groups: a tree of
//! splits, side by side or one above the other, each child with its share
//! of the room. "Split right" and "Split down" add a group beside the one
//! in focus; closing a group's last tab takes the group away and gives its
//! room to its neighbours.

use alloc::vec::Vec;
#[cfg(target_arch = "wasm32")]
#[allow(unused_imports)]
use krate::prelude::*;

/// A box on screen, in the window's pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Area {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Area {
    pub fn new(x: f32, y: f32, w: f32, h: f32) -> Area {
        Area { x, y, w, h }
    }

    pub fn contains(&self, px: f32, py: f32) -> bool {
        px >= self.x && py >= self.y && px < self.x + self.w && py < self.y + self.h
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Layout {
    /// A tab group, by its index in the app's list of groups.
    Pane(usize),
    /// Children side by side (`down: false`) or stacked (`down: true`),
    /// with their shares of the room (summing to 1).
    Split {
        down: bool,
        kids: Vec<Layout>,
        sizes: Vec<f32>,
    },
}

/// A divider between two children of a split, as drawn: the split's path
/// in the tree and which gap (after child `i`).
#[derive(Debug, Clone, PartialEq)]
pub struct Divider {
    pub path: Vec<usize>,
    pub after: usize,
    pub down: bool,
    pub area: Area,
    /// The split's own box, to turn a drag into shares.
    pub whole: Area,
}

impl Layout {
    pub fn panes(&self, out: &mut Vec<usize>) {
        match self {
            Layout::Pane(p) => out.push(*p),
            Layout::Split { kids, .. } => kids.iter().for_each(|k| k.panes(out)),
        }
    }

    pub fn contains(&self, pane: usize) -> bool {
        match self {
            Layout::Pane(p) => *p == pane,
            Layout::Split { kids, .. } => kids.iter().any(|k| k.contains(pane)),
        }
    }

    /// Put `new` beside `at`: to the right, or below when `down`.
    pub fn split(&mut self, at: usize, new: usize, down: bool) -> bool {
        match self {
            Layout::Pane(p) if *p == at => {
                *self = Layout::Split {
                    down,
                    kids: alloc::vec![Layout::Pane(at), Layout::Pane(new)],
                    sizes: alloc::vec![0.5, 0.5],
                };
                true
            }
            Layout::Pane(_) => false,
            Layout::Split {
                down: d,
                kids,
                sizes,
            } => {
                for i in 0..kids.len() {
                    if kids[i] == Layout::Pane(at) && *d == down {
                        // Same direction: share this child's room with the new one.
                        let half = sizes[i] * 0.5;
                        sizes[i] = half;
                        kids.insert(i + 1, Layout::Pane(new));
                        sizes.insert(i + 1, half);
                        return true;
                    }
                    if kids[i].split(at, new, down) {
                        return true;
                    }
                }
                false
            }
        }
    }

    /// Take a group out; its room goes to a neighbour. Splits left with one
    /// child become that child.
    pub fn remove(&mut self, pane: usize) {
        if let Layout::Split { kids, sizes, .. } = self {
            if let Some(i) = kids.iter().position(|k| *k == Layout::Pane(pane)) {
                kids.remove(i);
                // Its room goes to the neighbour before it (after, if first).
                let gone = sizes.remove(i);
                if let Some(s) = sizes.get_mut(i.saturating_sub(1)) {
                    *s += gone;
                }
            } else {
                for k in kids.iter_mut() {
                    k.remove(pane);
                }
            }
            if kids.len() == 1 {
                let only = kids.remove(0);
                *self = only;
            }
        }
    }

    /// Group numbers above `gone` move down one (after a group is deleted
    /// from the app's list).
    pub fn renumber_after(&mut self, gone: usize) {
        match self {
            Layout::Pane(p) => {
                if *p > gone {
                    *p -= 1;
                }
            }
            Layout::Split { kids, .. } => kids.iter_mut().for_each(|k| k.renumber_after(gone)),
        }
    }

    /// Every group's box inside `area`, and the dividers between them.
    pub fn place(&self, area: Area, out: &mut Vec<(usize, Area)>, divs: &mut Vec<Divider>) {
        self.place_at(area, out, divs, &mut Vec::new());
    }

    fn place_at(
        &self,
        area: Area,
        out: &mut Vec<(usize, Area)>,
        divs: &mut Vec<Divider>,
        path: &mut Vec<usize>,
    ) {
        match self {
            Layout::Pane(p) => out.push((*p, area)),
            Layout::Split { down, kids, sizes } => {
                let total = if *down { area.h } else { area.w };
                let mut at = if *down { area.y } else { area.x };
                for (i, k) in kids.iter().enumerate() {
                    let share = sizes.get(i).copied().unwrap_or(0.0);
                    let len = if i + 1 == kids.len() {
                        (if *down {
                            area.y + area.h
                        } else {
                            area.x + area.w
                        }) - at
                    } else {
                        (total * share).round()
                    };
                    let sub = if *down {
                        Area::new(area.x, at, area.w, len)
                    } else {
                        Area::new(at, area.y, len, area.h)
                    };
                    path.push(i);
                    k.place_at(sub, out, divs, path);
                    path.pop();
                    at += len;
                    if i + 1 < kids.len() {
                        divs.push(Divider {
                            path: path.clone(),
                            after: i,
                            down: *down,
                            area: if *down {
                                Area::new(area.x, at - 3.0, area.w, 6.0)
                            } else {
                                Area::new(at - 3.0, area.y, 6.0, area.h)
                            },
                            whole: area,
                        });
                    }
                }
            }
        }
    }

    /// Drag a divider so the gap after child `after` sits at `pos` (a
    /// window coordinate along the split).
    pub fn drag(&mut self, path: &[usize], after: usize, whole: Area, pos: f32) {
        let mut node = self;
        for &i in path {
            match node {
                Layout::Split { kids, .. } => match kids.get_mut(i) {
                    Some(k) => node = k,
                    None => return,
                },
                Layout::Pane(_) => return,
            }
        }
        if let Layout::Split { down, sizes, .. } = node {
            let (start, len) = if *down {
                (whole.y, whole.h)
            } else {
                (whole.x, whole.w)
            };
            if len <= 1.0 || after + 1 >= sizes.len() {
                return;
            }
            let before: f32 = sizes.iter().take(after).sum();
            let pair = sizes[after] + sizes[after + 1];
            let min = (120.0 / len).min(pair * 0.5);
            let f = ((pos - start) / len - before).clamp(min, pair - min);
            sizes[after] = f;
            sizes[after + 1] = pair - f;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_place_and_close() {
        let mut l = Layout::Pane(0);
        assert!(l.split(0, 1, false));
        assert!(l.split(1, 2, true));
        let mut out = Vec::new();
        let mut divs = Vec::new();
        l.place(Area::new(0.0, 0.0, 1000.0, 600.0), &mut out, &mut divs);
        assert_eq!(out[0], (0, Area::new(0.0, 0.0, 500.0, 600.0)));
        assert_eq!(out[1], (1, Area::new(500.0, 0.0, 500.0, 300.0)));
        assert_eq!(out[2], (2, Area::new(500.0, 300.0, 500.0, 300.0)));
        assert_eq!(divs.len(), 2);
        // A third group to the right of the first shares its room.
        assert!(l.split(0, 3, false));
        out.clear();
        divs.clear();
        l.place(Area::new(0.0, 0.0, 1000.0, 600.0), &mut out, &mut divs);
        assert_eq!(out[0].1.w, 250.0);
        assert_eq!(out[1], (3, Area::new(250.0, 0.0, 250.0, 600.0)));
        l.remove(3);
        l.remove(2);
        assert_eq!(
            l,
            Layout::Split {
                down: false,
                kids: alloc::vec![Layout::Pane(0), Layout::Pane(1)],
                sizes: alloc::vec![0.5, 0.5]
            }
        );
        l.drag(&[], 0, Area::new(0.0, 0.0, 1000.0, 600.0), 700.0);
        out.clear();
        l.place(Area::new(0.0, 0.0, 1000.0, 600.0), &mut out, &mut divs);
        assert_eq!(out[0].1.w, 700.0);
        l.remove(1);
        assert_eq!(l, Layout::Pane(0));
    }
}
