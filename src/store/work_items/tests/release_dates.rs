use super::*;
use crate::store::work_items::release::parse_date;

fn version(id: &str, start: Option<&str>, end: Option<&str>) -> ReleaseVersion {
    ReleaseVersion {
        id: id.into(),
        name: id.into(),
        start_date: start.and_then(parse_date),
        end_date: end.and_then(parse_date),
        ..Default::default()
    }
}

#[test]
fn estimates_use_adjacent_explicit_boundaries_and_preserve_raw_dates() {
    let mut versions = vec![
        version("next", Some("2026-10-12"), Some("2026-10-23")),
        version("previous", None, Some("2026-09-11")),
        version("end-only", None, Some("2026-09-25")),
        version("start-only", Some("2026-09-28"), None),
        version("undated", None, None),
    ];
    let original = versions.clone();
    estimate_dates(&mut versions);
    for (raw, estimated) in original.iter().zip(&versions) {
        assert_eq!(raw.start_date, estimated.start_date);
        assert_eq!(raw.end_date, estimated.end_date);
    }
    assert_eq!(
        versions[2].estimated_start,
        Some(ReleaseDateEstimate {
            date: parse_date("2026-09-12").unwrap(),
            source_version_id: "previous".into(),
        })
    );
    assert_eq!(
        versions[3].estimated_end,
        Some(ReleaseDateEstimate {
            date: parse_date("2026-10-11").unwrap(),
            source_version_id: "next".into(),
        })
    );
    for index in [0, 1, 4] {
        assert!(versions[index].estimated_start.is_none());
        assert!(versions[index].estimated_end.is_none());
    }
    let estimated = versions.clone();
    estimate_dates(&mut versions);
    assert_eq!(estimated, versions);
    versions.reverse();
    estimate_dates(&mut versions);
    versions.reverse();
    assert_eq!(estimated, versions);
}

#[test]
fn ambiguous_boundaries_and_overlapping_windows_leave_dates_unknown() {
    for mut versions in [
        vec![
            version("previous", None, Some("2026-09-11")),
            version("same-deadline", None, Some("2026-09-11")),
            version("target", None, Some("2026-09-25")),
        ],
        vec![
            version("previous", None, Some("2026-09-11")),
            version("target", None, Some("2026-09-25")),
            version("same-deadline", None, Some("2026-09-25")),
        ],
        vec![
            version("target", Some("2026-09-01"), None),
            version("next", Some("2026-09-25"), None),
            version("same-start", Some("2026-09-25"), None),
        ],
        vec![
            version("previous", None, Some("2026-09-11")),
            version("target", None, Some("2026-09-25")),
            version("parallel", Some("2026-09-20"), Some("2026-10-01")),
        ],
        vec![
            version("parallel", Some("2026-09-01"), Some("2026-09-20")),
            version("target", Some("2026-09-15"), None),
            version("next", Some("2026-10-01"), Some("2026-10-14")),
        ],
        vec![
            version("invalid", Some("2026-10-01"), Some("2026-09-11")),
            version("target", None, Some("2026-09-25")),
        ],
    ] {
        estimate_dates(&mut versions);
        let target = versions
            .iter()
            .find(|version| version.id == "target")
            .unwrap();
        assert!(target.estimated_start.is_none(), "{versions:?}");
        assert!(target.estimated_end.is_none(), "{versions:?}");
    }
}

#[test]
fn unknown_neighbor_boundaries_do_not_chain_estimates_or_guess_durations() {
    let mut versions = vec![
        version("start-only", Some("2026-09-01"), None),
        version("end-only", None, Some("2026-09-25")),
        version("undated", None, None),
    ];
    estimate_dates(&mut versions);
    for version in versions {
        assert!(version.estimated_start.is_none());
        assert!(version.estimated_end.is_none());
    }
}
