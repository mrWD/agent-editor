//! Health: what each project is missing, read from its own files.

mod common;

use common::*;
use serde_json::{Value, json};
use std::path::Path;

fn titles(health: &Value) -> Vec<String> {
    health["findings"].as_array().unwrap().iter().map(|f| f["title"].as_str().unwrap().to_string()).collect()
}

#[tokio::test]
async fn what_a_project_is_missing_is_read_from_its_files() {
    let sandbox = Sandbox::new("health");
    let repo = git_project(&sandbox);
    // A project with a lint and tests, on GitHub, whose pipeline runs only the build.
    std::fs::write(
        repo.join("package.json"),
        json!({ "scripts": { "lint": "eslint .", "test": "vitest run" }, "dependencies": { "express": "5" } })
            .to_string(),
    )
    .unwrap();
    std::fs::create_dir_all(repo.join(".github/workflows")).unwrap();
    std::fs::write(repo.join(".github/workflows/ci.yml"), "jobs:\n  build:\n    steps:\n      - run: npm run build\n")
        .unwrap();
    std::fs::write(repo.join(".env"), "SECRET=1\n").unwrap();
    std::fs::write(repo.join(".env.example"), "SECRET=\n").unwrap();
    git(&repo, &["add", "-f", "."]);
    git(&repo, &["-c", "user.name=T", "-c", "user.email=t@example.com", "commit", "-qm", "More"]);
    git(&repo, &["remote", "add", "origin", "git@github.com:x/shop.git"]);
    let manager = sandbox.manager();
    let project = manager.add_project(&repo.to_string_lossy(), None).unwrap();

    let health = manager.health().unwrap();
    assert_eq!(
        titles(&health),
        [
            "A file that may hold secrets is in git: .env",
            "Lint does not run before merging",
            "Tests do not run before merging",
            "There is no .gitignore",
            "Dependencies have no lock file",
        ],
        "what matters most comes first; the example file is not a secret"
    );
    let first = &health["findings"][0];
    assert_eq!(
        (first["category"].as_str(), first["projectId"].as_str()),
        (Some("Security"), Some(project.id.as_str()))
    );
    assert!(first["fix"].as_str().unwrap().contains("Take them out of git without deleting them from disk"));
    assert_eq!(health["ignored"], 0);
    assert_eq!(health["auditedAt"], Value::Null);
    // The project's checks, each with whether it is on.
    let checks = health["projects"][0]["checks"].as_array().unwrap();
    let brief: Vec<(&str, bool)> =
        checks.iter().map(|c| (c["id"].as_str().unwrap(), c["on"].as_bool().unwrap())).collect();
    assert_eq!(brief, [("lint", true), ("test", true)]);

    // Putting things right takes the findings away.
    std::fs::write(repo.join(".github/workflows/ci.yml"), "steps:\n  - run: npm run lint\n  - run: npm test\n")
        .unwrap();
    std::fs::write(repo.join(".gitignore"), "node_modules/\n").unwrap();
    std::fs::write(repo.join("package-lock.json"), "{}").unwrap();
    git(&repo, &["rm", "-q", "--cached", ".env"]);
    assert!(titles(&manager.health().unwrap()).is_empty());
}

#[tokio::test]
async fn a_project_with_nothing_set_up_and_one_that_is_no_code() {
    let sandbox = Sandbox::new("health-bare");
    let repo = git_project(&sandbox);
    std::fs::write(repo.join("Cargo.toml"), "[package]\nname = \"x\"\n").unwrap();
    std::fs::write(repo.join(".gitignore"), "target/\n").unwrap();
    git(&repo, &["remote", "add", "origin", "https://gitlab.com/group/x.git"]);
    let manager = sandbox.manager();
    manager.add_project(&repo.to_string_lossy(), None).unwrap();
    // Rust brings its own lint and tests; what is missing is the pipeline.
    assert_eq!(titles(&manager.health().unwrap()), ["Nothing runs before a merge"]);

    // A folder of notes is not a project that needs tests.
    let notes = Sandbox::new("health-notes");
    std::fs::write(notes.project().join("README.md"), "# Notes\n").unwrap();
    let manager = notes.manager();
    manager.add_project(&notes.project().to_string_lossy(), None).unwrap();
    assert!(titles(&manager.health().unwrap()).is_empty());
}

#[tokio::test]
async fn findings_are_ignored_and_brought_back_and_checks_are_turned_per_project() {
    let sandbox = Sandbox::new("health-ignore");
    let repo = git_project(&sandbox);
    std::fs::write(
        repo.join("package.json"),
        json!({ "scripts": { "test": "vitest run", "e2e": "playwright test" } }).to_string(),
    )
    .unwrap();
    std::fs::write(repo.join(".gitignore"), "node_modules/\n").unwrap();
    // Committed, so a task's own working copy has the scripts too.
    git(&repo, &["add", "."]);
    git(&repo, &["-c", "user.name=T", "-c", "user.email=t@example.com", "commit", "-qm", "Scripts"]);
    let manager = sandbox.manager();
    let project = manager.add_project(&repo.to_string_lossy(), None).unwrap();
    let health = manager.health().unwrap();
    assert_eq!(titles(&health), ["There is no lint command"]);

    manager.health_ignore(health["findings"][0]["id"].as_str()).unwrap();
    let health = manager.health().unwrap();
    assert!(titles(&health).is_empty());
    assert_eq!(health["ignored"], 1);
    manager.health_ignore(None).unwrap();
    assert_eq!(titles(&manager.health().unwrap()).len(), 1);

    // Turning a check off here turns it off for every task of the project.
    let mut events = manager.subscribe();
    let task = manager.create(&project.id, "mock", "What does the limiter do?").await.unwrap();
    until(&mut events, &task.row.id, |e| state_is(e, "idle")).await;
    manager.project_check_turn(&project.id, "test", false).unwrap();
    manager.project_check_turn(&project.id, "e2e", true).unwrap();
    let checks = manager.health().unwrap()["projects"][0]["checks"].clone();
    assert_eq!((checks[0]["on"].as_bool(), checks[1]["on"].as_bool()), (Some(false), Some(true)));
    let state = manager.checks_state(&task.row.id).unwrap();
    assert_eq!((state["checks"][0]["on"].as_bool(), state["checks"][1]["on"].as_bool()), (Some(false), Some(true)));
    assert!(manager.project_check_turn(&project.id, "nope", true).is_err());
}

#[tokio::test]
async fn the_audit_of_the_package_manager_becomes_a_finding() {
    let sandbox = Sandbox::new("health-audit");
    let repo = git_project(&sandbox);
    std::fs::write(repo.join(".gitignore"), "node_modules/\n").unwrap();
    let manager = sandbox.manager();
    manager.add_project(&repo.to_string_lossy(), None).unwrap();
    assert!(titles(&manager.health().unwrap()).is_empty());

    // The package manager is not asked for real here: what it would answer is given.
    let answer = |_: &Path| {
        Some(
            json!({ "metadata": { "vulnerabilities": { "low": 1, "high": 2, "critical": 1, "total": 4 } } })
                .to_string(),
        )
    };
    manager.health_audit_with(&answer).unwrap();
    let health = manager.health().unwrap();
    assert_eq!(titles(&health), ["4 dependencies have known vulnerabilities, 1 critical and 2 high"]);
    assert_eq!(health["findings"][0]["category"], "Security");
    assert!(health["auditedAt"].as_u64().is_some());

    // A clean audit, or a project the package manager cannot audit, leaves nothing.
    manager
        .health_audit_with(&|_: &Path| Some(json!({ "metadata": { "vulnerabilities": { "total": 0 } } }).to_string()))
        .unwrap();
    assert!(titles(&manager.health().unwrap()).is_empty());
    manager.health_audit_with(&|_: &Path| None).unwrap();
    assert!(titles(&manager.health().unwrap()).is_empty());
    assert_eq!(ae_core::health::read_npm_audit("not json"), None);
}
