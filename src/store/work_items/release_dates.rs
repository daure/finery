use chrono::NaiveDate;

use super::release::{ReleaseDateEstimate, ReleaseVersion};

/// Treat dated versions within one project as a sequential planning stream.
pub(crate) fn estimate_dates(versions: &mut [ReleaseVersion]) {
    let mut dated = versions
        .iter()
        .filter_map(|version| Some((version.end_date.or(version.start_date)?, version)))
        .collect::<Vec<_>>();
    dated.sort_by_key(|(date, _)| *date);
    let estimates = dated
        .iter()
        .enumerate()
        .filter_map(|(index, (anchor, version))| {
            let previous = index.checked_sub(1).and_then(|index| dated.get(index));
            let next = dated.get(index + 1);
            if previous.is_some_and(|(date, _)| date == anchor)
                || next.is_some_and(|(date, _)| date == anchor)
            {
                return None;
            }
            let (start, end, source, is_start) = match (version.start_date, version.end_date) {
                (None, Some(end)) => {
                    let (date, source) = previous?;
                    if index >= 2 && dated[index - 2].0 == *date {
                        return None;
                    }
                    (source.end_date?.succ_opt()?, end, *source, true)
                }
                (Some(start), None) => {
                    let (date, source) = next?;
                    if dated.get(index + 2).is_some_and(|(other, _)| other == date) {
                        return None;
                    }
                    (start, source.start_date?.pred_opt()?, *source, false)
                }
                _ => return None,
            };
            if start > end || overlaps_explicit_window(versions, &version.id, start, end) {
                return None;
            }
            Some((
                version.id.clone(),
                is_start,
                ReleaseDateEstimate {
                    date: if is_start { start } else { end },
                    source_version_id: source.id.clone(),
                },
            ))
        })
        .collect::<Vec<_>>();
    for version in versions {
        version.estimated_start = None;
        version.estimated_end = None;
        for (_, is_start, estimate) in estimates.iter().filter(|(id, _, _)| id == &version.id) {
            if *is_start {
                version.estimated_start = Some(estimate.clone());
            } else {
                version.estimated_end = Some(estimate.clone());
            }
        }
    }
}

fn overlaps_explicit_window(
    versions: &[ReleaseVersion],
    id: &str,
    start: NaiveDate,
    end: NaiveDate,
) -> bool {
    versions
        .iter()
        .filter(|version| version.id != id)
        .any(|version| {
            match (version.start_date, version.end_date) {
                (Some(other_start), Some(other_end)) => {
                    // Invalid windows cannot provide a trustworthy planning boundary.
                    other_start > other_end || (other_start <= end && start <= other_end)
                }
                _ => false,
            }
        })
}

#[cfg(test)]
#[path = "tests/release_dates.rs"]
mod tests;
