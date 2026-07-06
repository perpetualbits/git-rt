//! The recursive layout tree — rt's port of Terminator's split/tab containers.
//!
//! ## What we are porting
//! Terminator arranges terminals by nesting `Gtk.Paned` widgets (binary
//! splits) and `Gtk.Notebook` widgets (tabs), and it *physically reparents* the
//! VTE widgets as the user splits and closes panes. That reparenting during
//! live signal handlers is the source of a whole class of intermittent crashes
//! (see `docs/TERMINATOR_BUGS.md`, #2/#4/#5).
//!
//! ## How rt differs
//! Here the layout is a **pure immutable-ish data structure**. A `Tree` owns a
//! single root `Node`; leaves carry only a `PaneId` (an integer handle), never
//! a live terminal or widget. The GUI keeps the real panes in a side table
//! keyed by `PaneId`. Splitting/closing is ordinary tree surgery on plain data,
//! so there is nothing to "use after free". The renderer asks the tree for a
//! list of `(PaneId, Rect)` each frame; nothing is ever reparented.
//!
//! ## Model
//! * `Node::Leaf(id)` — a single terminal pane.
//! * `Node::Split { orient, children }` — an N-ary split (we only ever create
//!   binary splits, matching `Gtk.Paned`, but N-ary keeps collapse logic
//!   simple and lets a future "even-split" feature append siblings cheaply).
//! * `Node::Tabs { children, active }` — a tab bar; only the active child is
//!   visible/laid-out.

use crate::geom::Rect;

/// Width in logical pixels of the draggable gutter drawn between split
/// children. Subtracted from the available space before dividing it, so panes
/// never visually overlap the divider. Kept small; the renderer draws the
/// handle within this band.
const DIVIDER: f32 = 6.0;

/// Opaque identity of a pane (a leaf of the tree).
///
/// It is just a `u64` handle. The layout tree never dereferences it; only the
/// GUI's pane table does. Using an integer (not a pointer/`Rc`) is what makes
/// the tree trivially `Clone`, serialisable for saved layouts, and immune to
/// the aliasing hazards that bite Terminator.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PaneId(pub u64);

/// How a split arranges its children.
///
/// Named by the visible result, not by the divider's direction, to avoid the
/// perennial "horizontal split" ambiguity: `LeftRight` places children side by
/// side (a vertical divider between them); `TopBottom` stacks them (a
/// horizontal divider between them).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Orientation {
    LeftRight, // children laid out along the x axis
    TopBottom, // children laid out along the y axis
}

/// A focus-movement direction for keyboard pane navigation (Terminator's
/// Alt+Arrow / Ctrl+Shift+navigation). Resolved geometrically against the
/// current rectangles rather than by tree position, so it feels spatial.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    Left,
    Right,
    Up,
    Down,
}

/// One child slot inside a split, carrying its flexible size `weight`.
///
/// Weights are relative: a child gets `weight / sum_of_weights` of the space
/// along the split axis. Fresh splits use equal weights (`1.0` each), matching
/// Terminator's default 50/50 `Gtk.Paned`; dragging a divider later just
/// rewrites these numbers.
#[derive(Clone, Debug)]
struct Child {
    weight: f32, // relative flex factor along the split's orientation
    node: Node,  // the subtree occupying this slot
}

/// A node in the layout tree: a leaf pane, a split, or a tab group.
#[derive(Clone, Debug)]
enum Node {
    /// A single terminal pane identified by its handle.
    Leaf(PaneId),
    /// An N-ary split of children along one orientation.
    Split {
        orient: Orientation, // arrangement axis for `children`
        children: Vec<Child>, // ordered slots, first = left/top
    },
    /// A tab group: children stacked in z-order, only `active` is shown.
    Tabs {
        children: Vec<Node>, // one entry per tab
        active: usize,       // index into `children`; always kept in range
    },
}

/// The whole layout for one window: a root node plus a monotonically increasing
/// id counter used to mint fresh `PaneId`s.
///
/// `next_id` lives here (not globally) so each window's ids are independent and
/// a saved/restored layout can renumber cleanly.
#[derive(Clone, Debug)]
pub struct Tree {
    root: Node,   // the current arrangement
    next_id: u64, // next PaneId to hand out; monotonic, never reused
}

impl Tree {
    /// Create a tree containing exactly one pane and return the pair
    /// `(tree, first_pane_id)`.
    ///
    /// Every window starts life as a single full-window terminal, exactly like
    /// Terminator opening a bare window before any split.
    pub fn new() -> (Self, PaneId) {
        let first = PaneId(0); // the very first pane always gets id 0
        let tree = Tree {
            root: Node::Leaf(first), // root starts as a lone leaf
            next_id: 1,              // 0 is taken; hand out 1 next
        };
        (tree, first)
    }

    /// Mint a brand-new, never-before-used `PaneId`.
    ///
    /// Private helper used by every operation that introduces a pane. Ids are
    /// never recycled even after a pane closes, so a stale id can never
    /// accidentally alias a live pane (the same discipline that makes
    /// `deregister` idempotent in rt vs. Terminator's double-remove crash, #3).
    fn mint(&mut self) -> PaneId {
        let id = PaneId(self.next_id); // take the current counter value
        self.next_id += 1;             // advance so the next call differs
        id
    }

    /// Split the pane `target` in two, inserting a fresh pane beside it.
    ///
    /// Returns `Some(new_pane_id)` on success, or `None` if `target` is not a
    /// leaf in this tree (a caller passing a stale id — we degrade gracefully
    /// instead of panicking, per rt's no-crash policy).
    ///
    /// Semantics: the target leaf is replaced by a binary `Split` whose first
    /// child is the original pane and whose second child is the new pane, each
    /// weighted `1.0` (a 50/50 divide). This mirrors `Gtk.Paned`'s binary
    /// nesting precisely.
    pub fn split(&mut self, target: PaneId, orient: Orientation) -> Option<PaneId> {
        let new_id = self.mint(); // reserve the id up front so we can return it
        // Walk the tree mutating in place; only proceed if the target existed.
        if Self::split_node(&mut self.root, target, orient, new_id) {
            Some(new_id) // surgery succeeded
        } else {
            // Target wasn't found: we already advanced next_id, which is fine —
            // ids are allowed to have gaps; correctness only needs uniqueness.
            None
        }
    }

    /// Recursive worker for [`Tree::split`]. Returns `true` once it has found
    /// and replaced the target leaf somewhere in `node`'s subtree.
    ///
    /// Taking `&mut Node` lets us rewrite the matched leaf into a split in
    /// place. We stop at the first match (pane ids are unique, so there is at
    /// most one).
    fn split_node(node: &mut Node, target: PaneId, orient: Orientation, new_id: PaneId) -> bool {
        match node {
            // Base case: this leaf is the one to split.
            Node::Leaf(id) if *id == target => {
                let original = *id; // remember the pane we are wrapping
                // Replace the leaf with a two-slot split: original then new.
                *node = Node::Split {
                    orient,
                    children: vec![
                        Child { weight: 1.0, node: Node::Leaf(original) }, // keep old pane
                        Child { weight: 1.0, node: Node::Leaf(new_id) },   // add new pane
                    ],
                };
                true // found and handled
            }
            // A different leaf: not our target.
            Node::Leaf(_) => false,
            // Recurse into each split child until one reports success.
            Node::Split { children, .. } => {
                for child in children.iter_mut() {
                    if Self::split_node(&mut child.node, target, orient, new_id) {
                        return true; // short-circuit on first match
                    }
                }
                false // target not in this split
            }
            // Recurse into every tab page (target may live in an inactive tab).
            Node::Tabs { children, .. } => {
                for child in children.iter_mut() {
                    if Self::split_node(child, target, orient, new_id) {
                        return true;
                    }
                }
                false
            }
        }
    }

    /// Add a new tab as a sibling of `target`, wrapping in a `Tabs` node if the
    /// target is not already inside one. Returns the new pane's id, or `None`
    /// if `target` was not found.
    ///
    /// Port of Terminator's "new tab" (Ctrl+Shift+T): the new tab becomes the
    /// active one, matching Terminator's behaviour of focusing a freshly opened
    /// tab.
    pub fn new_tab(&mut self, target: PaneId) -> Option<PaneId> {
        let new_id = self.mint(); // reserve id for the tab's pane
        if Self::add_tab_node(&mut self.root, target, new_id) {
            Some(new_id)
        } else {
            None
        }
    }

    /// Recursive worker for [`Tree::new_tab`]. Returns `true` when handled.
    fn add_tab_node(node: &mut Node, target: PaneId, new_id: PaneId) -> bool {
        match node {
            // The target leaf is not yet tabbed: wrap it in a fresh Tabs node
            // with the new pane as a second, now-active tab.
            Node::Leaf(id) if *id == target => {
                let original = *id;
                *node = Node::Tabs {
                    children: vec![Node::Leaf(original), Node::Leaf(new_id)],
                    active: 1, // focus the newly added tab, like Terminator
                };
                true
            }
            Node::Leaf(_) => false,
            Node::Split { children, .. } => {
                for child in children.iter_mut() {
                    if Self::add_tab_node(&mut child.node, target, new_id) {
                        return true;
                    }
                }
                false
            }
            // If the target is a *direct* leaf child of this Tabs node, append
            // the new pane as another tab here rather than nesting a second
            // Tabs (keeps the tree flat and matches user expectation).
            Node::Tabs { children, active } => {
                // First, is the target one of our own top-level tab leaves?
                let direct = children
                    .iter()
                    .position(|c| matches!(c, Node::Leaf(id) if *id == target));
                if let Some(_) = direct {
                    children.push(Node::Leaf(new_id)); // append new tab page
                    *active = children.len() - 1;      // focus it
                    return true;
                }
                // Otherwise recurse into each page (target may be nested deeper).
                for child in children.iter_mut() {
                    if Self::add_tab_node(child, target, new_id) {
                        return true;
                    }
                }
                false
            }
        }
    }

    /// Remove the pane `target` from the tree, collapsing now-redundant
    /// containers. Returns `true` if a pane was removed.
    ///
    /// Collapse rules (the inverse of split/new_tab):
    /// * A split or tab group left with a single child is replaced by that
    ///   child (no lone one-way splits linger — Terminator does the same via
    ///   `Gtk.Paned` removal, but here it cannot crash mid-reparent).
    /// * Removing the last pane leaves the tree empty; the caller (window) is
    ///   expected to close the window in that case. We report success and leave
    ///   a sentinel the caller checks via [`Tree::is_empty`].
    pub fn close(&mut self, target: PaneId) -> bool {
        // `remove_from` returns an Option<Node>: Some(replacement) when the
        // subtree still has content, None when it became empty and should be
        // dropped by the parent. We seed it with the whole root.
        match Self::remove_from(std::mem::replace(&mut self.root, Node::Leaf(PaneId(u64::MAX))), target) {
            Removal::NotFound(node) => {
                self.root = node; // restore the untouched tree; nothing removed
                false
            }
            Removal::Replaced(Some(node)) => {
                self.root = node; // install the collapsed subtree
                true
            }
            Removal::Replaced(None) => {
                // The last pane was removed; mark the tree empty with a sentinel
                // leaf id that `is_empty` recognises.
                self.root = Node::Leaf(PaneId(u64::MAX));
                true
            }
        }
    }

    /// Whether the tree has been emptied (its final pane closed). The window
    /// layer should close the window when this becomes true.
    pub fn is_empty(&self) -> bool {
        // The sentinel installed by `close` is a leaf holding u64::MAX.
        matches!(self.root, Node::Leaf(PaneId(u64::MAX)))
    }

    /// Recursive worker for [`Tree::close`]. Consumes a node and returns a
    /// [`Removal`] describing whether the target was found and what the node
    /// should become afterwards.
    ///
    /// Taking the node by value (not `&mut`) makes the collapse cases natural:
    /// we can move a surviving single child *up* to replace its parent without
    /// fighting the borrow checker.
    fn remove_from(node: Node, target: PaneId) -> Removal {
        match node {
            // A leaf: removed if it is the target, otherwise handed back intact.
            Node::Leaf(id) if id == target => Removal::Replaced(None), // this pane is gone
            Node::Leaf(id) => Removal::NotFound(Node::Leaf(id)),       // unrelated leaf, untouched
            Node::Split { orient, mut children } => {
                // Try to remove the target from exactly one child slot.
                let mut hit = false; // did we find the target in this split?
                let mut i = 0; // manual index so we can drop a slot in place
                while i < children.len() {
                    match Self::remove_from(std::mem::replace(&mut children[i].node, Node::Leaf(PaneId(u64::MAX))), target) {
                        Removal::NotFound(orig) => {
                            children[i].node = orig; // put the untouched subtree back
                            i += 1; // advance to the next slot
                        }
                        Removal::Replaced(Some(new_node)) => {
                            children[i].node = new_node; // slot survived (smaller)
                            hit = true;
                            break; // target is unique; stop scanning
                        }
                        Removal::Replaced(None) => {
                            children.remove(i); // slot emptied: drop it entirely
                            hit = true;
                            break;
                        }
                    }
                }
                if !hit {
                    // Reassemble the split unchanged; target was elsewhere.
                    return Removal::NotFound(Node::Split { orient, children });
                }
                // Collapse rules after a successful removal:
                match children.len() {
                    0 => Removal::Replaced(None), // split emptied → tell parent to drop us
                    1 => Removal::Replaced(Some(children.pop().unwrap().node)), // lone child bubbles up
                    _ => Removal::Replaced(Some(Node::Split { orient, children })), // still a real split
                }
            }
            Node::Tabs { mut children, mut active } => {
                // Same one-slot removal scan as splits, but for tab pages.
                let mut hit = false;
                let mut i = 0;
                while i < children.len() {
                    match Self::remove_from(std::mem::replace(&mut children[i], Node::Leaf(PaneId(u64::MAX))), target) {
                        Removal::NotFound(orig) => {
                            children[i] = orig;
                            i += 1;
                        }
                        Removal::Replaced(Some(new_node)) => {
                            children[i] = new_node;
                            hit = true;
                            break;
                        }
                        Removal::Replaced(None) => {
                            children.remove(i); // remove the closed tab page
                            // Keep `active` valid: if we removed at/above it,
                            // shift it left, then clamp into range.
                            if active >= i && active > 0 {
                                active -= 1; // slide focus toward the start
                            }
                            hit = true;
                            break;
                        }
                    }
                }
                if !hit {
                    return Removal::NotFound(Node::Tabs { children, active });
                }
                match children.len() {
                    0 => Removal::Replaced(None), // no tabs left → drop the group
                    1 => Removal::Replaced(Some(children.pop().unwrap())), // one tab → unwrap it
                    _ => {
                        // Clamp active in case the last tab was removed.
                        if active >= children.len() {
                            active = children.len() - 1;
                        }
                        Removal::Replaced(Some(Node::Tabs { children, active }))
                    }
                }
            }
        }
    }

    /// Collect every pane and its pixel rectangle for the given window
    /// `bounds`, in stable left-to-right / top-to-bottom order.
    ///
    /// This is the function the renderer calls each frame: it turns the
    /// abstract tree into concrete blit targets. Inactive tab pages are omitted
    /// (they are not visible), which is why the result length can be smaller
    /// than the total pane count.
    pub fn rects(&self, bounds: Rect) -> Vec<(PaneId, Rect)> {
        if self.is_empty() {
            return Vec::new(); // an emptied tree draws nothing (window is closing)
        }
        let mut out = Vec::new(); // accumulator threaded through the recursion
        Self::layout_node(&self.root, bounds, &mut out); // fill it
        out
    }

    /// Recursive worker for [`Tree::rects`]. Divides `bounds` among `node`'s
    /// children according to orientation and weights, pushing `(PaneId, Rect)`
    /// for every visible leaf into `out`.
    fn layout_node(node: &Node, bounds: Rect, out: &mut Vec<(PaneId, Rect)>) {
        match node {
            // A leaf simply occupies its whole allotted rectangle.
            Node::Leaf(id) => out.push((*id, bounds)),
            Node::Split { orient, children } => {
                // Total weight normalises each child's share; guard against a
                // zero sum (shouldn't happen, but avoids a divide-by-zero NaN).
                let total: f32 = children.iter().map(|c| c.weight).sum();
                let total = if total > 0.0 { total } else { 1.0 };
                // Reserve gutters: n children need (n-1) dividers between them.
                let gutters = DIVIDER * (children.len().saturating_sub(1) as f32);
                match orient {
                    Orientation::LeftRight => {
                        let usable = (bounds.w - gutters).max(0.0); // width left for panes
                        let mut cursor = bounds.x; // running left edge
                        for child in children {
                            // This child's width is its share of the usable width.
                            let w = usable * (child.weight / total);
                            let r = Rect::new(cursor, bounds.y, w, bounds.h);
                            Self::layout_node(&child.node, r, out); // recurse into slot
                            cursor += w + DIVIDER; // advance past pane + gutter
                        }
                    }
                    Orientation::TopBottom => {
                        let usable = (bounds.h - gutters).max(0.0); // height for panes
                        let mut cursor = bounds.y; // running top edge
                        for child in children {
                            let h = usable * (child.weight / total); // this child's height
                            let r = Rect::new(bounds.x, cursor, bounds.w, h);
                            Self::layout_node(&child.node, r, out);
                            cursor += h + DIVIDER; // advance past pane + gutter
                        }
                    }
                }
            }
            // Only the active tab page is laid out; the rest are hidden.
            Node::Tabs { children, active } => {
                if let Some(child) = children.get(*active) {
                    // Reserve a tab strip at the top; the renderer draws tab
                    // labels there. TAB_STRIP height is a fixed band.
                    const TAB_STRIP: f32 = 24.0; // pixel height of the tab bar
                    let body = Rect::new(
                        bounds.x,
                        bounds.y + TAB_STRIP,               // push content below the strip
                        bounds.w,
                        (bounds.h - TAB_STRIP).max(0.0),    // remaining height
                    );
                    Self::layout_node(child, body, out);
                }
            }
        }
    }

    /// Every pane id in the tree, including panes on inactive tab pages, in
    /// traversal order. Useful for bookkeeping (e.g. reaping closed PTYs) where
    /// visibility does not matter.
    pub fn all_panes(&self) -> Vec<PaneId> {
        let mut out = Vec::new();
        Self::collect_panes(&self.root, &mut out); // walk the whole tree
        out
    }

    /// Recursive worker for [`Tree::all_panes`].
    fn collect_panes(node: &Node, out: &mut Vec<PaneId>) {
        match node {
            Node::Leaf(id) if *id != PaneId(u64::MAX) => out.push(*id), // skip the empty sentinel
            Node::Leaf(_) => {}                                          // sentinel: nothing to collect
            Node::Split { children, .. } => {
                for c in children {
                    Self::collect_panes(&c.node, out); // recurse each slot
                }
            }
            Node::Tabs { children, .. } => {
                for c in children {
                    Self::collect_panes(c, out); // recurse each tab page
                }
            }
        }
    }

    /// Find the pane the user would move focus to when pressing a directional
    /// navigation key from `from`. Returns `None` if there is no pane in that
    /// direction (an edge of the window).
    ///
    /// Algorithm (spatial, like tmux/i3): compute all visible rectangles for
    /// `bounds`, take the source pane's centre, then among panes that lie on the
    /// correct side pick the one whose centre is nearest. This gives intuitive
    /// movement regardless of how the tree happens to be nested.
    pub fn neighbor(&self, from: PaneId, dir: Direction, bounds: Rect) -> Option<PaneId> {
        let rects = self.rects(bounds); // visible panes only — you can't focus a hidden tab
        // Locate the source rectangle; if the source isn't visible, bail.
        let (_, src) = rects.iter().find(|(id, _)| *id == from)?;
        let (sx, sy) = src.center(); // source centre, our reference point
        let mut best: Option<(PaneId, f32)> = None; // (candidate, distance-so-far)
        for (id, r) in &rects {
            if *id == from {
                continue; // never navigate to yourself
            }
            let (cx, cy) = r.center(); // candidate centre
            // Keep only candidates strictly on the requested side of the source.
            let on_side = match dir {
                Direction::Left => cx < sx,
                Direction::Right => cx > sx,
                Direction::Up => cy < sy,
                Direction::Down => cy > sy,
            };
            if !on_side {
                continue; // wrong side; ignore
            }
            // Distance metric: straight-line distance between centres. Simple
            // and good enough; ties (rare) resolve to first-seen.
            let dist = (cx - sx).powi(2) + (cy - sy).powi(2); // squared distance (monotone)
            match best {
                Some((_, bd)) if dist >= bd => {} // not closer; keep current best
                _ => best = Some((*id, dist)),    // new nearest candidate
            }
        }
        best.map(|(id, _)| id) // strip the distance, return just the pane
    }
}

/// Result of removing a pane from a subtree, used by [`Tree::remove_from`].
///
/// * `NotFound(node)` — the target was not in this subtree; the node is handed
///   back unchanged (we moved it out to recurse, so we must return it).
/// * `Replaced(Some(node))` — the target was removed; `node` is what this
///   subtree collapsed to.
/// * `Replaced(None)` — the target was removed and this subtree is now empty;
///   the parent should drop the slot entirely.
enum Removal {
    NotFound(Node),
    Replaced(Option<Node>),
}

impl Default for Tree {
    /// A default `Tree` is a single-pane tree (id 0), matching [`Tree::new`].
    /// Provided so `Tree` slots into `#[derive(Default)]` structs cleanly.
    fn default() -> Self {
        Tree::new().0 // discard the returned first-pane id
    }
}
