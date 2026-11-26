//! Source-preserving edits for a complete Markdown pipe table.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Action {
    RowAbove,
    RowBelow,
    ColumnLeft,
    ColumnRight,
    DeleteRow,
    DeleteColumn,
}

/// `row` counts visible rows (header = 0), not the separator line.
/// Invalid coordinates, malformed/nonrectangular tables, and removing the
/// last column or the only header return `None` without changing the source.
pub(super) fn edit(raw: &str, row: usize, column: usize, action: Action) -> Option<String> {
    let ranges = super::line_ranges(raw);
    let mut lines: Vec<(String, String)> = ranges.iter().enumerate()
        .filter(|&(_, &(start, end))| start != raw.len() || start != end)
        .map(|(i, &(start, end))| {
            let next = ranges.get(i + 1).map_or(raw.len(), |r| r.0);
            (raw[start..end].to_owned(), raw[end..next].to_owned())
        }).collect();
    if lines.len() < 2 { return None; }
    let columns = super::table_cells(&lines[0].0).len();
    if columns == 0 || column >= columns || row >= lines.len() - 1
        || lines.iter().any(|line| super::table_cells(&line.0).len() != columns)
    { return None; }
    // Do not accidentally operate on arbitrary multiline text.
    if !super::table_cells(&lines[1].0).into_iter().all(|r| {
        let cell = lines[1].0[r].trim_matches(':');
        cell.len() >= 3 && cell.bytes().all(|b| b == b'-')
    }) { return None; }
    let source_row = if row == 0 { 0 } else { row + 1 };
    let newline = lines.iter().find(|l| !l.1.is_empty())?.1.clone();
    let trailing_newline = !lines.last()?.1.is_empty();
    match action {
        Action::RowAbove | Action::RowBelow => {
            let blank = empty_row(&lines[source_row].0);
            let position = source_row + usize::from(action == Action::RowBelow);
            lines.insert(position, (blank, newline.clone()));
            if action == Action::RowAbove && row == 0 {
                // New empty header, separator, old header, then existing data.
                lines.swap(1, 2);
            } else if action == Action::RowBelow && row == 0 {
                // A data row must follow (not precede) the separator.
                lines.swap(1, 2);
            }
        }
        Action::DeleteRow => {
            if row == 0 && lines.len() == 2 { return None; }
            lines.remove(source_row);
            if row == 0 { lines.swap(0, 1); }
        }
        Action::ColumnLeft | Action::ColumnRight | Action::DeleteColumn => {
            if action == Action::DeleteColumn && columns == 1 { return None; }
            for (index, line) in lines.iter_mut().enumerate() {
                line.0 = edit_column(&line.0, column, action, index == 1);
            }
        }
    }
    // An inserted last row needs a separator before it; deleting a last row
    // must not accidentally change whether the whole table has a final EOL.
    let last = lines.len() - 1;
    for (index, line) in lines.iter_mut().enumerate() {
        if index < last && line.1.is_empty() { line.1 = newline.clone(); }
        if index == last {
            if trailing_newline {
                if line.1.is_empty() { line.1 = newline.clone(); }
            } else { line.1.clear(); }
        }
    }
    Some(lines.into_iter().map(|(body, ending)| body + &ending).collect())
}

// Recover untrimmed cell segments from the parent's source ranges. Only gaps
// between cells are searched for delimiters, never escaped/code-span contents.
fn segments(line: &str) -> (Vec<std::ops::Range<usize>>, bool, bool) {
    let cells = super::table_cells(line);
    let first = &cells[0];
    let last = cells.last().unwrap();
    let left = line[..first.start].find('|');
    let right = line[last.end..].rfind('|').map(|p| last.end + p);
    let mut start = left.map_or(0, |p| p + 1);
    let mut segments = Vec::with_capacity(cells.len());
    for pair in cells.windows(2) {
        let delimiter = pair[0].end + line[pair[0].end..pair[1].start].find('|').unwrap();
        segments.push(start..delimiter);
        start = delimiter + 1;
    }
    segments.push(start..right.unwrap_or(line.len()));
    (segments, left.is_some(), right.is_some())
}

fn empty_row(line: &str) -> String {
    let (_, left, right) = segments(line);
    let mut out = line.to_owned();
    for range in super::table_cells(line).into_iter().rev() { out.replace_range(range, ""); }
    // Empty edge cells without outside pipes would be parsed as outside space.
    if !right { out.push('|'); }
    if !left { out.insert(0, '|'); }
    out
}

fn edit_column(line: &str, column: usize, action: Action, separator: bool) -> String {
    let (segments, left, right) = segments(line);
    let target = segments[column].clone();
    let mut out = line.to_owned();
    if action == Action::DeleteColumn {
        let removal = if column + 1 < segments.len() {
            target.start..segments[column + 1].start
        } else { segments[column - 1].end..target.end };
        out.replace_range(removal, "");
        // Keep surviving empty edge cells representable with no outside pipes.
        let cells = super::table_cells(line);
        let first = if column == 0 { 1 } else { 0 };
        let last = if column == cells.len() - 1 { cells.len() - 2 } else { cells.len() - 1 };
        if !right && cells[last].is_empty() { out.push('|'); }
        if !left && cells[first].is_empty() { out.insert(0, '|'); }
        // A one-column table still needs a structural pipe in its header.
        if segments.len() == 2 && !left && !right && !cells[last].is_empty() {
            out.push('|');
        }
    } else {
        let cell = &line[target.clone()];
        let leading = cell.len() - cell.trim_start().len();
        let trailing = (cell.len() - cell.trim_end().len()).min(cell.len() - leading);
        let blank = format!("{}{}{}", &cell[..leading], if separator { "---" } else { "" },
            &cell[cell.len() - trailing..]);
        if action == Action::ColumnLeft {
            out.insert_str(target.start, &(blank + "|"));
            if column == 0 && !left && !separator { out.insert(0, '|'); }
        } else {
            out.insert_str(target.end, &("|".to_owned() + &blank));
            if column + 1 == segments.len() && !right && !separator { out.push('|'); }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{edit, Action::*};

    #[test]
    fn all_six_actions() {
        let raw = "| A | B |\n| :--- | ---: |\n| x | y |\n";
        assert_eq!(edit(raw, 1, 0, RowAbove).unwrap(), "| A | B |\n| :--- | ---: |\n|  |  |\n| x | y |\n");
        assert_eq!(edit(raw, 1, 0, RowBelow).unwrap(), "| A | B |\n| :--- | ---: |\n| x | y |\n|  |  |\n");
        assert_eq!(edit(raw, 1, 1, ColumnLeft).unwrap(), "| A |  | B |\n| :--- | --- | ---: |\n| x |  | y |\n");
        assert_eq!(edit(raw, 1, 0, ColumnRight).unwrap(), edit(raw, 1, 1, ColumnLeft).unwrap());
        assert_eq!(edit(raw, 1, 0, DeleteRow).unwrap(), "| A | B |\n| :--- | ---: |\n");
        assert_eq!(edit(raw, 1, 0, DeleteColumn).unwrap(), "| B |\n| ---: |\n| y |\n");
    }

    #[test]
    fn header_insert_and_promotion() {
        let raw = "A | B\n--- | ---\nx | y";
        assert_eq!(edit(raw, 0, 0, RowAbove).unwrap(), "| | |\n--- | ---\nA | B\nx | y");
        assert_eq!(edit(raw, 0, 0, RowBelow).unwrap(), "A | B\n--- | ---\n| | |\nx | y");
        assert_eq!(edit(raw, 0, 0, DeleteRow).unwrap(), "x | y\n--- | ---");
        assert!(edit("A | B\n--- | ---", 0, 0, DeleteRow).is_none());
    }

    #[test]
    fn crlf_and_final_newline_are_preserved() {
        let raw = "| A | B |\r\n| --- | --- |\r\n| x | y |";
        assert_eq!(edit(raw, 1, 0, RowBelow).unwrap(), "| A | B |\r\n| --- | --- |\r\n| x | y |\r\n|  |  |");
        assert_eq!(edit(&(raw.to_owned() + "\r\n"), 1, 0, DeleteRow).unwrap(), "| A | B |\r\n| --- | --- |\r\n");
    }

    #[test]
    fn no_outer_pipes_preserves_existing_segments() {
        let raw = "A  | B\n:--- | ---:\nx | y";
        assert_eq!(edit(raw, 1, 1, DeleteColumn).unwrap(), "A  |\n:--- |\nx |");
        assert_eq!(edit(raw, 1, 0, ColumnLeft).unwrap(), "|  |A  | B\n--- |:--- | ---:\n| |x | y");
        let right = edit(raw, 1, 1, ColumnRight).unwrap();
        assert_eq!(right, "A  | B| |\n:--- | ---:| ---\nx | y| |");
    }

    #[test]
    fn escaped_and_code_pipes_are_not_delimiters() {
        let raw = "| a\\|b | `c|d` |\n| --- | --- |\n| ``e|`f`` | z\\|w |";
        assert_eq!(edit(raw, 1, 0, DeleteColumn).unwrap(), "| `c|d` |\n| --- |\n| z\\|w |");
        assert_eq!(edit(raw, 1, 1, DeleteColumn).unwrap(), "| a\\|b |\n| --- |\n| ``e|`f`` |");
        let inserted = edit(raw, 1, 1, ColumnLeft).unwrap();
        assert!(inserted.contains("| a\\|b |  | `c|d` |"));
        assert!(inserted.contains("| ``e|`f`` |  | z\\|w |"));
    }

    #[test]
    fn empty_cells_unicode_and_repeated_edits_remain_rectangular() {
        let raw = "| 名称 |   |\n| --- | --- |\n| a |   |";
        let inserted = edit(raw, 1, 1, ColumnRight).unwrap();
        assert!(inserted.contains("| 名称 |   |   |"));
        assert_eq!(super::super::table_cells(inserted.lines().next().unwrap()).len(), 3);
        for action in [RowAbove, RowBelow, ColumnLeft, ColumnRight, DeleteRow, DeleteColumn] {
            let changed = edit(raw, 1, 1, action).unwrap();
            let widths: Vec<_> = changed.lines().map(|l| super::super::table_cells(l).len()).collect();
            assert!(widths.iter().all(|width| *width == widths[0]), "{action:?}: {changed}");
        }
        let single = edit("A | B\n--- | ---\nx | y", 1, 1, DeleteColumn).unwrap();
        assert!(single.lines().next().unwrap().contains('|'));
        assert!(edit(&single, 1, 0, ColumnRight).is_some());
    }

    #[test]
    fn last_column_and_invalid_coordinates_are_protected() {
        let raw = "| A |\n| --- |\n| x |";
        assert!(edit(raw, 0, 0, DeleteColumn).is_none());
        assert!(edit(raw, 2, 0, RowAbove).is_none());
        assert!(edit(raw, 0, 1, ColumnRight).is_none());
        assert!(edit("not a table", 0, 0, RowAbove).is_none());
        assert!(edit("A | B\n--- | ---\nx", 0, 0, RowAbove).is_none());
    }
}
