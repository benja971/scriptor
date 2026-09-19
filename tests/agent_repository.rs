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

    fn rebuild_command(&self) -> std::process::Command {
        let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_scriptor"));
        command
            .env("XDG_DATA_HOME", &self.data)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .args(["capture", "index", "rebuild"]);
        command
    }

    fn capture(&self, name: &str, contents: &[u8]) -> Value {
        self.capture_with_policy(name, contents, "safe-local@1")
    }

    fn capture_with_policy(&self, name: &str, contents: &[u8], policy: &str) -> Value {
        let source = self.work.join(name);
        fs::write(&source, contents).expect("écriture de la Source");
        let created: Value = serde_json::from_slice(
            &self
                .command()
                .args([
                    "capture",
                    source.to_str().expect("chemin UTF-8"),
                    "--policy",
                    policy,
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

    fn inspect(&self, capture_id: &str) -> Value {
        serde_json::from_slice(
            &self
                .command()
                .args(["capture", "inspect", capture_id])
                .assert()
                .success()
                .get_output()
                .stdout,
        )
        .expect("Capture inspectée JSON valide")
    }

    fn list(&self) -> Value {
        serde_json::from_slice(
            &self
                .command()
                .args(["capture", "list"])
                .assert()
                .success()
                .get_output()
                .stdout,
        )
        .expect("liste de Captures JSON valide")
    }
}

fn explicit_local_policy(duplicate_mode: &str) -> String {
    serde_json::json!({
        "id": "safe-local",
        "version": 1,
        "snapshot": {
            "duplicate_mode": duplicate_mode,
            "limits": {
                "max_depth": 2,
                "max_sources": 50,
                "max_download_bytes": 2 * 1024 * 1024 * 1024_u64,
                "max_disk_bytes": 10 * 1024 * 1024 * 1024_u64,
                "max_duration_secs": 30 * 60,
                "max_concurrency": 2
            },
            "allows_remote_calls": false,
            "allowed_providers": [
                "ffmpeg",
                "ffprobe",
                "whisper-cli",
                "pdftotext",
                "pdfinfo",
                "tesseract",
                "scriptor-local-derive"
            ],
            "allowed_recipes": [
                "structured-summary",
                "proven-claims",
                "checklist",
                "markdown-note",
                "sourced-answer"
            ]
        }
    })
    .to_string()
}

impl Drop for TestRepository {
    fn drop(&mut self) {
        if let Some(root) = self.data.parent() {
            let _ = fs::remove_dir_all(root);
        }
    }
}

#[test]
fn capture_continue_requires_a_known_capture_and_an_explicit_policy() {
    let repository = TestRepository::new("continue-contract");
    repository
        .command()
        .args(["capture", "continue", "capture-1-1"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"code\":\"policy_required\""));
}

#[test]
fn duplicate_mode_create_publishes_versioned_observations() {
    let repository = TestRepository::new("json-policy");
    let policy = explicit_local_policy("create");
    let first = repository.capture_with_policy("source.txt", b"same Source", &policy);
    let second = repository.capture_with_policy("source.txt", b"same Source", &policy);

    assert_eq!(first["state"], "succeeded");
    assert_eq!(second["state"], "succeeded");
    assert_ne!(first["capture_id"], second["capture_id"]);
    assert_eq!(first["policy"]["id"], "safe-local");
    assert_eq!(first["policy"]["version"], 1);
    assert_eq!(first["policy"]["snapshot"]["duplicate_mode"], "create");
    assert_eq!(first["policy"]["sha256"].as_str().map(str::len), Some(64));
    let first_capture_id = first["capture_id"]
        .as_str()
        .expect("identifiant de première Capture");
    let second_capture_id = second["capture_id"]
        .as_str()
        .expect("identifiant de seconde Capture");
    assert_eq!(
        repository.inspect(first_capture_id)["manifest"]["capture_version"],
        1
    );
    assert_eq!(
        repository.inspect(second_capture_id)["manifest"]["capture_version"],
        2
    );
}

#[test]
fn duplicate_mode_reuse_keeps_the_existing_capture() {
    let repository = TestRepository::new("duplicate-reuse");
    let policy = explicit_local_policy("reuse");
    let first = repository.capture_with_policy("source.txt", b"same Source", &policy);
    let second = repository.capture_with_policy("source.txt", b"same Source", &policy);

    assert_eq!(first["state"], "succeeded");
    assert_eq!(second["state"], "succeeded");
    assert_eq!(first["capture_id"], second["capture_id"]);
    let capture_id = first["capture_id"]
        .as_str()
        .expect("identifiant de Capture réutilisée");
    assert_eq!(
        repository.inspect(capture_id)["manifest"]["capture_version"],
        1
    );
    assert_eq!(
        repository.list()["captures"]
            .as_array()
            .expect("liste de Captures")
            .len(),
        1
    );
}

#[test]
fn duplicate_mode_fail_returns_a_failed_job_without_publishing() {
    let repository = TestRepository::new("duplicate-fail");
    let first = repository.capture("source.txt", b"same Source");
    let failed = repository.capture_with_policy(
        "source.txt",
        b"same Source",
        &explicit_local_policy("fail"),
    );

    assert_eq!(failed["state"], "failed");
    assert!(failed["capture_id"].is_null());
    assert!(failed["error"]["code"].is_string());
    assert!(
        failed["error"]["message"]
            .as_str()
            .is_some_and(|message| message.contains("Capture already exists"))
    );
    assert_eq!(
        repository.list()["captures"]
            .as_array()
            .expect("liste de Captures")
            .len(),
        1
    );
    assert_eq!(
        repository.list()["captures"][0]["capture_id"],
        first["capture_id"]
    );
}

#[test]
fn json_policy_rejects_unknown_fields_and_inconsistent_allowlists() {
    let repository = TestRepository::new("json-policy-invalid");
    let source = repository.work.join("source.txt");
    fs::write(&source, b"Source").expect("écriture de la Source");
    let mut policy: Value =
        serde_json::from_str(&explicit_local_policy("reuse")).expect("Policy JSON valide");
    policy["unexpected"] = Value::Bool(true);

    repository
        .command()
        .args([
            "capture",
            source.to_str().expect("chemin UTF-8"),
            "--policy",
            &policy.to_string(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"code\":\"invalid_policy\""));

    policy
        .as_object_mut()
        .expect("Policy objet")
        .remove("unexpected");
    policy["snapshot"]["allows_remote_calls"] = Value::Bool(true);
    repository
        .command()
        .args([
            "capture",
            source.to_str().expect("chemin UTF-8"),
            "--policy",
            &policy.to_string(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"code\":\"invalid_policy\""));
}

#[test]
fn policy_aliases_keep_their_existing_normalized_snapshot() {
    let repository = TestRepository::new("policy-alias");
    let capture = repository.capture("source.txt", b"alias policy");

    assert_eq!(capture["policy"]["id"], "safe-local");
    assert_eq!(capture["policy"]["version"], 1);
    assert_eq!(capture["policy"]["snapshot"]["duplicate_mode"], "reuse");
    assert_eq!(capture["policy"]["sha256"].as_str().map(str::len), Some(64));
}

#[test]
fn published_capture_declares_its_format_version() {
    let repository = TestRepository::new("manifest-format-version");
    let capture = repository.capture("source.txt", b"versioned manifest");
    let capture_id = capture["capture_id"]
        .as_str()
        .expect("identifiant de Capture");
    let manifest: Value = serde_json::from_slice(
        &fs::read(
            repository
                .repository_path()
                .join("captures")
                .join(capture_id)
                .join("manifest.json"),
        )
        .expect("lecture du manifest"),
    )
    .expect("manifest JSON valide");

    assert_eq!(manifest["format_version"], 1);
}

#[test]
fn inspect_rejects_missing_or_unknown_capture_format_versions() {
    for (label, version) in [("missing", None), ("unknown", Some(2))] {
        let repository = TestRepository::new(&format!("manifest-format-{label}"));
        let capture = repository.capture("source.txt", b"versioned manifest");
        let capture_id = capture["capture_id"]
            .as_str()
            .expect("identifiant de Capture");
        let manifest_path = repository
            .repository_path()
            .join("captures")
            .join(capture_id)
            .join("manifest.json");
        let mut manifest: Value =
            serde_json::from_slice(&fs::read(&manifest_path).expect("lecture du manifest"))
                .expect("manifest JSON valide");
        match version {
            Some(version) => manifest["format_version"] = Value::from(version),
            None => {
                manifest
                    .as_object_mut()
                    .expect("objet manifest")
                    .remove("format_version");
            }
        }
        fs::write(
            &manifest_path,
            serde_json::to_vec(&manifest).expect("sérialisation du manifest"),
        )
        .expect("écriture du manifest corrompu");

        repository
            .command()
            .args(["capture", "inspect", capture_id])
            .assert()
            .success()
            .stdout(
                predicate::str::contains("manifest format version")
                    .or(predicate::str::contains("missing field `format_version`")),
            );
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
    let cursor = first["next_cursor"].as_str().expect("curseur opaque");
    assert!(cursor.len() < 80, "le curseur ne divulgue pas son état");
    for capture in first_captures {
        assert_eq!(capture["reference"]["capture_id"], capture["capture_id"]);
        assert_eq!(capture["reference"]["artifact_id"], "proof-source");
        assert_eq!(capture["reference"]["locator"]["kind"], "file");
        assert_eq!(
            capture["reference"]["sha256"].as_str().map(str::len),
            Some(64)
        );
    }

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

    repository.capture("delta.txt", b"delta");
    repository
        .command()
        .args(["capture", "list", "--cursor", cursor])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"code\":\"invalid_cursor\""));

    repository
        .command()
        .args(["capture", "list", "--limit", "101"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"code\":\"invalid_pagination\""))
        .stderr(predicate::str::is_empty());

    let forged_cursor = format!("{cursor}forged");
    repository
        .command()
        .args(["capture", "list", "--cursor", &forged_cursor])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"code\":\"invalid_cursor\""));
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

    repository
        .command()
        .args(["capture", "search", "meeting", "--cursor", "invalid"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"code\":\"invalid_cursor\""))
        .stderr(predicate::str::is_empty());

    repository.capture("other.txt", b"other");
    let list_cursor: Value = serde_json::from_slice(
        &repository
            .command()
            .args(["capture", "list", "--limit", "1"])
            .assert()
            .success()
            .get_output()
            .stdout,
    )
    .expect("liste JSON valide");
    repository
        .command()
        .args([
            "capture",
            "search",
            "meeting",
            "--cursor",
            list_cursor["next_cursor"]
                .as_str()
                .expect("curseur de liste"),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"code\":\"invalid_cursor\""));

    fs::remove_dir_all(repository.repository_path().join("index"))
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
fn search_paginates_scored_results_without_skipping_captures() {
    let repository = TestRepository::new("search-pagination");
    repository.capture("first.txt", b"needle");
    repository.capture("second.txt", b"needle needle needle");
    repository.capture("third.txt", b"needle needle");
    let expected: Value = serde_json::from_slice(
        &repository
            .command()
            .args(["capture", "search", "needle", "--limit", "3"])
            .assert()
            .success()
            .get_output()
            .stdout,
    )
    .expect("résultats de recherche JSON valides");
    let expected = expected["captures"]
        .as_array()
        .expect("résultats de recherche")
        .iter()
        .map(|capture| {
            capture["capture_id"]
                .as_str()
                .expect("identifiant de Capture")
                .to_string()
        })
        .collect::<Vec<_>>();

    let mut cursor = None;
    let mut found = Vec::new();
    loop {
        let mut command = repository.command();
        command.args(["capture", "search", "needle", "--limit", "1"]);
        if let Some(cursor) = cursor.as_deref() {
            command.args(["--cursor", cursor]);
        }
        let page: Value = serde_json::from_slice(&command.assert().success().get_output().stdout)
            .expect("page de recherche JSON valide");
        found.push(
            page["captures"][0]["capture_id"]
                .as_str()
                .expect("Capture trouvée")
                .to_string(),
        );
        cursor = page["next_cursor"].as_str().map(str::to_string);
        if cursor.is_none() {
            break;
        }
    }

    assert_eq!(found, expected);
}

#[test]
fn rebuilding_skips_invalid_utf8_text_and_keeps_other_captures_searchable() {
    let repository = TestRepository::new("invalid-utf8-index");
    repository.capture("invalid.txt", b"invalid\xfftext");
    let valid = repository.capture("valid.txt", b"searchable meeting notes");
    let valid_capture = valid["capture_id"]
        .as_str()
        .expect("identifiant de Capture valide");

    repository
        .command()
        .args(["capture", "index", "rebuild"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"captures\":2"))
        .stderr(predicate::str::is_empty());

    repository
        .command()
        .args(["capture", "search", "meeting"])
        .assert()
        .success()
        .stdout(predicate::str::contains(valid_capture))
        .stderr(predicate::str::is_empty());
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

    let listed: Value = serde_json::from_slice(
        &repository
            .command()
            .args(["capture", "list"])
            .assert()
            .success()
            .get_output()
            .stdout,
    )
    .expect("liste JSON valide");
    let reference = serde_json::to_string(&listed["captures"][0]["reference"])
        .expect("Référence JSON sérialisable");
    repository
        .command()
        .args(["capture", "read", "--reference", &reference])
        .assert()
        .success()
        .stdout(predicate::str::contains("abcdefghij"));

    let binary = repository.capture("photo.bin", &[0, 159, 146, 150]);
    let binary_capture = binary["capture_id"]
        .as_str()
        .expect("identifiant de Capture");
    repository
        .command()
        .args(["capture", "read", binary_capture, "proof-source"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"code\":\"binary_artifact\""))
        .stderr(predicate::str::is_empty());

    let unicode = repository.capture("unicode.txt", "éé".as_bytes());
    let unicode_capture = unicode["capture_id"]
        .as_str()
        .expect("identifiant de Capture");
    repository
        .command()
        .args([
            "capture",
            "read",
            unicode_capture,
            "proof-source",
            "--offset",
            "1",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"code\":\"invalid_range\""));

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
        .success()
        .stdout(predicate::str::contains("\"code\":\"invalid_range\""))
        .stderr(predicate::str::is_empty());
}

#[test]
fn read_rejects_a_manifest_path_outside_its_capture() {
    let repository = TestRepository::new("read-traversal");
    let capture = repository.capture("outside.txt", b"outside the Capture directory");
    let capture_id = capture["capture_id"]
        .as_str()
        .expect("identifiant de Capture");
    let manifest_path = repository
        .repository_path()
        .join("captures")
        .join(capture_id)
        .join("manifest.json");
    let mut manifest: Value =
        serde_json::from_slice(&fs::read(&manifest_path).expect("lecture du manifest de test"))
            .expect("manifest JSON valide");
    manifest["proof"]["path"] = Value::String("../../../../../work/outside.txt".to_string());
    fs::write(
        &manifest_path,
        serde_json::to_vec(&manifest).expect("sérialisation du manifest corrompu"),
    )
    .expect("écriture du manifest corrompu");

    repository
        .command()
        .args(["capture", "read", capture_id, "proof-source"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "invalid artifact path in Capture manifest",
        ))
        .stderr(predicate::str::is_empty());
}

#[test]
fn concurrent_index_rebuilds_publish_a_valid_projection() {
    let repository = TestRepository::new("concurrent-index");
    repository.capture("one.txt", b"first searchable text");
    repository.capture("two.txt", b"second searchable text");

    let mut first = repository
        .rebuild_command()
        .spawn()
        .expect("première reconstruction");
    let mut second = repository
        .rebuild_command()
        .spawn()
        .expect("seconde reconstruction");
    assert!(
        first
            .wait()
            .expect("attente première reconstruction")
            .success()
    );
    assert!(
        second
            .wait()
            .expect("attente seconde reconstruction")
            .success()
    );

    repository
        .command()
        .args(["capture", "search", "searchable"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"captures\""));
}

#[test]
fn agent_commands_return_structured_json_for_runtime_and_clap_errors() {
    let repository = TestRepository::new("agent-errors");

    repository
        .command()
        .args(["capture", "inspect", "capture-invalid"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"code\":\"invalid_identifier\""))
        .stderr(predicate::str::is_empty());
    repository
        .command()
        .args(["job", "get", "job-invalid"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"code\":\"invalid_identifier\""))
        .stderr(predicate::str::is_empty());
    repository
        .command()
        .args(["capture", "list", "--unknown-option"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"code\":\"invalid_command\""))
        .stderr(predicate::str::is_empty());
    repository
        .command()
        .args(["capture", "index", "rebuild"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"captures\":0"));
}

#[test]
fn index_failure_after_publication_does_not_fail_the_capture_job() {
    let repository = TestRepository::new("index-degradation");
    fs::create_dir_all(repository.repository_path()).expect("création du Référentiel");
    fs::write(
        repository.repository_path().join("index"),
        b"not a directory",
    )
    .expect("création d'une cible d'Index invalide");

    let finished = repository.capture("published.txt", b"published despite Index failure");
    assert_eq!(finished["state"], "succeeded");
    let capture_id = finished["capture_id"]
        .as_str()
        .expect("identifiant de Capture");
    repository
        .command()
        .args(["capture", "inspect", capture_id])
        .assert()
        .success();
    repository
        .command()
        .args(["capture", "search", "published"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"code\":\"index_degraded\""));
}
