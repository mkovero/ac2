//! A plain-text page of sections, rows and aligned columns for a terminal: a heading per
//! section (with an optional summary beside it), rows whose cells line up in columns, and
//! continuation lines under the row's value column. A line that reports trouble starts
//! with `!` in column 0, every other line with two spaces, so a problem is found by its
//! first character. Lines wider than the page break at their ` · ` separators.

/// Spaces between columns.
const GAP: usize = 3;

/// One line of text, and whether it reports trouble.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Line {
    pub text: String,
    pub trouble: bool,
}

impl Line {
    pub fn plain(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            trouble: false,
        }
    }

    pub fn trouble(text: impl Into<String>, trouble: bool) -> Self {
        Self {
            text: text.into(),
            trouble,
        }
    }
}

/// A row: its label and value cells, aligned with the other rows of its section, and the
/// lines that continue it under its first value cell.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Row {
    pub cells: Vec<String>,
    pub trouble: bool,
    pub more: Vec<Line>,
}

impl Row {
    pub fn new<S: Into<String>>(cells: impl IntoIterator<Item = S>) -> Self {
        Self {
            cells: cells.into_iter().map(Into::into).collect(),
            trouble: false,
            more: Vec::new(),
        }
    }

    pub fn trouble(mut self, t: bool) -> Self {
        self.trouble = t;
        self
    }

    pub fn more(mut self, l: Line) -> Self {
        self.more.push(l);
        self
    }
}

/// A section: its heading (said as given) and summary, then its rows.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Section {
    pub heading: String,
    pub summary: Option<Line>,
    pub rows: Vec<Row>,
}

fn width(s: &str) -> usize {
    s.chars().count()
}

fn pad(s: &str, w: usize) -> String {
    let n = w.saturating_sub(width(s));
    format!("{s}{}", " ".repeat(n))
}

fn mark(trouble: bool) -> &'static str {
    if trouble { "! " } else { "  " }
}

/// `text` broken at its ` · ` separators into pieces, the first at most `first` columns
/// wide and the rest at most `rest`, as far as the pieces allow (a piece wider than its
/// line stays whole).
fn wrap(text: &str, first: usize, rest: usize) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut cur = String::new();
    for piece in text.split(" · ") {
        let room = if out.is_empty() { first } else { rest };
        if cur.is_empty() {
            cur = piece.to_owned();
        } else if width(&cur) + 3 + width(piece) <= room {
            cur.push_str(" · ");
            cur.push_str(piece);
        } else {
            out.push(std::mem::replace(&mut cur, piece.to_owned()));
        }
    }
    out.push(cur);
    out
}

/// The sections as text, a blank line between them, lines at most `page` columns wide
/// where their separators allow. A section with no summary and no rows is left out.
pub fn render(sections: &[Section], page: usize) -> String {
    let mut blocks = Vec::new();
    for s in sections {
        if s.summary.is_none() && s.rows.is_empty() {
            continue;
        }
        blocks.push(section(s, page));
    }
    blocks.join("\n\n")
}

fn section(s: &Section, page: usize) -> String {
    let ncols = s.rows.iter().map(|r| r.cells.len()).max().unwrap_or(0);
    // A column is as wide as its widest cell that another cell follows: the last cell of a
    // row is never padded, so a long one does not push the others apart.
    let mut widths = vec![0usize; ncols];
    for r in &s.rows {
        for (i, c) in r
            .cells
            .iter()
            .enumerate()
            .take(r.cells.len().saturating_sub(1))
        {
            widths[i] = widths[i].max(width(c));
        }
    }
    // The value column: after the labels, and after the heading where a summary sits.
    let labels = widths.first().map_or(0, |w| 2 + w + GAP);
    let value_col = labels.max(width(&s.heading) + GAP);
    if let Some(w) = widths.first_mut() {
        *w = value_col - 2 - GAP;
    }
    let mut out: Vec<String> = Vec::new();
    let cont = |out: &mut Vec<String>, l: &Line| {
        for piece in wrap(
            &l.text,
            page.saturating_sub(value_col),
            page.saturating_sub(value_col),
        ) {
            out.push(format!(
                "{}{}{piece}",
                mark(l.trouble),
                " ".repeat(value_col - 2)
            ));
        }
    };
    match &s.summary {
        Some(l) if !l.trouble => {
            let pieces = wrap(
                &l.text,
                page.saturating_sub(value_col),
                page.saturating_sub(value_col),
            );
            out.push(format!("{}{}", pad(&s.heading, value_col), pieces[0]));
            for p in &pieces[1..] {
                out.push(format!("{}{p}", " ".repeat(value_col)));
            }
        }
        Some(l) => {
            out.push(s.heading.clone());
            cont(&mut out, l);
        }
        None => out.push(s.heading.clone()),
    }
    for r in &s.rows {
        let mut line = String::from(mark(r.trouble));
        let last = r.cells.len().saturating_sub(1);
        for (i, c) in r.cells.iter().enumerate() {
            // A column empty in every row takes no room: it would only widen the gap.
            if i < last && (i == 0 || widths[i] > 0) {
                line.push_str(&pad(c, widths[i]));
                line.push_str(&" ".repeat(GAP));
            }
        }
        let start = width(&line);
        let tail = r.cells.get(last).map_or("", String::as_str);
        let pieces = wrap(
            tail,
            page.saturating_sub(start),
            page.saturating_sub(value_col),
        );
        out.push(format!("{line}{}", pieces[0]).trim_end().to_owned());
        for p in &pieces[1..] {
            out.push(format!(
                "{}{}{p}",
                mark(r.trouble),
                " ".repeat(value_col - 2)
            ));
        }
        for l in &r.more {
            cont(&mut out, l);
        }
    }
    out.join("\n")
}

#[cfg(test)]
#[path = "layout_tests.rs"]
mod tests;
