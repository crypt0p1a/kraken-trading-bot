//! The P&L of the sells, as flags above the curve of the portfolio

use crate::format;
use crate::theme;
use ratatui::text::{Line, Span};

/// What a sell says above the curve
#[derive(Debug, PartialEq)]
pub(super) struct Flag {
    column: usize,
    text: String,
    pnl: f64,
}

/// The flags at their columns
pub(super) fn flags_line(flags: &[Flag]) -> Line<'static> {
    let mut spans = Vec::new();
    let mut end = 0;
    for flag in flags {
        spans.push(Span::raw(" ".repeat(flag.column.saturating_sub(end))));
        spans.push(Span::styled(
            flag.text.clone(),
            theme::bold(theme::trend(flag.pnl)),
        ));
        end = flag.column + flag.text.chars().count();
    }
    Line::from(spans)
}

/// A flag per sell (time, P&L), the oldest first, at its column in the width of the chart. Sells
/// too close for their texts to fit share a flag, "3× +1.02", which never covers the one before
pub(super) fn sell_flags(sells: &[(f64, f64)], since: f64, now: f64, columns: usize) -> Vec<Flag> {
    /// Sells sharing a flag, and the column of the first one
    struct Group {
        wanted: usize,
        count: usize,
        pnl: f64,
    }
    impl Group {
        fn text(&self) -> String {
            if self.count > 1 {
                format!("{}× {}", self.count, format::signed(self.pnl))
            } else {
                format::signed(self.pnl)
            }
        }
        fn column(&self, columns: usize) -> usize {
            self.wanted
                .min(columns.saturating_sub(self.text().chars().count()))
        }
    }

    let span = (now - since).max(1.0);
    let mut groups: Vec<Group> = Vec::new();
    for (at, pnl) in sells {
        groups.push(Group {
            wanted: ((at - since) / span * columns.saturating_sub(1) as f64) as usize,
            count: 1,
            pnl: *pnl,
        });
        // a flag which doesn't fit after the one before joins it, which may then not fit either
        while let [.., before, last] = groups.as_slice() {
            if last.column(columns) > before.column(columns) + before.text().chars().count() {
                break;
            }
            let last = groups.pop().unwrap();
            let before = groups.last_mut().unwrap();
            before.count += last.count;
            before.pnl += last.pnl;
        }
    }
    groups
        .iter()
        .map(|it| Flag {
            column: it.column(columns),
            text: it.text(),
            pnl: it.pnl,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn puts_each_sell_at_its_time_and_merges_the_close_ones() {
        let flags = sell_flags(&[(0.0, 0.36), (50.0, 0.5), (51.0, 0.25)], 0.0, 100.0, 100);
        assert_eq!(
            flags,
            vec![
                Flag {
                    column: 0,
                    text: "+0.36".into(),
                    pnl: 0.36
                },
                Flag {
                    column: 49,
                    text: "2× +0.75".into(),
                    pnl: 0.75
                },
            ]
        );
    }

    #[test]
    fn a_flag_growing_at_the_edge_never_covers_the_one_before() {
        // 3 sells at the very end of 30 columns: their flag moves left, onto the one before
        let flags = sell_flags(
            &[(70.0, 0.38), (98.0, 1.0), (99.0, 1.0), (100.0, 2.4)],
            0.0,
            100.0,
            30,
        );
        for pair in flags.windows(2) {
            assert!(
                pair[0].column + pair[0].text.chars().count() < pair[1].column,
                "{flags:?}"
            );
        }
        let last = flags.last().unwrap();
        assert!(last.column + last.text.chars().count() <= 30, "{flags:?}");
        let total: f64 = flags.iter().map(|it| it.pnl).sum();
        assert!((total - 4.78).abs() < 1e-9, "{flags:?}");
    }

    #[test]
    fn writes_the_flags_at_their_columns() {
        let line = flags_line(&[
            Flag {
                column: 2,
                text: "+0.36".into(),
                pnl: 0.36,
            },
            Flag {
                column: 10,
                text: "-1.00".into(),
                pnl: -1.0,
            },
        ]);
        assert_eq!(line.to_string(), "  +0.36   -1.00");
    }

    #[test]
    fn keeps_a_flag_inside_the_chart() {
        let flags = sell_flags(&[(100.0, 0.36)], 0.0, 100.0, 40);
        assert_eq!(flags[0].column, 35);
    }
}
