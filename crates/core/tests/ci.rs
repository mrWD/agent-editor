//! The pipeline of a branch, read and steered through stand-ins for `glab` and `gh`.
#![cfg(unix)]

mod common;

use ae_core::hosts::{HostKind, Hosts, Remote};
use common::{Sandbox, fake_tool};
use std::path::{Path, PathBuf};

fn calls(dir: &Path) -> Vec<String> {
    std::fs::read_to_string(dir.join("calls.log")).unwrap_or_default().lines().map(str::to_string).collect()
}

/// A `glab` that answers GitLab's API from files beside it and writes down what it was asked, with the server.
fn fake_glab(dir: &Path) -> PathBuf {
    fake_tool(
        dir,
        "glab",
        r#"here="$(dirname "$0")"
echo "[$GITLAB_HOST] $*" >> "$here/calls.log"
[ "$1" = "api" ] || { echo "unexpected call: $*" >&2; exit 2; }
shift
[ "$1" = "--method" ] && { echo '{}'; exit 0; }
case "$1" in
  */merge_requests/*/pipelines*) cat "$here/request-pipelines.json" ;;
  */pipelines\?ref=*) cat "$here/branch-pipelines.json" ;;
  */pipelines/*/jobs*page=2) cat "$here/jobs-2.json" 2>/dev/null || echo "[]" ;;
  */pipelines/*/jobs*) cat "$here/jobs.json" ;;
  */pipelines/*) cat "$here/pipeline.json" ;;
  *) echo "unexpected endpoint: $1" >&2; exit 2 ;;
esac
"#,
    )
}

fn fake_gh(dir: &Path) -> PathBuf {
    fake_tool(
        dir,
        "gh",
        r#"here="$(dirname "$0")"
echo "$*" >> "$here/calls.log"
case "$1 $2" in
  "run list") cat "$here/runs.json" ;;
  "run view") cat "$here/jobs-$3.json" ;;
  "run rerun"|"run cancel") exit 0 ;;
  *) echo "unexpected call: $*" >&2; exit 2 ;;
esac
"#,
    )
}

fn hosts(sandbox: &Sandbox) -> (Hosts, PathBuf) {
    let dir = sandbox.0.join("tools");
    let missing = PathBuf::from("/nonexistent/ae-test/docker");
    (Hosts::with(fake_gh(&dir), fake_glab(&dir), missing, "http://127.0.0.1:9/graphql".into()), dir)
}

#[test]
fn a_gitlab_pipeline_is_read_from_the_request_or_the_branch_and_steered_job_by_job() {
    let sandbox = Sandbox::new("ci-gitlab");
    let (hosts, dir) = hosts(&sandbox);
    let remote = Remote { kind: HostKind::GitLab, host: "gitlab.example.com".into(), slug: "group/sub/shop".into() };
    let branch = "fix/login limit";
    std::fs::write(dir.join("request-pipelines.json"), r#"[{"id":991,"status":"failed"}]"#).unwrap();
    std::fs::write(dir.join("branch-pipelines.json"), r#"[{"id":700,"status":"success"}]"#).unwrap();
    std::fs::write(
        dir.join("pipeline.json"),
        r#"{"id":991,"status":"failed","sha":"0123456789ab","web_url":"https://gitlab.example.com/p/991"}"#,
    )
    .unwrap();
    std::fs::write(
        dir.join("jobs.json"),
        r#"[{"id":5003,"name":"deploy","stage":"deploy","status":"manual","web_url":"u3","allow_failure":false},
            {"id":5002,"name":"unit","stage":"test","status":"failed","web_url":"u2","allow_failure":false}]"#,
    )
    .unwrap();

    // With a merge request, its own pipeline is the one: a project may run pipelines for requests only.
    let read = hosts.pipeline(&remote, branch, Some(42)).unwrap().expect("there is a pipeline");
    assert_eq!((read.state, read.runs[0].id.as_str(), read.runs[0].jobs.len()), ("failed", "991", 2));
    let asked = calls(&dir);
    assert_eq!(
        asked,
        [
            "[gitlab.example.com] api projects/group%2Fsub%2Fshop/merge_requests/42/pipelines?per_page=1",
            "[gitlab.example.com] api projects/group%2Fsub%2Fshop/pipelines/991",
            "[gitlab.example.com] api projects/group%2Fsub%2Fshop/pipelines/991/jobs?per_page=100&page=1",
        ]
    );

    // A pipeline of more than a hundred jobs is read to its end: the oldest jobs are its first stages.
    let page = |from: u32, count: u32| {
        let jobs: Vec<String> = (0..count)
            .map(|n| {
                format!(
                    r#"{{"id":{},"name":"job {}","stage":"test","status":"success","web_url":"u"}}"#,
                    from - n,
                    from - n
                )
            })
            .collect();
        format!("[{}]", jobs.join(","))
    };
    std::fs::write(dir.join("jobs.json"), page(6000, 100)).unwrap();
    std::fs::write(
        dir.join("jobs-2.json"),
        r#"[{"id":5001,"name":"build","stage":"build","status":"failed","web_url":"u1"}]"#,
    )
    .unwrap();
    let long = hosts.pipeline(&remote, branch, Some(42)).unwrap().unwrap();
    assert_eq!(long.runs[0].jobs.len(), 101);
    assert_eq!(
        (long.runs[0].jobs[0].name.as_str(), long.runs[0].jobs[0].state),
        ("build", "failed"),
        "the first stage is there"
    );
    assert!(calls(&dir).last().unwrap().ends_with("jobs?per_page=100&page=2"));
    std::fs::write(
        dir.join("jobs.json"),
        r#"[{"id":5003,"name":"deploy","stage":"deploy","status":"manual","web_url":"u3","allow_failure":false},
            {"id":5002,"name":"unit","stage":"test","status":"failed","web_url":"u2","allow_failure":false}]"#,
    )
    .unwrap();
    std::fs::remove_file(dir.join("jobs-2.json")).unwrap();

    // Without one, or when the request has none yet, the branch's newest pipeline is read.
    std::fs::remove_file(dir.join("calls.log")).unwrap();
    hosts.pipeline(&remote, branch, None).unwrap().expect("there is a pipeline");
    assert_eq!(
        calls(&dir)[0],
        "[gitlab.example.com] api projects/group%2Fsub%2Fshop/pipelines?ref=fix%2Flogin%20limit&per_page=1"
    );
    std::fs::write(dir.join("request-pipelines.json"), "[]").unwrap();
    std::fs::remove_file(dir.join("calls.log")).unwrap();
    hosts.pipeline(&remote, branch, Some(42)).unwrap().expect("the branch has one");
    assert!(calls(&dir)[1].contains("pipelines?ref="), "{:?}", calls(&dir));
    // A branch nothing ever ran for has no pipeline, which is not an error.
    std::fs::write(dir.join("branch-pipelines.json"), "[]").unwrap();
    assert_eq!(hosts.pipeline(&remote, branch, None).unwrap(), None);

    // Each thing a person does is one request to GitLab, with nothing in it but numbers of GitLab's own.
    std::fs::remove_file(dir.join("calls.log")).unwrap();
    for (what, id) in [
        ("retryJob", "5002"),
        ("playJob", "5003"),
        ("cancelJob", "5002"),
        ("retryFailed", "991"),
        ("cancelRun", "991"),
        ("runNew", ""),
    ] {
        hosts.pipeline_act(&remote, branch, what, id).unwrap_or_else(|error| panic!("{what}: {error:#}"));
    }
    let project = "[gitlab.example.com] api --method POST projects/group%2Fsub%2Fshop";
    assert_eq!(
        calls(&dir),
        [
            format!("{project}/jobs/5002/retry"),
            format!("{project}/jobs/5003/play"),
            format!("{project}/jobs/5002/cancel"),
            format!("{project}/pipelines/991/retry"),
            format!("{project}/pipelines/991/cancel"),
            format!("{project}/pipeline?ref=fix%2Flogin%20limit"),
        ]
    );
    // What is not a number never reaches GitLab.
    assert!(hosts.pipeline_act(&remote, branch, "retryJob", "5002/../../x").is_err());
    assert!(hosts.pipeline_act(&remote, branch, "format the disk", "1").is_err());
    assert_eq!(calls(&dir).len(), 6);
}

#[test]
fn githubs_runs_for_the_newest_commit_are_read_with_their_jobs_and_run_again() {
    let sandbox = Sandbox::new("ci-github");
    let (hosts, dir) = hosts(&sandbox);
    let remote = Remote { kind: HostKind::GitHub, host: "github.com".into(), slug: "mrx/shop".into() };
    std::fs::write(
        dir.join("runs.json"),
        r#"[{"databaseId":30,"status":"completed","conclusion":"failure","workflowName":"CI","url":"r30","headSha":"aaaa1111bbbb"},
            {"databaseId":29,"status":"in_progress","conclusion":"","workflowName":"Docs","url":"r29","headSha":"aaaa1111bbbb"},
            {"databaseId":20,"status":"completed","conclusion":"success","workflowName":"CI","url":"r20","headSha":"0000old"}]"#,
    )
    .unwrap();
    std::fs::write(
        dir.join("jobs-30.json"),
        r#"{"jobs":[{"databaseId":77,"name":"test","status":"completed","conclusion":"failure","url":"j77"}]}"#,
    )
    .unwrap();
    std::fs::write(dir.join("jobs-29.json"), r#"{"jobs":[]}"#).unwrap();

    let read = hosts.pipeline(&remote, "task/limit", None).unwrap().expect("there are runs");
    assert_eq!((read.state, read.commit.as_str(), read.runs.len()), ("failed", "aaaa1111", 2));
    assert_eq!((read.runs[0].name.as_str(), read.runs[0].jobs[0].id.as_str()), ("CI", "77"));
    assert_eq!(read.runs[1].state, "running");
    let asked = calls(&dir);
    assert!(asked[0].starts_with("run list --branch task/limit -R mrx/shop --limit 20 --json "), "{}", asked[0]);
    assert_eq!(asked[1..], ["run view 30 -R mrx/shop --json jobs", "run view 29 -R mrx/shop --json jobs"]);

    std::fs::remove_file(dir.join("calls.log")).unwrap();
    hosts.pipeline_act(&remote, "task/limit", "retryJob", "77").unwrap();
    hosts.pipeline_act(&remote, "task/limit", "retryFailed", "30").unwrap();
    hosts.pipeline_act(&remote, "task/limit", "retryRun", "30").unwrap();
    hosts.pipeline_act(&remote, "task/limit", "cancelRun", "29").unwrap();
    assert_eq!(
        calls(&dir),
        [
            "run rerun --job 77 -R mrx/shop",
            "run rerun 30 --failed -R mrx/shop",
            "run rerun 30 -R mrx/shop",
            "run cancel 29 -R mrx/shop",
        ]
    );
    // GitHub starts a run from what its workflow listens to: there is nothing to start from here, and that is said.
    let refused = hosts.pipeline_act(&remote, "task/limit", "runNew", "").unwrap_err();
    assert!(refused.to_string().contains("GitHub has no way to do that from here"), "{refused}");

    // A branch without runs has no pipeline.
    std::fs::write(dir.join("runs.json"), "[]").unwrap();
    assert_eq!(hosts.pipeline(&remote, "task/limit", None).unwrap(), None);
}
