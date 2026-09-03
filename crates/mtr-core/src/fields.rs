//! `data_fields[]` (ui/mtr.c:69-87) and the `-o` rules (ui/mtr.c:335-348, 755-767) —
//! mtr 0.96, commit 7b01773. GPL-2.0-only.

use crate::hop::Hop;

/// The printf format of a field, as report.c/json/csv interpret it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldFormat {
    /// `" %6.2f%%"` — value is loss percent × 1000.
    Percent,
    /// `" %5.1f"` — microseconds shown as milliseconds.
    Ms5,
    /// `" %4.1f"` — microseconds shown as milliseconds.
    Ms4,
    /// `" %4d"`
    Int4,
    /// `" %5d"`
    Int5,
    /// `" "` — the spacer field.
    Space,
}

impl FieldFormat {
    /// `strchr(format, 'f') != NULL`: the value is divided by 1000.0 before printing.
    pub fn is_float(self) -> bool {
        matches!(
            self,
            FieldFormat::Percent | FieldFormat::Ms5 | FieldFormat::Ms4
        )
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Field {
    pub key: char,
    pub descr: &'static str,
    pub title: &'static str,
    pub format: FieldFormat,
    pub length: usize,
    pub value: fn(&Hop) -> i32,
}

pub const FIELDS: [Field; 15] = [
    Field {
        key: ' ',
        descr: "<sp>: Space between fields",
        title: " ",
        format: FieldFormat::Space,
        length: 1,
        value: Hop::dropped,
    },
    Field {
        key: 'L',
        descr: "L: Loss Ratio",
        title: "Loss%",
        format: FieldFormat::Percent,
        length: 8,
        value: Hop::loss,
    },
    Field {
        key: 'D',
        descr: "D: Dropped Packets",
        title: "Drop",
        format: FieldFormat::Int4,
        length: 5,
        value: Hop::dropped,
    },
    Field {
        key: 'R',
        descr: "R: Received Packets",
        title: "Rcv",
        format: FieldFormat::Int5,
        length: 6,
        value: Hop::received,
    },
    Field {
        key: 'S',
        descr: "S: Sent Packets",
        title: "Snt",
        format: FieldFormat::Int5,
        length: 6,
        value: Hop::transmitted,
    },
    Field {
        key: 'N',
        descr: "N: Newest RTT(ms)",
        title: "Last",
        format: FieldFormat::Ms5,
        length: 6,
        value: Hop::last,
    },
    Field {
        key: 'B',
        descr: "B: Min/Best RTT(ms)",
        title: "Best",
        format: FieldFormat::Ms5,
        length: 6,
        value: Hop::best,
    },
    Field {
        key: 'A',
        descr: "A: Average RTT(ms)",
        title: "Avg",
        format: FieldFormat::Ms5,
        length: 6,
        value: Hop::avg,
    },
    Field {
        key: 'W',
        descr: "W: Max/Worst RTT(ms)",
        title: "Wrst",
        format: FieldFormat::Ms5,
        length: 6,
        value: Hop::worst,
    },
    Field {
        key: 'V',
        descr: "V: Standard Deviation",
        title: "StDev",
        format: FieldFormat::Ms5,
        length: 6,
        value: Hop::stdev,
    },
    Field {
        key: 'G',
        descr: "G: Geometric Mean",
        title: "Gmean",
        format: FieldFormat::Ms5,
        length: 6,
        value: Hop::gmean,
    },
    Field {
        key: 'J',
        descr: "J: Current Jitter",
        title: "Jttr",
        format: FieldFormat::Ms4,
        length: 5,
        value: Hop::jitter,
    },
    Field {
        key: 'M',
        descr: "M: Jitter Mean/Avg.",
        title: "Javg",
        format: FieldFormat::Ms4,
        length: 5,
        value: Hop::javg,
    },
    Field {
        key: 'X',
        descr: "X: Worst Jitter",
        title: "Jmax",
        format: FieldFormat::Ms4,
        length: 5,
        value: Hop::jworst,
    },
    Field {
        key: 'I',
        descr: "I: Interarrival Jitter",
        title: "Jint",
        format: FieldFormat::Ms4,
        length: 5,
        value: Hop::jinta,
    },
];

/// Characters `-o` accepts (`available_options`): every key plus a no-op `_`.
pub const AVAILABLE_OPTIONS: &str = " LDRSNBAWVGJMXI_";
/// `MAXFLD`: maximum length of the `-o` string.
pub const MAX_FIELDS: usize = 20;

pub fn field_by_key(key: char) -> Option<&'static Field> {
    FIELDS.iter().find(|f| f.key == key)
}

/// Resolve an active-fields string; unknown characters (`_`) are skipped like `fld_index[c] < 0`.
pub fn active_fields(spec: &str) -> Vec<&'static Field> {
    spec.chars().filter_map(field_by_key).collect()
}

/// `-o` validation (ui/mtr.c:755-767), with C's messages.
pub fn validate_fields(spec: &str) -> Result<(), String> {
    if spec.chars().count() > MAX_FIELDS {
        return Err(format!("Too many fields: {spec}"));
    }
    if let Some(c) = spec.chars().find(|c| !AVAILABLE_OPTIONS.contains(*c)) {
        return Err(format!("Unknown field identifier: {c}"));
    }
    Ok(())
}

/// Render a value with the field's `data_fields[].format` (float formats get `value / 1000.0`).
pub fn format_value(f: &Field, v: i32) -> String {
    match f.format {
        FieldFormat::Space => " ".to_string(),
        FieldFormat::Percent => format!(" {:6.2}%", f64::from(v) / 1000.0),
        FieldFormat::Ms5 => format!(" {:5.1}", f64::from(v) / 1000.0),
        FieldFormat::Ms4 => format!(" {:4.1}", f64::from(v) / 1000.0),
        FieldFormat::Int4 => format!(" {v:4}"),
        FieldFormat::Int5 => format!(" {v:5}"),
    }
}

/// Title right-aligned in the field width (`"%{length}s"`, report.c:214).
pub fn format_title(f: &Field) -> String {
    format!("{:>width$}", f.title, width = f.length)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_matches_ui_mtr_c() {
        let keys: String = FIELDS.iter().map(|f| f.key).collect();
        assert_eq!(keys, " LDRSNBAWVGJMXI");
        let titles: Vec<&str> = FIELDS.iter().map(|f| f.title).collect();
        assert_eq!(
            titles,
            [
                " ", "Loss%", "Drop", "Rcv", "Snt", "Last", "Best", "Avg", "Wrst", "StDev",
                "Gmean", "Jttr", "Javg", "Jmax", "Jint"
            ]
        );
        let lengths: Vec<usize> = FIELDS.iter().map(|f| f.length).collect();
        assert_eq!(lengths, [1, 8, 5, 6, 6, 6, 6, 6, 6, 6, 6, 5, 5, 5, 5]);
    }

    #[test]
    fn default_active_fields_and_skipping_of_underscore() {
        let keys: String = active_fields("LS NABWV").iter().map(|f| f.key).collect();
        assert_eq!(keys, "LS NABWV");
        assert_eq!(active_fields("L_S").len(), 2);
    }

    #[test]
    fn validation_matches_c_messages() {
        assert_eq!(validate_fields("LS NABWV_"), Ok(()));
        assert_eq!(
            validate_fields("LQ"),
            Err("Unknown field identifier: Q".to_string())
        );
        let long = "L".repeat(21);
        assert_eq!(
            validate_fields(&long),
            Err(format!("Too many fields: {long}"))
        );
        assert_eq!(validate_fields(&"L".repeat(20)), Ok(()));
    }

    #[test]
    fn formats_values_like_data_fields_format_strings() {
        let f = |k| field_by_key(k).unwrap();
        assert_eq!(format_value(f('L'), 11111), "  11.11%");
        assert_eq!(format_value(f('L'), 100000), " 100.00%");
        assert_eq!(format_value(f('S'), 2), "     2");
        assert_eq!(format_value(f('D'), 7), "    7");
        assert_eq!(format_value(f('N'), 500), "   0.5");
        assert_eq!(format_value(f('N'), 123456), " 123.5");
        assert_eq!(format_value(f('J'), 2500), "  2.5");
        assert_eq!(format_value(f(' '), 0), " ");
        assert!(f('L').format.is_float() && f('N').format.is_float() && f('J').format.is_float());
        assert!(!f('S').format.is_float() && !f(' ').format.is_float());
    }

    #[test]
    fn titles_are_right_aligned_in_the_field_width() {
        assert_eq!(format_title(field_by_key('L').unwrap()), "   Loss%");
        assert_eq!(format_title(field_by_key('V').unwrap()), " StDev");
        assert_eq!(format_title(field_by_key(' ').unwrap()), " ");
    }
}
