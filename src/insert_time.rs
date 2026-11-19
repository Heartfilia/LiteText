//! "Insert current time" submenu shared by every body context menu.
//!
//! The template text doubles as the menu label, so the submenu documents
//! itself: picking `YYYY-mm-dd HH:MM:SS` inserts the local time in exactly
//! that shape. `mm` is the month and `MM` the minute, matching the labels.
//! Besides the literal patterns there are two computed stamps: ISO 8601 with
//! the local offset and the Unix epoch in whole seconds.

use chrono::{Datelike, Timelike};
use eframe::egui;

/// The offered templates. The pattern is also the label expanded by
/// [`expand`], so both stay in sync by construction.
#[derive(Clone, Copy)]
pub(crate) enum Template {
    /// A literal pattern whose tokens are replaced; the pattern is the label.
    Pattern(&'static str),
    /// ISO 8601 / RFC 3339 with the local offset, second precision.
    Iso,
    /// Seconds since the Unix epoch.
    Unix,
}

pub(crate) const TEMPLATES: &[Template] = &[
    Template::Pattern("YYYY-mm-dd"),
    Template::Pattern("YYYY/mm/dd"),
    Template::Pattern("YYYY.mm.dd"),
    Template::Pattern("YYYY-mm-dd HH:MM:SS"),
    Template::Pattern("YYYY/mm/dd HH:MM:SS"),
    Template::Pattern("YYYY.mm.dd HH:MM:SS"),
    Template::Iso,
    Template::Unix,
];

/// Local calendar time, split into the fields the templates need.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Parts {
    pub year: i32,
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
    pub second: u32,
}

pub(crate) fn now() -> Parts {
    let now = chrono::Local::now();
    Parts {
        year: now.year(),
        month: now.month(),
        day: now.day(),
        hour: now.hour(),
        minute: now.minute(),
        second: now.second(),
    }
}

/// Expand one pattern with the given local time.
pub(crate) fn expand(template: &str, parts: &Parts) -> String {
    // Tokens are case-distinct: `mm`/`MM` never collide, and every field is
    // exactly two (or four) characters wide, so one pass of `replace` is
    // deterministic.
    template
        .replace("YYYY", &format!("{:04}", parts.year))
        .replace("mm", &format!("{:02}", parts.month))
        .replace("dd", &format!("{:02}", parts.day))
        .replace("HH", &format!("{:02}", parts.hour))
        .replace("MM", &format!("{:02}", parts.minute))
        .replace("SS", &format!("{:02}", parts.second))
}

impl Template {
    fn label(self, unix_label: &str) -> String {
        match self {
            Template::Pattern(pattern) => pattern.to_owned(),
            Template::Iso => "ISO 8601".to_owned(),
            Template::Unix => unix_label.to_owned(),
        }
    }

    /// The stamp for this template at the current local time.
    fn stamp(self) -> String {
        match self {
            Template::Pattern(pattern) => expand(pattern, &now()),
            Template::Iso => chrono::Local::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            Template::Unix => chrono::Local::now().timestamp().to_string(),
        }
    }
}

/// Add the submenu to an open context menu. Returns the formatted stamp of
/// the template the user picked, computed at click time.
pub(crate) fn submenu(menu: &mut egui::Ui, label: &str, unix_label: &str) -> Option<String> {
    let header = menu.menu_button(egui::RichText::new(label).size(13.0), |menu| {
        crate::editor::line_menu_style(menu, "YYYY-mm-dd HH:MM:SS", unix_label);
        let mut picked = None;
        for template in TEMPLATES {
            let label = template.label(unix_label);
            let button = crate::editor::line_menu_button(menu, &label);
            #[cfg(test)]
            menu.ctx().data_mut(|data| {
                data.insert_temp(
                    egui::Id::new(("insert_time_template", label.clone())),
                    button.rect,
                )
            });
            if button.clicked() {
                picked = Some(template.stamp());
                menu.close_menu();
            }
        }
        picked
    });
    #[cfg(test)]
    menu.ctx().data_mut(|data| {
        data.insert_temp(egui::Id::new("insert_time_header"), header.response.rect)
    });
    header.inner.flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn patterns_expand_with_fixed_width_fields() {
        let parts = Parts {
            year: 2026,
            month: 11,
            day: 9,
            hour: 8,
            minute: 5,
            second: 3,
        };
        assert_eq!(expand("YYYY-mm-dd", &parts), "2026-11-09");
        assert_eq!(expand("YYYY/mm/dd", &parts), "2026/11/09");
        assert_eq!(expand("YYYY.mm.dd", &parts), "2026.11.09");
        assert_eq!(expand("YYYY-mm-dd HH:MM:SS", &parts), "2026-11-09 08:05:03");
        assert_eq!(expand("YYYY/mm/dd HH:MM:SS", &parts), "2026/11/09 08:05:03");
        assert_eq!(expand("YYYY.mm.dd HH:MM:SS", &parts), "2026.11.09 08:05:03");
        let single = Parts {
            year: 7,
            month: 1,
            day: 2,
            hour: 3,
            minute: 4,
            second: 5,
        };
        assert_eq!(
            expand("YYYY-mm-dd HH:MM:SS", &single),
            "0007-01-02 03:04:05",
            "fields must be zero-padded"
        );
    }

    #[test]
    fn iso_stamps_use_the_local_offset_at_second_precision() {
        let stamp = Template::Iso.stamp();
        let shape: String = stamp
            .chars()
            .map(|c| if c.is_ascii_digit() { '0' } else { c })
            .collect();
        // `to_rfc3339_opts(.., use_z = true)` prints `Z` in UTC, an explicit
        // `+hh:mm`/`-hh:mm` offset anywhere else.
        if stamp.ends_with('Z') {
            assert_eq!(shape, "0000-00-00T00:00:00Z");
        } else {
            assert_eq!(shape, "0000-00-00T00:00:00+00:00");
        }
    }

    #[test]
    fn unix_stamps_are_whole_seconds() {
        let stamp = Template::Unix.stamp();
        let expected = chrono::Local::now().timestamp().to_string();
        assert_eq!(stamp, expected);
        assert!(stamp.chars().all(|c| c.is_ascii_digit()));
        assert_eq!(stamp.len(), 10, "seconds since the epoch are 10 digits until 2286");
    }

    #[test]
    fn the_local_now_expands_into_the_pattern_shape() {
        for template in TEMPLATES {
            let pattern = match template {
                Template::Pattern(pattern) => pattern,
                // Computed stamps have their own shape tests.
                Template::Iso | Template::Unix => continue,
            };
            let stamp = expand(pattern, &now());
            let shape: String = stamp
                .chars()
                .map(|c| if c.is_ascii_digit() { '0' } else { c })
                .collect();
            let expected: String = pattern
                .chars()
                .map(|c| if c.is_ascii_alphabetic() { '0' } else { c })
                .collect();
            assert_eq!(shape, expected, "{pattern} -> {stamp}");
            let digits: String = stamp.chars().filter(|c| c.is_ascii_digit()).collect();
            assert_eq!(digits.len(), if pattern.contains(' ') { 14 } else { 8 });
        }
    }
}
