//! Where a branch is deployed, as GitLab and GitHub say.
//!
//! `glab` and `gh` are stand-ins that answer from files and write down what they were asked.
#![cfg(unix)]

mod common;

use ae_core::hosts::{HostKind, Hosts, Remote};
use common::*;
use std::path::{Path, PathBuf};

fn calls(dir: &Path) -> Vec<String> {
    std::fs::read_to_string(dir.join("calls.log")).unwrap_or_default().lines().map(str::to_string).collect()
}

fn hosts(sandbox: &Sandbox) -> (Hosts, PathBuf) {
    let dir = sandbox.0.join("tools");
    let glab = fake_tool(
        &dir,
        "glab",
        r#"here="$(dirname "$0")"
echo "[$GITLAB_HOST] $*" >> "$here/calls.log"
case "$2" in
  */environments\?*) cat "$here/environments.json" ;;
  */deployments\?*) cat "$here/deployments.json" ;;
  *) echo "unexpected endpoint: $2" >&2; exit 2 ;;
esac
"#,
    );
    let gh = fake_tool(
        &dir,
        "gh",
        r#"here="$(dirname "$0")"
echo "$*" >> "$here/calls.log"
case "$2" in
  */deployments\?ref=*) cat "$here/by-branch.json" ;;
  */deployments\?sha=*) cat "$here/by-commit.json" ;;
  */deployments/*/statuses*) id="${2#*/deployments/}"; cat "$here/status-${id%%/*}.json" ;;
  *) echo "unexpected endpoint: $2" >&2; exit 2 ;;
esac
"#,
    );
    let missing = PathBuf::from("/nonexistent/ae-test/docker");
    (Hosts::with(gh, glab, missing, "http://127.0.0.1:9/graphql".into()), dir)
}

#[test]
fn gitlab_is_asked_for_the_environments_named_after_the_branch_and_the_newest_deployments() {
    let sandbox = Sandbox::new("deploys-gitlab");
    let (hosts, dir) = hosts(&sandbox);
    let remote = Remote { kind: HostKind::GitLab, host: "gitlab.example.com".into(), slug: "group/sub/shop".into() };
    std::fs::write(
        dir.join("environments.json"),
        r#"[{"id":200,"name":"review/fix-login-limit","external_url":"https://42.preview.example.com","state":"available"}]"#,
    )
    .unwrap();
    std::fs::write(
        dir.join("deployments.json"),
        r#"[{"id":8,"ref":"refs/merge-requests/42/merge","sha":"cccccccccccc","status":"success","created_at":"2026-11-13T11:15:46Z",
             "environment":{"id":200,"name":"review/fix-login-limit","external_url":"https://42.preview.example.com"}}]"#,
    )
    .unwrap();

    let found = hosts.deploys(&remote, "fix/Login limit", Some("abc123"), Some(42)).unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(
        (found[0].name.as_str(), found[0].state, found[0].url.as_str(), found[0].commit.as_str()),
        ("review/fix-login-limit", "ready", "https://42.preview.example.com", "cccccccc")
    );
    // The branch is looked for as the pipeline spells it in names; the project stands in the address as one piece.
    assert_eq!(
        calls(&dir),
        [
            "[gitlab.example.com] api projects/group%2Fsub%2Fshop/environments?states=available&per_page=20&search=fix-login-limit",
            "[gitlab.example.com] api projects/group%2Fsub%2Fshop/deployments?order_by=created_at&sort=desc&per_page=100",
        ]
    );

    // A branch whose name is too short to look for is not looked for by name.
    std::fs::remove_file(dir.join("calls.log")).unwrap();
    hosts.deploys(&remote, "x", None, None).unwrap();
    assert_eq!(calls(&dir).len(), 1, "{:?}", calls(&dir));
    // What GitLab refuses is said, not taken for "nowhere".
    std::fs::remove_file(dir.join("deployments.json")).unwrap();
    assert!(hosts.deploys(&remote, "fix/login", None, None).is_err());
}

#[test]
fn github_is_asked_by_branch_and_by_commit_and_then_how_each_newest_one_stands() {
    let sandbox = Sandbox::new("deploys-github");
    let (hosts, dir) = hosts(&sandbox);
    let remote = Remote { kind: HostKind::GitHub, host: "github.com".into(), slug: "acme/shop".into() };
    std::fs::write(dir.join("by-branch.json"), "[]").unwrap();
    std::fs::write(
        dir.join("by-commit.json"),
        r#"[{"id":14,"environment":"Preview","sha":"bbbbbbbbbbbbbbbb","created_at":"2026-11-12T09:00:00Z"},
            {"id":13,"environment":"storybook","sha":"bbbbbbbbbbbbbbbb","created_at":"2026-11-12T08:00:00Z"},
            {"id":12,"environment":"Preview","sha":"bbbbbbbbbbbbbbbb","created_at":"2026-11-12T07:00:00Z"}]"#,
    )
    .unwrap();
    std::fs::write(
        dir.join("status-14.json"),
        r#"[{"state":"success","environment_url":"https://shop-git-fix.vercel.app"}]"#,
    )
    .unwrap();
    // Replaced by something newer: no place any more.
    std::fs::write(dir.join("status-13.json"), r#"[{"state":"inactive","environment_url":"https://old.example.com"}]"#)
        .unwrap();

    let commit = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    let found = hosts.deploys(&remote, "fix/login limit", Some(commit), None).unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(
        (found[0].name.as_str(), found[0].state, found[0].url.as_str()),
        ("Preview", "ready", "https://shop-git-fix.vercel.app")
    );
    assert_eq!(
        calls(&dir),
        [
            "api repos/acme/shop/deployments?ref=fix%2Flogin%20limit&per_page=20",
            &format!("api repos/acme/shop/deployments?sha={commit}&per_page=20"),
            "api repos/acme/shop/deployments/14/statuses?per_page=1",
            "api repos/acme/shop/deployments/13/statuses?per_page=1",
        ],
        "the older deployment of the same environment is not asked about"
    );

    // A commit that is no commit never reaches an address.
    std::fs::remove_file(dir.join("calls.log")).unwrap();
    hosts.deploys(&remote, "main", Some("abc; rm -rf"), None).unwrap();
    assert!(calls(&dir).iter().all(|call| !call.contains("sha=")), "{:?}", calls(&dir));
}
