use mfn_test_affected::graph::{
    CoverageManifest, CoveredTest, Error, GraphReader, GraphWriter, LineRange, SCHEMA_VERSION,
};
use serde_json::json;
use std::{
    fs,
    path::{Path, PathBuf},
};

fn lcov(root: &Path, name: &str, contents: &str) -> PathBuf {
    let path = root.join(name);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, contents).unwrap();
    path
}

fn hit(path: &Path, lines: &[(u32, u32)]) -> String {
    let mut record = format!("SF:{}\n", path.display());
    for (line, count) in lines {
        record.push_str(&format!("DA:{line},{count}\n"));
    }
    record.push_str("end_of_record\n");
    record
}

fn export(commit_sha: &str, tests: Vec<CoveredTest>) -> CoverageManifest {
    CoverageManifest {
        commit_sha: commit_sha.into(),
        run_id: "424242".into(),
        attempt: 2,
        signature: json!({
            "toolchain": "rustc 1.99.0 (fixture 2026-10-21)",
            "features": "all",
            "target": null,
            "profile": "ci",
            "environment": {"RUSTFLAGS": "-Cinstrument-coverage -Ccodegen-units=1 -Copt-level=0"},
        })
        .to_string(),
        tests,
    }
}

fn test(package: &str, binary: &str, name: &str, lcov: PathBuf) -> CoveredTest {
    CoveredTest {
        package: package.into(),
        binary: binary.into(),
        name: name.into(),
        lcov,
    }
}

fn range(start: u32, end: u32) -> LineRange {
    LineRange { start, end }
}

fn selectors(
    reader: &mut GraphReader,
    commit: &str,
    path: &str,
    ranges: &[LineRange],
) -> Vec<(String, String, String)> {
    reader
        .tests_covering(commit, path, ranges)
        .unwrap()
        .into_iter()
        .map(|case| (case.package, case.binary, case.name))
        .collect()
}

const ALPHA: (&str, &str, &str) = ("sample-core", "sample-core", "tests::alpha");
const BETA: (&str, &str, &str) = ("sample-core", "sample-core", "tests::beta");
const GAMMA: (&str, &str, &str) = ("sample-app", "sample-app::integration", "hits_core");

fn tuple((package, binary, name): (&str, &str, &str)) -> (String, String, String) {
    (package.into(), binary.into(), name.into())
}

fn fixture() -> (tempfile::TempDir, PathBuf, CoverageManifest) {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let core = root.join("crates/core/src/lib.rs");
    // Alpha: hit intervals [10,12] and [20,21]; line 13 is executable but not hit.
    let alpha = lcov(
        root,
        "alpha.lcov",
        &format!(
            "{}{}{}",
            hit(
                &core,
                &[(10, 1), (11, 2), (12, 1), (13, 0), (20, 1), (21, 1)]
            ),
            // Dependency sources outside the workspace are ignored.
            hit(Path::new("/registry/dependency/src/lib.rs"), &[(1, 5)]),
            // Relative source paths are already workspace-relative.
            hit(Path::new("crates/core/src/extra.rs"), &[(5, 1)]),
        ),
    );
    // Beta: hit intervals [12,13] and [30,30] in the core file.
    let beta = lcov(
        root,
        "nested/beta.lcov",
        &hit(&core, &[(12, 3), (13, 1), (30, 1)]),
    );
    let graph = root.join("graph.sqlite");
    let manifest = export(
        "1111111111111111111111111111111111111111",
        vec![
            test(ALPHA.0, ALPHA.1, ALPHA.2, alpha),
            test(BETA.0, BETA.1, BETA.2, beta),
        ],
    );
    (directory, graph, manifest)
}

#[test]
fn indexes_compressed_intervals_and_answers_range_queries() {
    let (directory, graph, manifest) = fixture();
    let root = directory.path();
    let mut writer = GraphWriter::create(&graph).unwrap();
    let indexed = writer.index(&manifest, root).unwrap();
    assert_eq!(
        indexed,
        mfn_test_affected::graph::IndexedCommit {
            commit_sha: manifest.commit_sha.clone(),
            tests: 2,
            files: 2,
            // Alpha [10,12], [20,21], extra [5,5]; beta [12,13], [30,30].
            ranges: 5,
        }
    );
    drop(writer);

    let mut reader = GraphReader::open(&graph).unwrap();
    let record = reader.record(&manifest.commit_sha).unwrap().unwrap();
    assert_eq!(record.schema_version, SCHEMA_VERSION);
    assert_eq!(record.run_id, "424242");
    assert_eq!(record.attempt, 2);
    assert_eq!(record.signature, manifest.signature);
    assert!(record.created_at > 0);

    let file = "crates/core/src/lib.rs";
    let alpha = || vec![tuple(ALPHA)];
    let beta = || vec![tuple(BETA)];
    let both = || vec![tuple(ALPHA), tuple(BETA)];
    // Interval hits, including ranges spanning several stored intervals.
    assert_eq!(
        selectors(&mut reader, &manifest.commit_sha, file, &[range(10, 10)]),
        alpha()
    );
    assert_eq!(
        selectors(&mut reader, &manifest.commit_sha, file, &[range(12, 12)]),
        both()
    );
    assert_eq!(
        selectors(&mut reader, &manifest.commit_sha, file, &[range(11, 21)]),
        both()
    );
    assert_eq!(
        selectors(&mut reader, &manifest.commit_sha, file, &[range(12, 30)]),
        both()
    );
    assert_eq!(
        selectors(&mut reader, &manifest.commit_sha, file, &[range(30, 30)]),
        beta()
    );
    // Several changed ranges resolve in one call.
    assert_eq!(
        selectors(
            &mut reader,
            &manifest.commit_sha,
            file,
            &[range(1, 5), range(30, 32)]
        ),
        beta()
    );
    // Executable-but-unhit and uncovered lines yield nothing.
    assert_eq!(
        selectors(&mut reader, &manifest.commit_sha, file, &[range(13, 13)]),
        beta()
    );
    assert!(selectors(&mut reader, &manifest.commit_sha, file, &[range(14, 19)]).is_empty());
    // The relative-path record is queryable by its workspace-relative path.
    assert_eq!(
        selectors(
            &mut reader,
            &manifest.commit_sha,
            "crates/core/src/extra.rs",
            &[range(5, 5)]
        ),
        alpha()
    );
    // Files outside the workspace were never indexed.
    assert!(
        selectors(
            &mut reader,
            &manifest.commit_sha,
            "/registry/dependency/src/lib.rs",
            &[range(1, 1)]
        )
        .is_empty()
    );
}

#[test]
fn rejects_duplicate_commits_and_keeps_commits_isolated() {
    let (directory, graph, manifest) = fixture();
    let root = directory.path();
    let mut writer = GraphWriter::create(&graph).unwrap();
    writer.index(&manifest, root).unwrap();
    let duplicate = writer.index(&manifest, root).err().unwrap();
    assert!(matches!(duplicate, Error::DuplicateCommit { .. }));

    // A second commit shares test and file identities but answers independently.
    let gamma = lcov(
        root,
        "gamma.lcov",
        &hit(&root.join("crates/core/src/lib.rs"), &[(21, 1)]),
    );
    let mut second = export(
        "2222222222222222222222222222222222222222",
        vec![
            test(BETA.0, BETA.1, BETA.2, root.join("nested/beta.lcov")),
            test(GAMMA.0, GAMMA.1, GAMMA.2, gamma),
        ],
    );
    second.run_id = "434343".into();
    let indexed = writer.index(&second, root).unwrap();
    assert_eq!(indexed.tests, 2);
    assert_eq!(indexed.files, 1);
    assert_eq!(indexed.ranges, 3);
    drop(writer);

    let mut reader = GraphReader::open(&graph).unwrap();
    let file = "crates/core/src/lib.rs";
    assert_eq!(
        selectors(&mut reader, &second.commit_sha, file, &[range(21, 21)]),
        vec![tuple(GAMMA)]
    );
    assert_eq!(
        selectors(&mut reader, &second.commit_sha, file, &[range(12, 12)]),
        vec![tuple(BETA)]
    );
    assert!(selectors(&mut reader, &second.commit_sha, file, &[range(10, 10)]).is_empty());
    assert!(
        selectors(
            &mut reader,
            &second.commit_sha,
            "crates/core/src/extra.rs",
            &[range(5, 5)]
        )
        .is_empty()
    );
    assert_eq!(
        selectors(&mut reader, &manifest.commit_sha, file, &[range(10, 10)]),
        vec![tuple(ALPHA)]
    );
}

#[test]
fn missing_commits_files_and_empty_ranges_yield_no_selectors() {
    let (directory, graph, manifest) = fixture();
    let root = directory.path();
    GraphWriter::create(&graph)
        .unwrap()
        .index(&manifest, root)
        .unwrap();
    let mut reader = GraphReader::open(&graph).unwrap();
    assert!(reader.record("missing").unwrap().is_none());
    let file = "crates/core/src/lib.rs";
    assert!(selectors(&mut reader, "missing", file, &[range(1, 100)]).is_empty());
    assert!(
        selectors(
            &mut reader,
            &manifest.commit_sha,
            "missing.rs",
            &[range(1, 100)]
        )
        .is_empty()
    );
    assert!(selectors(&mut reader, &manifest.commit_sha, file, &[]).is_empty());
    assert!(matches!(
        reader.tests_covering(&manifest.commit_sha, file, &[range(0, 5)]),
        Err(Error::InvalidRange { .. })
    ));
    assert!(matches!(
        reader.tests_covering(&manifest.commit_sha, file, &[range(9, 3)]),
        Err(Error::InvalidRange { .. })
    ));
}

#[test]
fn error_variants_expose_display_messages_and_source_chains() {
    use std::error::Error as _;
    let clock_error = (std::time::UNIX_EPOCH - std::time::Duration::from_secs(1))
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_err();
    let variants: Vec<(Error, bool)> = vec![
        (
            Error::Missing {
                path: "graph.sqlite".into(),
            },
            false,
        ),
        (
            Error::Path {
                path: "graph.sqlite".into(),
            },
            false,
        ),
        (
            Error::Connect {
                path: "graph.sqlite".into(),
                source: diesel::ConnectionError::InvalidConnectionUrl("sqlite://x".into()),
            },
            true,
        ),
        (
            Error::Migration {
                path: "graph.sqlite".into(),
                source: Box::new(std::io::Error::other("failed")),
            },
            true,
        ),
        (
            Error::Database {
                operation: "read coverage graph records",
                source: diesel::result::Error::NotFound,
            },
            true,
        ),
        (
            // Transparent: the inner error is the representation, not a source.
            Error::Query {
                source: diesel::result::Error::NotFound,
            },
            false,
        ),
        (
            Error::DuplicateCommit {
                commit_sha: "1".repeat(40),
            },
            false,
        ),
        (
            Error::InvalidManifest {
                field: "tests",
                reason: "test identities must be unique",
            },
            false,
        ),
        (
            Error::Io {
                action: "read coverage manifest",
                path: "manifest.json".into(),
                source: std::io::Error::new(std::io::ErrorKind::NotFound, "gone"),
            },
            true,
        ),
        (
            Error::Json {
                action: "decode coverage manifest",
                source: serde_json::from_str::<()>("not json").unwrap_err(),
            },
            true,
        ),
        (
            Error::Lcov {
                path: "test.lcov".into(),
                record: "SF",
            },
            false,
        ),
        (
            Error::LcovNumber {
                path: "test.lcov".into(),
                record: "DA",
                source: "ten".parse::<u32>().unwrap_err(),
            },
            true,
        ),
        (Error::InvalidRange { start: 0, end: 5 }, false),
        (
            Error::Clock {
                source: clock_error,
            },
            true,
        ),
        (
            Error::Timestamp {
                source: i64::try_from(u64::MAX).unwrap_err(),
            },
            true,
        ),
    ];
    let mut messages = std::collections::BTreeSet::new();
    for (variant, sourced) in &variants {
        let message = variant.to_string();
        assert!(!message.is_empty(), "{variant:?}");
        assert_eq!(variant.source().is_some(), *sourced, "{variant:?}");
        messages.insert(message);
    }
    // Distinct variants carry distinct messages.
    assert_eq!(messages.len(), variants.len());
}

#[test]
fn has_file_distinguishes_absent_files_per_commit() {
    let (directory, graph, manifest) = fixture();
    let root = directory.path();
    let mut writer = GraphWriter::create(&graph).unwrap();
    writer.index(&manifest, root).unwrap();
    let gamma = lcov(
        root,
        "gamma.lcov",
        &hit(&root.join("crates/core/src/only-second.rs"), &[(3, 1)]),
    );
    let second = export(
        "2222222222222222222222222222222222222222",
        vec![test(GAMMA.0, GAMMA.1, GAMMA.2, gamma)],
    );
    writer.index(&second, root).unwrap();
    drop(writer);
    let mut reader = GraphReader::open(&graph).unwrap();
    let first = "1111111111111111111111111111111111111111";
    assert!(reader.has_file(first, "crates/core/src/lib.rs").unwrap());
    assert!(reader.has_file(first, "crates/core/src/extra.rs").unwrap());
    assert!(
        !reader
            .has_file(first, "crates/core/src/missing.rs")
            .unwrap()
    );
    // The file table is shared; per-commit presence still differs.
    assert!(
        !reader
            .has_file(first, "crates/core/src/only-second.rs")
            .unwrap()
    );
    assert!(
        reader
            .has_file(&second.commit_sha, "crates/core/src/only-second.rs")
            .unwrap()
    );
    assert!(
        !reader
            .has_file("missing", "crates/core/src/lib.rs")
            .unwrap()
    );
}

#[test]
fn rejects_malformed_manifests_exports_and_identities() {
    let (directory, graph, manifest) = fixture();
    let root = directory.path();
    let mut writer = GraphWriter::create(&graph).unwrap();

    // A malformed export fails before anything is written.
    let broken = lcov(
        root,
        "broken.lcov",
        "SF:crates/core/src/lib.rs\nDA:ten,1\nend_of_record\n",
    );
    let mut invalid = export("3333333333333333333333333333333333333333", vec![]);
    invalid.tests.push(test(ALPHA.0, ALPHA.1, ALPHA.2, broken));
    assert!(matches!(
        writer.index(&invalid, root),
        Err(Error::LcovNumber { .. })
    ));
    assert!(
        GraphReader::open(&graph)
            .unwrap()
            .record(&invalid.commit_sha)
            .unwrap()
            .is_none()
    );

    invalid.tests[0].lcov = root.join("absent.lcov");
    assert!(matches!(
        writer.index(&invalid, root),
        Err(Error::Io { .. })
    ));

    let mut invalid = export("", vec![]);
    assert!(matches!(
        writer.index(&invalid, root),
        Err(Error::InvalidManifest {
            field: "commit_sha",
            ..
        })
    ));
    invalid.commit_sha = "3333333333333333333333333333333333333333".into();
    invalid.tests = vec![
        test(ALPHA.0, ALPHA.1, ALPHA.2, root.join("alpha.lcov")),
        test(ALPHA.0, ALPHA.1, ALPHA.2, root.join("alpha.lcov")),
    ];
    assert!(matches!(
        writer.index(&invalid, root),
        Err(Error::InvalidManifest { field: "tests", .. })
    ));

    // Manifest files decode strictly and resolve relative lcov paths.
    let manifest_path = root.join("manifest.json");
    let mut encoded = serde_json::to_value(&manifest).unwrap();
    encoded["tests"][0]["lcov"] = json!("alpha.lcov");
    fs::write(&manifest_path, encoded.to_string()).unwrap();
    let decoded = CoverageManifest::read(&manifest_path).unwrap();
    assert_eq!(decoded.tests[0].lcov, root.join("alpha.lcov"));
    fs::write(&manifest_path, "{\"commit_sha\":").unwrap();
    assert!(matches!(
        CoverageManifest::read(&manifest_path),
        Err(Error::Json { .. })
    ));
}
