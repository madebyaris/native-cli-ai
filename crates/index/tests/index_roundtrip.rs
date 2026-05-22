//! Index round-trip: `FeatureDisabled` stub without `semantic-index`, rebuild+search with it.

use nca_index::{Index, IndexError};

#[test]
fn feature_disabled_without_semantic_index_feature() {
    if Index::is_available() {
        return;
    }

    let temp = tempfile::tempdir().expect("tempdir");
    assert!(matches!(
        Index::open(temp.path()),
        Err(IndexError::FeatureDisabled)
    ));
    assert!(!Index::is_available());
}

#[test]
#[cfg(feature = "semantic-index")]
fn rebuild_and_search_roundtrip() {
    let temp = tempfile::tempdir().expect("tempdir");
    let src = temp.path().join("src");
    std::fs::create_dir_all(&src).expect("mkdir src");
    std::fs::write(
        src.join("main.rs"),
        "fn main() { println!(\"hello nca index\"); }\n",
    )
    .expect("write");

    let mut index = Index::open(temp.path()).expect("open index");
    let count = index
        .rebuild(&["*.rs".to_string()])
        .expect("rebuild should index rust files");
    assert_eq!(count, 1);

    let hits = index.search("hello nca", 5).expect("search");
    assert!(!hits.is_empty());
    assert!(hits[0].path.to_string_lossy().contains("main.rs"));
    assert!(hits[0].snippet.contains("hello"));
    assert!(Index::is_available());
}

#[test]
#[cfg(feature = "semantic-index")]
fn search_returns_empty_for_missing_terms() {
    let temp = tempfile::tempdir().expect("tempdir");
    std::fs::write(temp.path().join("readme.txt"), "nothing special").expect("write");

    let mut index = Index::open(temp.path()).expect("open");
    index.rebuild(&[]).expect("rebuild");
    let hits = index.search("xyzzy_nonexistent_token", 5).expect("search");
    assert!(hits.is_empty());
}
