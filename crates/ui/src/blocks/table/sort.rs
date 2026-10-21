//! Row order of a sorted table column.
//!
//! Numbers compare numerically, text lexicographically and booleans as
//! `false < true`; a column mixing kinds groups them as booleans, numbers,
//! then text. Nulls (and missing cells) always go last, in either
//! direction. The sort is stable, so equal cells keep their original order.
//! Clicking a header cycles descending → ascending → original ([`cycle`]).

use std::cmp::Ordering;

use domain::block::Cell;

/// Direction of a column sort.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// Smallest first.
    Ascending,
    /// Largest first.
    Descending,
}

fn rank(cell: &Cell) -> u8 {
    match cell {
        Cell::Bool(_) => 0,
        Cell::Number(_) => 1,
        Cell::Text(_) => 2,
        Cell::Null => 3,
    }
}

/// Compares two non-null cells in ascending order.
fn compare_values(a: &Cell, b: &Cell) -> Ordering {
    match (a, b) {
        (Cell::Number(a), Cell::Number(b)) => a.total_cmp(b),
        (Cell::Text(a), Cell::Text(b)) => a.cmp(b),
        (Cell::Bool(a), Cell::Bool(b)) => a.cmp(b),
        _ => rank(a).cmp(&rank(b)),
    }
}

/// Compares two cells for `direction`, nulls last either way.
pub fn compare(a: &Cell, b: &Cell, direction: Direction) -> Ordering {
    match (a, b) {
        (Cell::Null, Cell::Null) => Ordering::Equal,
        (Cell::Null, _) => Ordering::Greater,
        (_, Cell::Null) => Ordering::Less,
        _ => match direction {
            Direction::Ascending => compare_values(a, b),
            Direction::Descending => compare_values(b, a),
        },
    }
}

/// Indices of `rows` ordered by column `col`. A row without that cell
/// counts as null.
pub fn sorted_order(rows: &[Vec<Cell>], col: usize, direction: Direction) -> Vec<usize> {
    const NULL: Cell = Cell::Null;
    let mut order: Vec<usize> = (0..rows.len()).collect();
    order.sort_by(|&a, &b| {
        let a = rows[a].get(col).unwrap_or(&NULL);
        let b = rows[b].get(col).unwrap_or(&NULL);
        compare(a, b, direction)
    });
    order
}

/// The sort after clicking column `col`'s header while sorted by
/// `current`: another column sorts descending, then a click switches to
/// ascending, then back to the original order (`None`).
pub fn cycle(current: Option<(usize, Direction)>, col: usize) -> Option<(usize, Direction)> {
    match current {
        Some((sorted, Direction::Descending)) if sorted == col => Some((col, Direction::Ascending)),
        Some((sorted, Direction::Ascending)) if sorted == col => None,
        _ => Some((col, Direction::Descending)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn num(v: f64) -> Cell {
        Cell::Number(v)
    }

    fn text(s: &str) -> Cell {
        Cell::Text(s.into())
    }

    fn column(cells: Vec<Cell>) -> Vec<Vec<Cell>> {
        cells.into_iter().map(|c| vec![c]).collect()
    }

    #[test]
    fn numbers_sort_numerically_not_as_text() {
        let rows = column(vec![num(10.0), num(9.0), num(100.0), num(-1.5)]);
        assert_eq!(sorted_order(&rows, 0, Direction::Ascending), [3, 1, 0, 2]);
        assert_eq!(sorted_order(&rows, 0, Direction::Descending), [2, 0, 1, 3]);
    }

    #[test]
    fn text_sorts_lexicographically() {
        let rows = column(vec![text("b"), text("B"), text("a10"), text("a9")]);
        assert_eq!(sorted_order(&rows, 0, Direction::Ascending), [1, 2, 3, 0]);
    }

    #[test]
    fn nulls_and_missing_cells_go_last_both_ways() {
        let mut rows = column(vec![Cell::Null, num(2.0), num(1.0), Cell::Null]);
        rows.push(Vec::new());
        assert_eq!(
            sorted_order(&rows, 0, Direction::Ascending),
            [2, 1, 0, 3, 4]
        );
        assert_eq!(
            sorted_order(&rows, 0, Direction::Descending),
            [1, 2, 0, 3, 4]
        );
    }

    #[test]
    fn equal_cells_keep_their_order_and_kinds_group() {
        let rows = column(vec![
            text("x"),
            num(1.0),
            Cell::Bool(true),
            num(1.0),
            Cell::Bool(false),
        ]);
        assert_eq!(
            sorted_order(&rows, 0, Direction::Ascending),
            [4, 2, 1, 3, 0]
        );
    }

    #[test]
    fn sorts_by_the_requested_column() {
        let rows = vec![
            vec![text("a"), num(3.0)],
            vec![text("b"), num(1.0)],
            vec![text("c"), num(2.0)],
        ];
        assert_eq!(sorted_order(&rows, 1, Direction::Ascending), [1, 2, 0]);
        assert_eq!(sorted_order(&rows, 7, Direction::Ascending), [0, 1, 2]);
    }

    #[test]
    fn header_clicks_cycle_descending_ascending_original() {
        let first = cycle(None, 2);
        assert_eq!(first, Some((2, Direction::Descending)));
        let second = cycle(first, 2);
        assert_eq!(second, Some((2, Direction::Ascending)));
        assert_eq!(cycle(second, 2), None);
        assert_eq!(cycle(second, 0), Some((0, Direction::Descending)));
    }
}
