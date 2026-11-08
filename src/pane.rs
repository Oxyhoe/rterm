//! Window subdivision: each pane holds a list of tabs, panes split further.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dir {
    Left,
    Right,
    Up,
    Down,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
}

impl Rect {
    pub fn contains(&self, x: i32, y: i32) -> bool {
        x >= self.x && y >= self.y && x < self.x + self.w as i32 && y < self.y + self.h as i32
    }
}

#[derive(Debug, Clone)]
pub struct Leaf {
    pub tabs: Vec<usize>,
    pub active: usize,
}

impl Leaf {
    fn new(sid: usize) -> Self {
        Self {
            tabs: vec![sid],
            active: 0,
        }
    }

    fn current(&self) -> usize {
        self.tabs.get(self.active).copied().unwrap_or(usize::MAX)
    }
}

#[derive(Debug, Clone)]
pub enum Node {
    Leaf(Leaf),
    Split {
        dir: Dir,
        ratio: f32,
        first: Box<Node>,
        second: Box<Node>,
    },
}

/// The pane tree of one window plus the session that currently has focus.
#[derive(Debug, Clone)]
pub struct PaneTree {
    root: Node,
    active: usize,
}

/// A leaf together with the rectangle it occupies.
pub struct PaneRect<'a> {
    pub leaf: &'a Leaf,
    pub rect: Rect,
}

impl PaneTree {
    pub fn new(sid: usize) -> Self {
        Self {
            root: Node::Leaf(Leaf::new(sid)),
            active: sid,
        }
    }

    pub fn active(&self) -> usize {
        self.active
    }

    pub fn set_active(&mut self, sid: usize) {
        if self.contains(sid) {
            self.active = sid;
            focus_tab(&mut self.root, sid);
        }
    }

    pub fn contains(&self, sid: usize) -> bool {
        node_contains(&self.root, sid)
    }

    /// Every live session, leaf order (left-to-right, top-to-bottom).
    pub fn sessions(&self) -> Vec<usize> {
        let mut out = Vec::new();
        collect(&self.root, &mut out);
        out
    }

    /// Split the pane holding `self.active` with a new session.
    pub fn split(&mut self, dir: Dir, sid: usize) {
        let active = self.active;
        if !split_node(&mut self.root, dir, active, sid) {
            return;
        }
        self.active = sid;
    }

    /// Open a tab in the pane that has focus.
    pub fn new_tab(&mut self, sid: usize) {
        if let Some(leaf) = leaf_with(&mut self.root, self.active) {
            leaf.tabs.push(sid);
            leaf.active = leaf.tabs.len() - 1;
            self.active = sid;
        }
    }

    /// Remove a session; returns `true` when the whole tree became empty.
    pub fn close(&mut self, sid: usize) -> bool {
        let removed_active = sid == self.active;
        let neighbour = self.neighbour_of(sid);
        let empty = close_node(&mut self.root, sid);
        if empty {
            self.active = usize::MAX;
            return true;
        }
        if removed_active {
            self.active = neighbour.unwrap_or_else(|| self.sessions()[0]);
            focus_tab(&mut self.root, self.active);
        }
        false
    }

    pub fn cycle_tab(&mut self, forward: bool) {
        let Some(leaf) = leaf_with(&mut self.root, self.active) else {
            return;
        };
        if leaf.tabs.len() < 2 {
            return;
        }
        let len = leaf.tabs.len();
        leaf.active = if forward {
            (leaf.active + 1) % len
        } else {
            (leaf.active + len - 1) % len
        };
        self.active = leaf.current();
    }

    /// Select tab `n` (0-based) in the focused pane.
    pub fn select_tab(&mut self, n: usize) {
        let Some(leaf) = leaf_with(&mut self.root, self.active) else {
            return;
        };
        if n < leaf.tabs.len() {
            leaf.active = n;
            self.active = leaf.current();
        }
    }

    fn neighbour_of(&self, sid: usize) -> Option<usize> {
        let all = self.sessions();
        let idx = all.iter().position(|s| *s == sid)?;
        all.get(idx.wrapping_sub(1))
            .or_else(|| all.get(idx + 1))
            .copied()
    }

    /// Split the tree into leaf rectangles inside `rect`.
    pub fn rects(&self, rect: Rect) -> Vec<PaneRect<'_>> {
        let mut out = Vec::new();
        node_rects(&self.root, rect, &mut out);
        out
    }
}

fn collect(node: &Node, out: &mut Vec<usize>) {
    match node {
        Node::Leaf(leaf) => out.extend(leaf.tabs.iter().copied()),
        Node::Split { first, second, .. } => {
            collect(first, out);
            collect(second, out);
        }
    }
}

fn node_contains(node: &Node, sid: usize) -> bool {
    match node {
        Node::Leaf(leaf) => leaf.tabs.contains(&sid),
        Node::Split { first, second, .. } => {
            node_contains(first, sid) || node_contains(second, sid)
        }
    }
}

fn focus_tab(node: &mut Node, sid: usize) {
    if let Node::Leaf(leaf) = node {
        if let Some(i) = leaf.tabs.iter().position(|s| *s == sid) {
            leaf.active = i;
        }
        return;
    }
    if let Node::Split { first, second, .. } = node {
        focus_tab(first, sid);
        focus_tab(second, sid);
    }
}

fn leaf_with(node: &mut Node, sid: usize) -> Option<&mut Leaf> {
    match node {
        Node::Leaf(leaf) => leaf.tabs.contains(&sid).then_some(leaf),
        Node::Split { first, second, .. } => {
            leaf_with(first, sid).or_else(|| leaf_with(second, sid))
        }
    }
}

fn split_node(node: &mut Node, dir: Dir, sid: usize, new_sid: usize) -> bool {
    match node {
        Node::Leaf(leaf) => {
            if !leaf.tabs.contains(&sid) {
                return false;
            }
            let old = std::mem::replace(node, Node::Leaf(Leaf::new(sid)));
            *node = Node::Split {
                dir,
                ratio: 0.5,
                first: Box::new(old),
                second: Box::new(Node::Leaf(Leaf::new(new_sid))),
            };
            true
        }
        Node::Split { first, second, .. } => {
            split_node(first, dir, sid, new_sid) || split_node(second, dir, sid, new_sid)
        }
    }
}

/// Drop `sid`; collapse a split whose side became empty.
fn close_node(node: &mut Node, sid: usize) -> bool {
    match node {
        Node::Leaf(leaf) => {
            if let Some(i) = leaf.tabs.iter().position(|s| *s == sid) {
                leaf.tabs.remove(i);
                if leaf.active >= leaf.tabs.len() {
                    leaf.active = leaf.tabs.len().saturating_sub(1);
                }
            }
            leaf.tabs.is_empty()
        }
        Node::Split { first, second, .. } => {
            let in_first = node_contains(first, sid);
            let empty = if in_first {
                close_node(first, sid)
            } else {
                close_node(second, sid)
            };
            if !empty {
                return false;
            }
            let empty_leaf = Node::Leaf(Leaf {
                tabs: Vec::new(),
                active: 0,
            });
            let keep = if in_first {
                std::mem::replace(second.as_mut(), empty_leaf)
            } else {
                std::mem::replace(first.as_mut(), empty_leaf)
            };
            *node = keep;
            matches!(node, Node::Leaf(leaf) if leaf.tabs.is_empty())
        }
    }
}

fn node_rects<'a>(node: &'a Node, rect: Rect, out: &mut Vec<PaneRect<'a>>) {
    match node {
        Node::Leaf(leaf) => {
            if !leaf.tabs.is_empty() {
                out.push(PaneRect { leaf, rect });
            }
        }
        Node::Split {
            dir,
            ratio,
            first,
            second,
        } => {
            let ratio = ratio.clamp(0.1, 0.9);
            let (a, b) = match dir {
                Dir::Right | Dir::Left => {
                    let w1 = ((rect.w as f32) * ratio).round().max(1.0) as u32;
                    let w1 = w1.min(rect.w.saturating_sub(1)).max(1);
                    let left = Rect {
                        x: rect.x,
                        y: rect.y,
                        w: w1,
                        h: rect.h,
                    };
                    let right = Rect {
                        x: rect.x + w1 as i32,
                        y: rect.y,
                        w: rect.w - w1,
                        h: rect.h,
                    };
                    (left, right)
                }
                Dir::Down | Dir::Up => {
                    let h1 = ((rect.h as f32) * ratio).round().max(1.0) as u32;
                    let h1 = h1.min(rect.h.saturating_sub(1)).max(1);
                    let top = Rect {
                        x: rect.x,
                        y: rect.y,
                        w: rect.w,
                        h: h1,
                    };
                    let bottom = Rect {
                        x: rect.x,
                        y: rect.y + h1 as i32,
                        w: rect.w,
                        h: rect.h - h1,
                    };
                    (top, bottom)
                }
            };
            node_rects(first, a, out);
            node_rects(second, b, out);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rects(tree: &PaneTree) -> Vec<Rect> {
        tree.rects(Rect {
            x: 0,
            y: 0,
            w: 800,
            h: 600,
        })
        .into_iter()
        .map(|p| p.rect)
        .collect()
    }

    #[test]
    fn single_leaf_fills_the_window() {
        let tree = PaneTree::new(0);
        assert_eq!(
            rects(&tree),
            vec![Rect {
                x: 0,
                y: 0,
                w: 800,
                h: 600
            }]
        );
    }

    #[test]
    fn split_divides_along_the_requested_axis() {
        let mut tree = PaneTree::new(0);
        tree.split(Dir::Right, 1);
        let r = rects(&tree);
        assert_eq!(r.len(), 2);
        assert_eq!(r[0].x + r[0].w as i32, r[1].x);
        assert_eq!(r[0].h, 600);
        assert_eq!(r[0].w + r[1].w, 800);

        let mut tree = PaneTree::new(0);
        tree.split(Dir::Down, 1);
        let r = rects(&tree);
        assert_eq!(r[0].y + r[0].h as i32, r[1].y);
        assert_eq!(r[0].h + r[1].h, 600);
    }

    #[test]
    fn tabs_stay_inside_one_leaf() {
        let mut tree = PaneTree::new(0);
        tree.split(Dir::Right, 1);
        tree.set_active(0);
        tree.new_tab(2);
        assert_eq!(rects(&tree).len(), 2, "a tab must not create a pane");
        assert_eq!(tree.sessions(), vec![0, 2, 1]);
        assert_eq!(tree.active(), 2);
    }

    #[test]
    fn closing_collapses_the_split() {
        let mut tree = PaneTree::new(0);
        tree.split(Dir::Right, 1);
        assert!(!tree.close(1));
        assert_eq!(rects(&tree).len(), 1);
        assert_eq!(tree.active(), 0);

        assert!(tree.close(0));
        assert!(tree.sessions().is_empty());
    }

    #[test]
    fn closing_the_active_tab_moves_focus() {
        let mut tree = PaneTree::new(0);
        tree.new_tab(1);
        assert_eq!(tree.active(), 1);
        assert!(!tree.close(1));
        assert_eq!(tree.active(), 0);
        assert!(tree.close(0), "the last tab empties the tree");
        assert_eq!(tree.active(), usize::MAX);
        assert!(tree.close(0), "an already empty tree stays empty");
    }

    #[test]
    fn cycle_wraps_and_reports_neighbours() {
        let mut tree = PaneTree::new(0);
        tree.new_tab(1);
        tree.new_tab(2);
        assert_eq!(tree.active(), 2);
        tree.cycle_tab(true);
        assert_eq!(tree.active(), 0);
        tree.cycle_tab(false);
        assert_eq!(tree.active(), 2);
        tree.select_tab(1);
        assert_eq!(tree.active(), 1);
    }
}
