//! Launcher-Update: Inhalt der `update.json` und Versionsvergleich.

use std::cmp::Ordering;

use serde::{Deserialize, Serialize};

/// Inhalt der `update.json` neben dem Installer.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Feed {
    pub version: String,
    #[serde(default)]
    pub notes: String,
    #[serde(default)]
    pub pub_date: String,
    pub url: String,
    #[serde(default)]
    pub sha256: String,
    #[serde(default)]
    pub size: Option<u64>,
}

/// `0.10.0` ist neuer als `0.9.0`, und `0.3.0` neuer als `0.3.0-beta.1`.
pub fn compare_versions(a: &str, b: &str) -> Ordering {
    fn split(v: &str) -> (Vec<u64>, &str) {
        let core = v.trim().trim_start_matches(['v', 'V']);
        let (core, pre) = core.split_once('-').unwrap_or((core, ""));
        let nums = core
            .split('.')
            .map(|p| {
                p.chars()
                    .take_while(char::is_ascii_digit)
                    .collect::<String>()
                    .parse()
                    .unwrap_or(0)
            })
            .collect();
        (nums, pre)
    }
    let ((an, ap), (bn, bp)) = (split(a), split(b));
    for i in 0..an.len().max(bn.len()) {
        let ord = an.get(i).unwrap_or(&0).cmp(bn.get(i).unwrap_or(&0));
        if ord != Ordering::Equal {
            return ord;
        }
    }
    // Vorabversionen sind älter als die fertige Version gleicher Nummer.
    match (ap.is_empty(), bp.is_empty()) {
        (true, true) => Ordering::Equal,
        (true, false) => Ordering::Greater,
        (false, true) => Ordering::Less,
        (false, false) => ap.cmp(bp),
    }
}
