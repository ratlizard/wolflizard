//! Architecture-neutral List Manager records.

use std::collections::{BTreeSet, HashMap};

/// Canonical host-side state for one guest `ListRec`.
///
/// The relocatable list record and cell-data handle remain guest-visible, but
/// the List Manager's logical cells, selection, geometry, and click state
/// belong to the Macintosh process rather than either CPU adapter. More
/// Macintosh Toolbox (1993), pp. 4-3--4-7 and 4-70--4-76.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProcessListRecord {
    pub(crate) handle: u32,
    pub(crate) cells_handle: u32,
    pub(crate) view_rect: (i16, i16, i16, i16),
    pub(crate) data_bounds: (i16, i16, i16, i16),
    pub(crate) cell_size: (i16, i16),
    pub(crate) visible: (i16, i16, i16, i16),
    pub(crate) port: u32,
    pub(crate) draw_enabled: bool,
    pub(crate) active: bool,
    pub(crate) cells: HashMap<(i16, i16), Vec<u8>>,
    pub(crate) selected: BTreeSet<(i16, i16)>,
    pub(crate) last_click: (i16, i16),
    pub(crate) last_click_tick: u32,
}

/// The guest's copy of a list's cells as last written: the `cellArray` words
/// and the `cells` handle's bytes.
pub(crate) type ListGuestImage = (Vec<u16>, Vec<u8>);

impl ProcessListRecord {
    /// Rows times columns of `dataBounds`.
    pub(crate) fn cell_count(&self) -> usize {
        let columns = usize::try_from(i32::from(self.data_bounds.3) - i32::from(self.data_bounds.1))
            .unwrap_or(0);
        let rows = usize::try_from(i32::from(self.data_bounds.2) - i32::from(self.data_bounds.0))
            .unwrap_or(0);
        columns.saturating_mul(rows)
    }

    /// The cell at `index` in the order the list record keeps them, row by
    /// row: index = row * columns + column, counted from `dataBounds`.
    pub(crate) fn cell_for_index(&self, index: usize) -> Option<(i16, i16)> {
        let columns = usize::try_from(i32::from(self.data_bounds.3) - i32::from(self.data_bounds.1))
            .unwrap_or(0);
        if columns == 0 || index >= self.cell_count() {
            return None;
        }
        Some((
            self.data_bounds.0.saturating_add((index / columns) as i16),
            self.data_bounds.1.saturating_add((index % columns) as i16),
        ))
    }

    /// The cells as the guest's `ListRec` holds them: `cellArray`, one word a
    /// cell in index order with the cell's selection in the high bit and
    /// where its data starts in the `cells` handle in the rest, and one word
    /// more where the last cell's data ends; and the data, packed in the same
    /// order. More Macintosh Toolbox (1993), pp. 4-72--4-73.
    pub(crate) fn guest_image(&self) -> ListGuestImage {
        let count = self.cell_count();
        let mut offsets = Vec::with_capacity(count + 1);
        let mut data = Vec::new();
        for index in 0..count {
            let cell = self.cell_for_index(index).expect("index is inside dataBounds");
            let selected = if self.selected.contains(&cell) { 0x8000 } else { 0 };
            offsets.push(selected | (data.len().min(0x7FFF) as u16));
            if let Some(bytes) = self.cells.get(&cell) {
                let room = 0x7FFF - data.len().min(0x7FFF);
                data.extend_from_slice(&bytes[..bytes.len().min(room)]);
            }
        }
        offsets.push(data.len().min(0x7FFF) as u16);
        (offsets, data)
    }

    /// Take the cells and their selection back from a guest copy, as an
    /// application that edits `**cells` in place leaves it; Cythera sorts a
    /// list's data that way. Nothing changes, and false is returned, when the
    /// copy is not one of this list's shape: a word a cell and one more,
    /// starts that never go back, and an end that is the data's length.
    pub(crate) fn absorb_guest_image(&mut self, offsets: &[u16], data: &[u8]) -> bool {
        let count = self.cell_count();
        if offsets.len() != count + 1 {
            return false;
        }
        let starts: Vec<usize> = offsets.iter().map(|word| usize::from(word & 0x7FFF)).collect();
        if starts.windows(2).any(|pair| pair[1] < pair[0]) || starts[count] != data.len() {
            return false;
        }
        let mut cells = HashMap::new();
        let mut selected = BTreeSet::new();
        for index in 0..count {
            let cell = self.cell_for_index(index).expect("index is inside dataBounds");
            let bytes = &data[starts[index]..starts[index + 1]];
            if !bytes.is_empty() {
                cells.insert(cell, bytes.to_vec());
            }
            if offsets[index] & 0x8000 != 0 {
                selected.insert(cell);
            }
        }
        self.cells = cells;
        self.selected = selected;
        true
    }

    /// LScroll is bounded by fully visible cells; a clipped last row must
    /// still be scrollable into full view. More Macintosh Toolbox, pp. 4-89--4-90;
    /// confirmed with 150-pixel views and 18-pixel rows on Mac OS 8.1.
    pub(crate) fn scrollbar_limits(&self, vertical: bool) -> (i16, i16, i16) {
        let (start, end, origin, pixels, cell) = if vertical {
            (
                self.data_bounds.0,
                self.data_bounds.2,
                self.visible.0,
                self.view_rect.2.saturating_sub(self.view_rect.0),
                self.cell_size.0,
            )
        } else {
            (
                self.data_bounds.1,
                self.data_bounds.3,
                self.visible.1,
                self.view_rect.3.saturating_sub(self.view_rect.1),
                self.cell_size.1,
            )
        };
        let page = (pixels.max(0) / cell.max(1)).max(1);
        let max = end.saturating_sub(page).max(start);
        (origin.clamp(start, max), start, max)
    }

    pub(crate) fn set_visible_origin(&mut self, row: i16, column: i16) {
        let (_, min_row, max_row) = self.scrollbar_limits(true);
        let (_, min_column, max_column) = self.scrollbar_limits(false);
        let top = row.clamp(min_row, max_row);
        let left = column.clamp(min_column, max_column);
        let extent = |pixels: i16, cell: i16| {
            ((i32::from(pixels.max(0)) + i32::from(cell.max(1)) - 1) / i32::from(cell.max(1)))
                .max(1)
                .min(i32::from(i16::MAX)) as i16
        };
        let rows = extent(
            self.view_rect.2.saturating_sub(self.view_rect.0),
            self.cell_size.0,
        );
        let columns = extent(
            self.view_rect.3.saturating_sub(self.view_rect.1),
            self.cell_size.1,
        );
        self.visible = (
            top,
            left,
            top.saturating_add(rows).min(self.data_bounds.2),
            left.saturating_add(columns).min(self.data_bounds.3),
        );
    }
}

/// Process-owned List Manager state keyed by guest `ListHandle`.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ProcessListManagerState {
    records: HashMap<u32, ProcessListRecord>,
    /// What the 68K dispatcher last wrote into each list's `cellArray` and
    /// `cells`, so a copy that differs is known to be the application's own
    /// edit, and one that does not leaves the record, which may be newer,
    /// alone.
    guest_images: HashMap<u32, ListGuestImage>,
}

impl ProcessListManagerState {
    pub(crate) fn is_pristine(&self) -> bool {
        self.records.is_empty() && self.guest_images.is_empty()
    }

    pub(crate) fn insert_record(&mut self, handle: u32, record: ProcessListRecord) {
        self.records.insert(handle, record);
    }

    pub(crate) fn remove_record(&mut self, handle: u32) -> Option<ProcessListRecord> {
        self.guest_images.remove(&handle);
        self.records.remove(&handle)
    }

    pub(crate) fn guest_image(&self, handle: u32) -> Option<&ListGuestImage> {
        self.guest_images.get(&handle)
    }

    pub(crate) fn set_guest_image(&mut self, handle: u32, image: ListGuestImage) {
        self.guest_images.insert(handle, image);
    }

    pub(crate) fn with_record_mut<R>(
        &mut self,
        handle: u32,
        f: impl FnOnce(&mut ProcessListRecord) -> R,
    ) -> Option<R> {
        self.records.get_mut(&handle).map(f)
    }

    pub(crate) fn with_record_ref<R>(
        &self,
        handle: u32,
        f: impl FnOnce(&ProcessListRecord) -> R,
    ) -> Option<R> {
        self.records.get(&handle).map(f)
    }

    pub(crate) fn get_record(&self, handle: u32) -> Option<ProcessListRecord> {
        self.records.get(&handle).cloned()
    }

    #[cfg(test)]
    pub(crate) fn contains_handle(&self, handle: u32) -> bool {
        self.records.contains_key(&handle)
    }

    pub(crate) fn records(&self) -> Vec<ProcessListRecord> {
        self.records.values().cloned().collect()
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.records.len()
    }

    #[cfg(test)]
    pub(crate) fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    #[allow(dead_code)]
    pub(crate) fn clear(&mut self) {
        self.records.clear();
        self.guest_images.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clipped_cells_can_scroll_fully_into_view_and_back() {
        let mut list = ProcessListRecord {
            handle: 0,
            cells_handle: 0,
            view_rect: (78, 24, 228, 528),
            data_bounds: (0, 0, 12, 1),
            cell_size: (18, 504),
            visible: (0, 0, 9, 1),
            port: 0,
            draw_enabled: true,
            active: true,
            cells: HashMap::new(),
            selected: BTreeSet::new(),
            last_click: (0, 0),
            last_click_tick: 0,
        };
        assert_eq!(list.scrollbar_limits(true), (0, 0, 4));
        list.set_visible_origin(4, 0);
        assert_eq!(list.visible, (4, 0, 12, 1));
        list.set_visible_origin(100, 0);
        assert_eq!(list.visible, (4, 0, 12, 1));
        list.set_visible_origin(0, 0);
        assert_eq!(list.visible, (0, 0, 9, 1));
        list.view_rect = (78, 24, 192, 474);
        list.set_visible_origin(4, 0);
        assert_eq!(list.visible, (4, 0, 11, 1));
        assert_eq!(list.scrollbar_limits(true), (4, 0, 6));
        list.set_visible_origin(100, 100);
        assert_eq!(list.visible, (6, 0, 12, 1));
    }

    #[test]
    fn process_list_manager_state_encapsulation() {
        let mut state = ProcessListManagerState::default();
        assert!(state.is_pristine());
        assert!(state.is_empty());
        assert_eq!(state.len(), 0);

        let record = ProcessListRecord {
            handle: 0x1000,
            cells_handle: 0x2000,
            view_rect: (0, 0, 40, 100),
            data_bounds: (0, 0, 2, 1),
            cell_size: (20, 100),
            visible: (0, 0, 2, 1),
            port: 0,
            draw_enabled: true,
            active: true,
            cells: HashMap::new(),
            selected: BTreeSet::new(),
            last_click: (0, 0),
            last_click_tick: 0,
        };
        state.insert_record(0x1000, record.clone());
        assert!(!state.is_pristine());
        assert!(!state.is_empty());
        assert_eq!(state.len(), 1);
        assert!(state.contains_handle(0x1000));
        assert_eq!(state.get_record(0x1000), Some(record.clone()));
        assert_eq!(
            state.with_record_ref(0x1000, |rec| rec.cells_handle),
            Some(0x2000)
        );
        state.with_record_mut(0x1000, |rec| rec.active = false);
        assert!(!state.get_record(0x1000).unwrap().active);
        assert_eq!(state.records().len(), 1);
        assert_eq!(state.remove_record(0x1000).unwrap().handle, 0x1000);
        assert!(state.is_empty());
    }

    fn two_by_three() -> ProcessListRecord {
        let mut cells = HashMap::new();
        cells.insert((0, 0), b"ab".to_vec());
        cells.insert((0, 2), b"c".to_vec());
        cells.insert((1, 1), b"def".to_vec());
        ProcessListRecord {
            handle: 0x1000,
            cells_handle: 0x2000,
            view_rect: (0, 0, 40, 90),
            data_bounds: (0, 0, 2, 3),
            cell_size: (20, 30),
            visible: (0, 0, 2, 3),
            port: 0,
            draw_enabled: false,
            active: true,
            cells,
            selected: [(1, 1)].into_iter().collect(),
            last_click: (0, 0),
            last_click_tick: 0,
        }
    }

    #[test]
    fn guest_image_packs_cells_row_by_row_with_selection_in_the_high_bit() {
        let (offsets, data) = two_by_three().guest_image();
        // (0,0) "ab", (0,1) empty, (0,2) "c", (1,0) empty, (1,1) "def" selected, (1,2) empty.
        assert_eq!(offsets, vec![0, 2, 2, 3, 0x8003, 6, 6]);
        assert_eq!(data, b"abcdef".to_vec());
    }

    #[test]
    fn absorb_guest_image_takes_an_in_place_edit_and_refuses_another_shape() {
        let mut list = two_by_three();
        let (mut offsets, mut data) = list.guest_image();
        // The application rewrites the bytes where they lie, leaving the
        // starts alone as a sort of equal-length cells does, and moves the
        // selection from (1,1) to (0,0).
        data = b"dec".iter().chain(b"ab".iter()).chain(b"f".iter()).copied().collect();
        offsets[0] |= 0x8000;
        offsets[4] &= 0x7FFF;
        assert!(list.absorb_guest_image(&offsets, &data));
        assert_eq!(list.cells.get(&(0, 0)), Some(&b"de".to_vec()));
        assert_eq!(list.cells.get(&(0, 2)), Some(&b"c".to_vec()));
        assert_eq!(list.cells.get(&(1, 1)), Some(&b"abf".to_vec()));
        assert_eq!(list.selected, [(0, 0)].into_iter().collect());

        let before = list.clone();
        assert!(!list.absorb_guest_image(&offsets[..6], &data));
        assert!(!list.absorb_guest_image(&offsets, &data[..5]));
        let mut backwards = offsets.clone();
        backwards[2] = 1;
        assert!(!list.absorb_guest_image(&backwards, &data));
        assert_eq!(list, before);
    }
}
