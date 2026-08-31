use super::*;

pub(crate) const DIVIDER_THICKNESS: usize = 2;
pub(crate) const RATIO_MIN_PERMILLE: u32 = 200;
pub(crate) const RATIO_MAX_PERMILLE: u32 = 800;
pub(crate) const RATIO_STEP_PERMILLE: u32 = 50;
pub(crate) const RATIO_DEFAULT_PERMILLE: u32 = 500;

#[derive(Clone, Copy, Eq, PartialEq, Debug)]
pub(crate) enum SplitAxis {
    Columns,
    Rows,
}

#[derive(Clone, Copy, Eq, PartialEq, Debug)]
pub(crate) enum PaneDirection {
    Left,
    Right,
    Up,
    Down,
}

/// Binary pane tree per tab. With MAX_PANES_PER_TAB = 2 the tree is either a
/// single leaf or a root split into exactly two leaves.
#[derive(Clone, Copy, Debug)]
pub(crate) struct PaneTree {
    pub(crate) split: bool,
    pub(crate) axis: SplitAxis,
    pub(crate) ratio_permille: u32,
    pub(crate) focused: usize,
}

impl PaneTree {
    pub(crate) const fn single() -> Self {
        Self {
            split: false,
            axis: SplitAxis::Columns,
            ratio_permille: RATIO_DEFAULT_PERMILLE,
            focused: 0,
        }
    }

    pub(crate) fn open_split(&mut self, axis: SplitAxis) {
        self.split = true;
        self.axis = axis;
        self.ratio_permille = RATIO_DEFAULT_PERMILLE;
        self.focused = 1;
    }

    /// Close the split, keeping `keep` (0 or 1) as the single remaining pane.
    pub(crate) fn close_split(&mut self, keep: usize) {
        self.split = false;
        self.focused = keep.min(1);
    }

    /// Move focus toward a direction; only succeeds when the split axis matches.
    pub(crate) fn focus_direction(&mut self, direction: PaneDirection) -> bool {
        if !self.split {
            return false;
        }
        let matches = match (self.axis, direction) {
            (SplitAxis::Columns, PaneDirection::Left) => self.focused == 1,
            (SplitAxis::Columns, PaneDirection::Right) => self.focused == 0,
            (SplitAxis::Rows, PaneDirection::Up) => self.focused == 1,
            (SplitAxis::Rows, PaneDirection::Down) => self.focused == 0,
            _ => false,
        };
        if matches {
            self.focused = 1 - self.focused;
        }
        matches
    }

    pub(crate) fn resize_ratio(&mut self, delta_permille: i32) {
        let current = self.ratio_permille as i32;
        let next =
            (current + delta_permille).clamp(RATIO_MIN_PERMILLE as i32, RATIO_MAX_PERMILLE as i32);
        self.ratio_permille = next as u32;
    }
}

/// Ctrl+Alt+Shift+arrow on a split tab resizes the split ratio; the same
/// arrows without Shift move focus. Returns the permille delta for the
/// resize chord, None for any other combination.
pub(crate) fn pane_resize_delta(key_code: u32, modifiers: u32) -> Option<i32> {
    let chord = MOD_CTRL | MOD_ALT | MOD_SHIFT;
    if modifiers & chord != chord {
        return None;
    }
    match key_code {
        KEY_LEFT | KEY_UP => Some(-(RATIO_STEP_PERMILLE as i32)),
        KEY_RIGHT | KEY_DOWN => Some(RATIO_STEP_PERMILLE as i32),
        _ => None,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct PixelRect {
    pub(crate) x: usize,
    pub(crate) y: usize,
    pub(crate) w: usize,
    pub(crate) h: usize,
}

impl PixelRect {
    pub(crate) const fn zero() -> Self {
        Self {
            x: 0,
            y: 0,
            w: 0,
            h: 0,
        }
    }

    pub(crate) fn contains(&self, x: i32, y: i32) -> bool {
        x >= self.x as i32
            && y >= self.y as i32
            && (x as usize) < self.x.saturating_add(self.w)
            && (y as usize) < self.y.saturating_add(self.h)
    }
}

/// Split `area` into two pane rects plus the divider gap between them.
/// Returns [first, second, divider].
pub(crate) fn pane_rects(area: PixelRect, tree: &PaneTree) -> [PixelRect; 3] {
    if !tree.split {
        return [area, PixelRect::zero(), PixelRect::zero()];
    }
    match tree.axis {
        SplitAxis::Columns => {
            let usable = area.w.saturating_sub(DIVIDER_THICKNESS);
            let first_w = (usable * tree.ratio_permille as usize / 1000).min(usable);
            let first = PixelRect {
                x: area.x,
                y: area.y,
                w: first_w,
                h: area.h,
            };
            let divider = PixelRect {
                x: area.x.saturating_add(first_w),
                y: area.y,
                w: DIVIDER_THICKNESS.min(area.w.saturating_sub(first_w)),
                h: area.h,
            };
            let second = PixelRect {
                x: area
                    .x
                    .saturating_add(first_w)
                    .saturating_add(DIVIDER_THICKNESS),
                y: area.y,
                w: area
                    .w
                    .saturating_sub(first_w)
                    .saturating_sub(DIVIDER_THICKNESS),
                h: area.h,
            };
            [first, second, divider]
        }
        SplitAxis::Rows => {
            let usable = area.h.saturating_sub(DIVIDER_THICKNESS);
            let first_h = (usable * tree.ratio_permille as usize / 1000).min(usable);
            let first = PixelRect {
                x: area.x,
                y: area.y,
                w: area.w,
                h: first_h,
            };
            let divider = PixelRect {
                x: area.x,
                y: area.y.saturating_add(first_h),
                w: area.w,
                h: DIVIDER_THICKNESS.min(area.h.saturating_sub(first_h)),
            };
            let second = PixelRect {
                x: area.x,
                y: area
                    .y
                    .saturating_add(first_h)
                    .saturating_add(DIVIDER_THICKNESS),
                w: area.w,
                h: area
                    .h
                    .saturating_sub(first_h)
                    .saturating_sub(DIVIDER_THICKNESS),
            };
            [first, second, divider]
        }
    }
}

/// Character grid size that fits a pixel rect, mirroring the window-level clamps.
pub(crate) fn grid_dims_for(rect: PixelRect) -> (usize, usize) {
    let cols = (rect.w / CELL_WIDTH).clamp(8, MAX_COLS);
    let rows = (rect.h / CELL_HEIGHT).clamp(4, MAX_SCROLLBACK_LINES);
    (cols, rows)
}

pub(crate) fn clear_pane_grid(slot: usize) {
    let lines = unsafe { GRIDS.pane_mut(slot) };
    let wraps = unsafe { WRAPS.wraps_mut(slot) };
    for row in lines.iter_mut() {
        for cell in row.iter_mut() {
            *cell = Cell::blank();
        }
    }
    wraps.fill(false);
}

/// Move grid contents `from_slot` -> `to_slot` (to_slot is overwritten),
/// using the reflow scratch buffers as staging. `from_slot` ends cleared.
pub(crate) fn move_pane_grid(from_slot: usize, to_slot: usize) {
    if from_slot == to_slot {
        return;
    }
    unsafe {
        let scratch = REFLOW_CELLS.get();
        let scratch_wraps = REFLOW_WRAPS.get();
        scratch.copy_from_slice(GRIDS.pane(from_slot));
        scratch_wraps.copy_from_slice(WRAPS.wraps_mut(from_slot));
        clear_pane_grid(from_slot);
        let destination = GRIDS.pane_mut(to_slot);
        destination.copy_from_slice(scratch);
        WRAPS.wraps_mut(to_slot).copy_from_slice(scratch_wraps);
    }
}

pub(crate) fn content_area(state: &TerminalState) -> PixelRect {
    PixelRect {
        x: state.content_x,
        y: state.content_y,
        w: state.content_w,
        h: state.content_h,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const AREA: PixelRect = PixelRect {
        x: 10,
        y: 20,
        w: 120,
        h: 60,
    };

    #[test]
    fn single_tree_renders_full_area() {
        let tree = PaneTree::single();
        let rects = pane_rects(AREA, &tree);
        assert_eq!(rects[0], AREA);
        assert_eq!(rects[1], PixelRect::zero());
    }

    #[test]
    fn split_columns_divides_with_divider() {
        let mut tree = PaneTree::single();
        tree.open_split(SplitAxis::Columns);
        let [first, second, divider] = pane_rects(AREA, &tree);
        assert_eq!(first.x, 10);
        assert_eq!(first.w, 59);
        assert_eq!(divider.w, DIVIDER_THICKNESS);
        assert_eq!(second.x, first.x + first.w + DIVIDER_THICKNESS);
        assert_eq!(second.w, AREA.w - first.w - DIVIDER_THICKNESS);
        assert_eq!(first.h, AREA.h);
        assert_eq!(tree.focused, 1);
    }

    #[test]
    fn split_rows_divides_vertically() {
        let mut tree = PaneTree::single();
        tree.open_split(SplitAxis::Rows);
        let [first, second, _] = pane_rects(AREA, &tree);
        assert_eq!(first.h, 29);
        assert_eq!(second.y, AREA.y + first.h + DIVIDER_THICKNESS);
        assert_eq!(second.h, AREA.h - first.h - DIVIDER_THICKNESS);
        assert_eq!(first.w, AREA.w);
    }

    #[test]
    fn resize_ratio_clamps_to_bounds() {
        let mut tree = PaneTree::single();
        tree.open_split(SplitAxis::Columns);
        tree.resize_ratio(10_000);
        assert_eq!(tree.ratio_permille, RATIO_MAX_PERMILLE);
        tree.resize_ratio(-10_000);
        assert_eq!(tree.ratio_permille, RATIO_MIN_PERMILLE);
        tree.resize_ratio(0);
        assert_eq!(tree.ratio_permille, RATIO_MIN_PERMILLE);
        tree.resize_ratio(RATIO_STEP_PERMILLE as i32);
        assert_eq!(
            tree.ratio_permille,
            RATIO_MIN_PERMILLE + RATIO_STEP_PERMILLE
        );
    }

    #[test]
    fn focus_direction_follows_axis() {
        let mut tree = PaneTree::single();
        tree.open_split(SplitAxis::Columns);
        assert!(tree.focus_direction(PaneDirection::Left));
        assert_eq!(tree.focused, 0);
        assert!(!tree.focus_direction(PaneDirection::Up));
        assert_eq!(tree.focused, 0);
        assert!(tree.focus_direction(PaneDirection::Right));
        assert_eq!(tree.focused, 1);
        tree.close_split(0);
        assert!(!tree.split);
        assert_eq!(tree.focused, 0);
        assert!(!tree.focus_direction(PaneDirection::Right));
    }

    #[test]
    fn close_split_keeps_requested_pane() {
        let mut tree = PaneTree::single();
        tree.open_split(SplitAxis::Rows);
        tree.focused = 0;
        tree.close_split(1);
        assert_eq!(tree.focused, 1);
        assert!(!tree.split);
    }

    #[test]
    fn grid_slot_maps_tab_and_pane() {
        assert_eq!(grid_slot(0, 0), 0);
        assert_eq!(grid_slot(1, 1), 3);
        assert_eq!(grid_slot(3, 1), 7);
        assert_eq!(GRID_SLOTS, 8);
    }

    #[test]
    fn grid_dims_respect_rect_and_clamps() {
        assert_eq!(grid_dims_for(AREA), (20, 6));
        let tiny = PixelRect {
            x: 0,
            y: 0,
            w: 4,
            h: 4,
        };
        assert_eq!(grid_dims_for(tiny), (8, 4));
        let huge = PixelRect {
            x: 0,
            y: 0,
            w: 4000,
            h: 5000,
        };
        assert_eq!(grid_dims_for(huge), (MAX_COLS, MAX_SCROLLBACK_LINES));
    }

    #[test]
    fn pane_resize_delta_key_matrix() {
        let chord = MOD_CTRL | MOD_ALT | MOD_SHIFT;
        assert_eq!(
            pane_resize_delta(KEY_LEFT, chord),
            Some(-(RATIO_STEP_PERMILLE as i32))
        );
        assert_eq!(
            pane_resize_delta(KEY_UP, chord),
            Some(-(RATIO_STEP_PERMILLE as i32))
        );
        assert_eq!(
            pane_resize_delta(KEY_RIGHT, chord),
            Some(RATIO_STEP_PERMILLE as i32)
        );
        assert_eq!(
            pane_resize_delta(KEY_DOWN, chord),
            Some(RATIO_STEP_PERMILLE as i32)
        );
        // Missing any chord modifier is not a resize (Ctrl+Alt+arrow is the
        // focus path; plain arrows are view scroll).
        assert_eq!(pane_resize_delta(KEY_LEFT, MOD_CTRL | MOD_ALT), None);
        assert_eq!(pane_resize_delta(KEY_LEFT, 0), None);
        // Non-arrow keys stay unmatched even with the full chord.
        assert_eq!(pane_resize_delta(KEY_Q, chord), None);
    }
}
