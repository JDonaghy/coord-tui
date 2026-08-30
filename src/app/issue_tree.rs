//! #12: the shared core of the Board / Pipeline / Kanban issue trees.
//!
//! Board and Pipeline each build a repo → milestone → issue → nested-epic-child
//! tree, and until this module each owned a private, drifting copy of the
//! rules for doing it:
//!
//! | rule | Pipeline copy | Board copy |
//! |---|---|---|
//! | milestone bucketing | `pipeline_milestones_for_issues` | `board_milestones_for_repo` |
//! | epic parent → child walk | `pipeline_globally_nested_children` | `board_globally_nested_children` + `board_epic_child_parents` |
//! | "already nested, skip the flat row" | `is_globally_nested` | `board_is_globally_nested` |
//!
//! The functions here are deliberately **rule-only, not policy**. Each panel
//! still decides *which rows it feeds in* — and those filters legitimately
//! differ (Pipeline honours `pipeline_dismissed`, Board does not; Kanban is
//! intentionally not search-filtered, see `board_epic_child_parents`). Making
//! the filters uniform would be a behaviour change, not a dedup. What is
//! shared is what happens to the rows once chosen: the bucketing order, the
//! `No milestone` fallback, and the parent→child walk.
//!
//! The already-shared seams this finishes the job on are
//! `epic_children_for_repo_issue` and `epic_expand_state_in` (`pipeline.rs`),
//! which #1270 factored out for exactly this reason.

use std::collections::{BTreeMap, HashMap, HashSet};

use super::types::EpicChild;

/// Stable id for the synthetic bucket holding issues with no milestone.
pub(crate) const NO_MILESTONE_KEY: &str = "no-milestone";
/// Display title for the synthetic bucket holding issues with no milestone.
pub(crate) const NO_MILESTONE_TITLE: &str = "No milestone";

/// Bucket `items` by milestone, returning `(key, display_title, items)` groups
/// in the panels' canonical order.
///
/// `milestone_of` yields each item's `(milestone_number, milestone_title)`, or
/// `None` for an unmilestoned issue.
///
/// Ordering is by `(milestone_number, milestone_title)`, so numbered
/// milestones come out in creation order and the `None` bucket — keyed
/// `(i64::MAX, "")` — always sorts last. That "no milestone sinks to the
/// bottom" rule is why the sort key is the raw number rather than the title,
/// and it is the same in both panels; it is now stated once.
///
/// Within a bucket, items keep the order they were fed in.
pub(crate) fn group_by_milestone<T>(
    items: impl IntoIterator<Item = T>,
    milestone_of: impl Fn(&T) -> Option<(i64, String)>,
) -> Vec<(String, String, Vec<T>)> {
    let mut buckets: BTreeMap<(i64, String), (String, String, Vec<T>)> = BTreeMap::new();

    for item in items {
        let (sort_key, key, title) = match milestone_of(&item) {
            Some((number, title)) => ((number, title.clone()), number.to_string(), title),
            None => (
                (i64::MAX, String::new()),
                NO_MILESTONE_KEY.to_string(),
                NO_MILESTONE_TITLE.to_string(),
            ),
        };
        buckets
            .entry(sort_key)
            .or_insert_with(|| (key, title, Vec::new()))
            .2
            .push(item);
    }

    buckets
        .into_values()
        .filter(|(_, _, items)| !items.is_empty())
        .collect()
}

/// Walk `rows` (already filtered by the caller) and map every child issue to
/// its parent epic's issue number, repo-scoped.
///
/// `rows` yields `(repo_name, issue_number)` for each row that is *allowed to
/// act as a parent* — the caller's filter set is applied before this point,
/// on purpose (see the module docs). `children_of` resolves an epic's child
/// list, i.e. `CoordApp::epic_children_for_repo_issue`.
///
/// A child listed under two visible epics resolves to the last one walked;
/// that matches the pre-#12 `HashMap::insert` behaviour of
/// `board_epic_child_parents`, and the data seam (`data.epic_children`, one
/// entry per tracking issue) does not produce it in practice.
pub(crate) fn epic_child_parents<'a, 'c>(
    rows: impl IntoIterator<Item = (&'a str, u64)>,
    children_of: impl Fn(&'a str, u64) -> Option<&'c [EpicChild]>,
) -> HashMap<(String, u64), u64> {
    let mut parents = HashMap::new();
    for (repo, number) in rows {
        let Some(children) = children_of(repo, number) else {
            continue;
        };
        for child in children {
            parents.insert((repo.to_string(), child.number), number);
        }
    }
    parents
}

/// The suppression set: every child issue nested under some visible epic.
///
/// A row in this set must not also be emitted as a flat top-level row — it
/// renders exactly once, nested under its parent. Same walk as
/// [`epic_child_parents`], keys only.
pub(crate) fn nested_children<'a, 'c>(
    rows: impl IntoIterator<Item = (&'a str, u64)>,
    children_of: impl Fn(&'a str, u64) -> Option<&'c [EpicChild]>,
) -> HashSet<(String, u64)> {
    epic_child_parents(rows, children_of).into_keys().collect()
}

/// `true` when `(repo, number)` is in a [`nested_children`] suppression set and
/// its flat top-level row should therefore be skipped.
pub(crate) fn is_nested(repo: &str, number: u64, nested: &HashSet<(String, u64)>) -> bool {
    nested.contains(&(repo.to_string(), number))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn child(number: u64) -> EpicChild {
        EpicChild {
            number,
            state: "open".to_string(),
        }
    }

    #[test]
    fn group_by_milestone_orders_by_number_and_sinks_the_none_bucket() {
        let items = vec![
            ("a", Some((7, "ms-7".to_string()))),
            ("b", None),
            ("c", Some((2, "ms-2".to_string()))),
            ("d", Some((7, "ms-7".to_string()))),
        ];
        let groups = group_by_milestone(items, |(_, m)| m.clone());

        let shape: Vec<(String, String, Vec<&str>)> = groups
            .into_iter()
            .map(|(k, t, items)| (k, t, items.into_iter().map(|(n, _)| n).collect()))
            .collect();

        assert_eq!(
            shape,
            vec![
                ("2".to_string(), "ms-2".to_string(), vec!["c"]),
                ("7".to_string(), "ms-7".to_string(), vec!["a", "d"]),
                (
                    NO_MILESTONE_KEY.to_string(),
                    NO_MILESTONE_TITLE.to_string(),
                    vec!["b"]
                ),
            ]
        );
    }

    #[test]
    fn group_by_milestone_on_no_items_is_empty() {
        let groups = group_by_milestone(Vec::<u64>::new(), |_| None);
        assert!(groups.is_empty());
    }

    #[test]
    fn epic_child_parents_maps_children_to_their_epic_repo_scoped() {
        let kids = vec![child(11), child(12)];
        let rows = vec![("repo-a", 1_u64), ("repo-b", 1_u64)];
        let parents = epic_child_parents(rows, |repo, number| {
            if repo == "repo-a" && number == 1 {
                Some(kids.as_slice())
            } else {
                None
            }
        });

        assert_eq!(parents.get(&("repo-a".to_string(), 11)), Some(&1));
        assert_eq!(parents.get(&("repo-a".to_string(), 12)), Some(&1));
        // Same issue number in a different repo is a different key.
        assert_eq!(parents.get(&("repo-b".to_string(), 11)), None);
        assert_eq!(parents.len(), 2);
    }

    #[test]
    fn nested_children_only_contains_children_of_supplied_rows() {
        let kids = vec![child(11)];
        // The caller filtered epic #1 out; its child must NOT be suppressed.
        let nested = nested_children(Vec::<(&str, u64)>::new(), |_, _| Some(kids.as_slice()));
        assert!(nested.is_empty());
        assert!(!is_nested("repo-a", 11, &nested));

        let nested = nested_children(vec![("repo-a", 1_u64)], |_, _| Some(kids.as_slice()));
        assert!(is_nested("repo-a", 11, &nested));
        assert!(!is_nested("repo-b", 11, &nested));
        assert!(!is_nested("repo-a", 1, &nested));
    }
}
