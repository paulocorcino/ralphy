//! Fetching an issue from the queue via the `gh` CLI. The queue *is* GitHub
//! issues, so this is a core (domain) concern — distinct from how any agent is
//! driven.

mod client;
mod comments;
mod issues;
mod labels;
mod references;
mod repo;

pub use comments::{
    comment_issue, edit_comment, find_marked_comment, issue_comments, list_comments_with_ids,
    parse_issue_comments, parse_rest_comments, upsert_marked_comment,
};
pub use issues::{
    add_label, build_queue, close_issue, create_issue, edit_issue_body, fetch_issue,
    issue_is_closed, issue_labels, list_open_issues, list_queue, parse_issue, queue_list_args,
    remove_label, resolve_login,
};
pub use labels::{
    apply_label_actions, format_label_plan, human_return_labels, list_repo_labels,
    parse_triage_mapping, plan_label_actions, ralphy_label_specs, resolve_human_return_labels,
    resolve_queue_labels, LabelAction, LabelSpec,
};
pub use references::fetch_reference;
pub use repo::{create_milestone, create_repo};
