//! Column-aligned text tables for human output.

use std::io::{self, Write};

use super::sanitize::printable;

const COLUMN_GAP: &str = "  ";

/// How a column aligns its cells.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Align {
    Left,
    Right,
}

/// One column: its header and alignment.
#[derive(Debug, Clone, Copy)]
pub struct Column {
    pub header: &'static str,
    pub align: Align,
}

impl Column {
    pub const fn left(header: &'static str) -> Self {
        Self {
            header,
            align: Align::Left,
        }
    }

    pub const fn right(header: &'static str) -> Self {
        Self {
            header,
            align: Align::Right,
        }
    }
}

/// A table that pads every column to its widest cell, measured in characters.
///
/// Columns are separated by two spaces. Cells are never truncated. Lines carry no trailing
/// whitespace.
#[derive(Debug)]
pub struct Table {
    columns: Vec<Column>,
    rows: Vec<Vec<String>>,
}

impl Table {
    pub fn new(columns: Vec<Column>) -> Self {
        Self {
            columns,
            rows: Vec::new(),
        }
    }

    /// Adds a row. Missing cells are empty; cells beyond the column count are ignored.
    /// Control characters in cells are escaped.
    pub fn row(&mut self, cells: Vec<String>) {
        self.rows.push(
            cells
                .into_iter()
                .map(|cell| printable(&cell).into_owned())
                .collect(),
        );
    }

    /// Writes the header line and one line per row.
    ///
    /// # Errors
    ///
    /// Returns the error of the underlying writer.
    pub fn write(&self, w: &mut dyn Write) -> io::Result<()> {
        let widths = self.widths();
        let header: Vec<&str> = self.columns.iter().map(|column| column.header).collect();
        self.write_line(w, &widths, &header)?;
        for row in &self.rows {
            let cells: Vec<&str> = row.iter().map(String::as_str).collect();
            self.write_line(w, &widths, &cells)?;
        }
        Ok(())
    }

    fn widths(&self) -> Vec<usize> {
        self.columns
            .iter()
            .enumerate()
            .map(|(index, column)| {
                self.rows
                    .iter()
                    .filter_map(|row| row.get(index))
                    .map(|cell| width(cell))
                    .fold(width(column.header), usize::max)
            })
            .collect()
    }

    fn write_line(&self, w: &mut dyn Write, widths: &[usize], cells: &[&str]) -> io::Result<()> {
        let mut line = String::new();
        for (index, (column, column_width)) in self.columns.iter().zip(widths).enumerate() {
            let cell = cells.get(index).copied().unwrap_or_default();
            let padding = " ".repeat(column_width - width(cell));
            if index > 0 {
                line.push_str(COLUMN_GAP);
            }
            match column.align {
                Align::Left => {
                    line.push_str(cell);
                    line.push_str(&padding);
                }
                Align::Right => {
                    line.push_str(&padding);
                    line.push_str(cell);
                }
            }
        }
        writeln!(w, "{}", line.trim_end())
    }
}

fn width(cell: &str) -> usize {
    cell.chars().count()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn render(table: &Table) -> String {
        let mut buffer = Vec::new();
        table.write(&mut buffer).unwrap();
        String::from_utf8(buffer).unwrap()
    }

    #[test]
    fn pads_each_column_to_its_widest_cell_and_right_aligns_number_columns() {
        let mut table = Table::new(vec![Column::left("NAME"), Column::right("COUNT")]);
        table.row(vec!["pick".into(), "7".into()]);
        table.row(vec!["stack-blocks".into(), "1234567".into()]);

        assert_eq!(
            render(&table),
            "NAME            COUNT\n\
             pick                7\n\
             stack-blocks  1234567\n"
        );
    }

    #[test]
    fn pads_a_cell_with_non_ascii_characters_by_its_character_count() {
        let mut table = Table::new(vec![Column::left("TASK"), Column::right("N")]);
        table.row(vec!["Würfel greifen".into(), "1".into()]);
        table.row(vec!["Pick".into(), "2".into()]);

        assert_eq!(
            render(&table),
            "TASK            N\n\
             Würfel greifen  1\n\
             Pick            2\n"
        );
    }

    #[test]
    fn writes_no_trailing_whitespace_after_a_left_aligned_last_column() {
        let mut table = Table::new(vec![Column::left("ID"), Column::left("TASK")]);
        table.row(vec!["1".into(), "a".into()]);

        assert_eq!(render(&table), "ID  TASK\n1   a\n");
    }
}
