#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use assert_cmd::Command;
use predicates::prelude::*;
use serde_json::Value;

static COUNTER: AtomicU64 = AtomicU64::new(0);

struct TestRepository {
    data: PathBuf,
    work: PathBuf,
}

impl TestRepository {
    fn new(label: &str) -> Self {
        let counter = COUNTER.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "scriptor-agent-repository-{label}-{}-{counter}",
            std::process::id()
        ));
        let data = root.join("data");
        let work = root.join("work");
        fs::create_dir_all(&data).expect("création des données de test");
        fs::create_dir_all(&work).expect("création du répertoire de travail");
        Self { data, work }
    }

    fn command(&self) -> Command {
        let mut command = Command::cargo_bin("scriptor").expect("binaire scriptor introuvable");
        command.env("XDG_DATA_HOME", &self.data);
        command
    }

    fn capture(&self, name: &str, contents: &[u8]) -> Value {
        let source = self.work.join(name);
        fs::write(&source, contents).expect("écriture de la Source");
        let created: Value = serde_json::from_slice(
            &self
                .command()
                .args([
                    "capture",
                    source.to_str().expect("chemin UTF-8"),
                    "--policy",
                    "safe-local@1",
                ])
                .assert()
                .success()
                .get_output()
                .stdout,
        )
        .expect("Job JSON valide");
        let job_id = created["job"]["job_id"]
            .as_str()
            .expect("identifiant de Job");
        serde_json::from_slice(
            &self
                .command()
                .args(["job", "wait", job_id, "--timeout-secs", "5"])
                .assert()
                .success()
                .get_output()
                .stdout,
        )
        .expect("Job final JSON valide")
    }

    fn repository_path(&self) -> PathBuf {
        self.data.join("scriptor").join("v2")
    }
}

impl Drop for TestRepository {
    fn drop(&mut self) {
        if let Some(root) = self.data.parent() {
            let _ = fs::remove_dir_all(root);
        }
    }
}

#[test]
fn list_has_stable_pages_and_verifiable_references() {
    let repository = TestRepository::new("list");
    repository.capture("alpha.txt", b"alpha");
    repository.capture("bravo.txt", b"bravo");
    repository.capture("charlie.txt", b"charlie");

    let first: Value = serde_json::from_slice(
        &repository
            .command()
            .args(["capture", "list", "--limit", "2"])
            .assert()
            .success()
            .get_output()
            .stdout,
    )
    .expect("liste JSON valide");
    let first_captures = first["captures"].as_array().expect("première page");
    assert_eq!(first_captures.len(), 2);
    assert!(first["next_cursor"].is_string());
    for capture in first_captures {
        assert_eq!(capture["reference"]["capture_id"], capture["capture_id"]);
        assert_eq!(capture["reference"]["artifact_id"], "proof-source");
        assert_eq!(
            capture["reference"]["sha256"].as_str().map(str::len),
            Some(64)
        );
    }

    let cursor = first["next_cursor"].as_str().expect("curseur opaque");
    let second: Value = serde_json::from_slice(
        &repository
            .command()
            .args(["capture", "list", "--limit", "2", "--cursor", cursor])
            .assert()
            .success()
            .get_output()
            .stdout,
    )
    .expect("liste JSON valide");
    let second_captures = second["captures"].as_array().expect("seconde page");
    assert_eq!(second_captures.len(), 1);
    assert!(second["next_cursor"].is_null());
    assert_ne!(
        first_captures[0]["capture_id"],
        second_captures[0]["capture_id"]
    );

    repository
        .command()
        .args(["capture", "list", "--limit", "101"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("limit must not exceed 100"));
}

#[test]
fn search_uses_its_projection_and_reports_when_it_is_unavailable() {
    let repository = TestRepository::new("search");
    let capture = repository.capture("record.txt", b"meeting notes");
    let capture_id = capture["capture_id"]
        .as_str()
        .expect("identifiant de Capture");

    let found: Value = serde_json::from_slice(
        &repository
            .command()
            .args(["capture", "search", "meeting"])
            .assert()
            .success()
            .get_output()
            .stdout,
    )
    .expect("recherche JSON valide");
    assert_eq!(found["captures"][0]["capture_id"], capture_id);

    fs::remove_file(repository.repository_path().join("index.json"))
        .expect("suppression de la projection de recherche");
    let unavailable: Value = serde_json::from_slice(
        &repository
            .command()
            .args(["capture", "search", "meeting"])
            .assert()
            .success()
            .get_output()
            .stdout,
    )
    .expect("erreur JSON valide");
    assert_eq!(unavailable["error"]["code"], "index_unavailable");

    repository
        .command()
        .args(["capture", "index", "rebuild"])
        .assert()
        .success();
    repository
        .command()
        .args(["capture", "search", "meeting"])
        .assert()
        .success()
        .stdout(predicate::str::contains(capture_id));

    repository
        .command()
        .args(["capture", "inspect", capture_id])
        .assert()
        .success();
}

#[test]
fn read_returns_a_bounded_text_excerpt_and_never_writes_binary_content() {
    let repository = TestRepository::new("read");
    let text = repository.capture("notes.txt", b"abcdefghij");
    let text_capture = text["capture_id"].as_str().expect("identifiant de Capture");

    let read: Value = serde_json::from_slice(
        &repository
            .command()
            .args([
                "capture",
                "read",
                text_capture,
                "proof-source",
                "--offset",
                "2",
                "--length",
                "4",
            ])
            .assert()
            .success()
            .get_output()
            .stdout,
    )
    .expect("lecture JSON valide");
    assert_eq!(read["reference"]["capture_id"], text_capture);
    assert_eq!(read["content"]["text"], "cdef");
    assert_eq!(read["content"]["offset"], 2);
    assert_eq!(read["content"]["length"], 4);

    let binary = repository.capture("photo.bin", &[0, 159, 146, 150]);
    let binary_capture = binary["capture_id"]
        .as_str()
        .expect("identifiant de Capture");
    repository
        .command()
        .args(["capture", "read", binary_capture, "proof-source"])
        .assert()
        .failure()
        .stdout(predicate::str::is_empty())
        .stderr(predicate::str::contains("binary artifacts cannot be read"));

    repository
        .command()
        .args([
            "capture",
            "read",
            text_capture,
            "proof-source",
            "--length",
            "1048577",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("length must not exceed 1048576"));
}
