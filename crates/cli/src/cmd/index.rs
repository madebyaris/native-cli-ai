//! Semantic/BM25 index rebuild and search commands.

use std::path::Path;

pub async fn run_index_rebuild(
    workspace_root: &Path,
    include: &[String],
    json: bool,
) -> anyhow::Result<()> {
    if !nca_index::Index::is_available() {
        let msg = "semantic-index feature is disabled. Rebuild with \
                   `cargo build --release --features semantic-index`.";
        if json {
            println!("{}", serde_json::json!({"error": msg}));
        } else {
            eprintln!("{msg}");
        }
        std::process::exit(2);
    }
    let root = workspace_root.to_path_buf();
    let include_vec = include.to_vec();
    let count = tokio::task::spawn_blocking(move || -> anyhow::Result<usize> {
        let mut idx = nca_index::Index::open(&root)?;
        Ok(idx.rebuild(&include_vec)?)
    })
    .await??;
    if json {
        println!("{}", serde_json::json!({"indexed": count}));
    } else {
        println!("Indexed {count} files under {}", workspace_root.display());
    }
    Ok(())
}

pub async fn run_index_search(
    workspace_root: &Path,
    query: &str,
    limit: usize,
    json: bool,
) -> anyhow::Result<()> {
    if !nca_index::Index::is_available() {
        let msg = "semantic-index feature is disabled. Rebuild with \
                   `cargo build --release --features semantic-index`.";
        if json {
            println!("{}", serde_json::json!({"error": msg}));
        } else {
            eprintln!("{msg}");
        }
        std::process::exit(2);
    }
    let root = workspace_root.to_path_buf();
    let q = query.to_string();
    let hits = tokio::task::spawn_blocking(move || -> anyhow::Result<Vec<nca_index::SearchHit>> {
        let idx = nca_index::Index::open(&root)?;
        Ok(idx.search(&q, limit)?)
    })
    .await??;
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "query": query,
                "hits": hits,
            }))?
        );
    } else {
        for h in &hits {
            println!("{:>6.2}  {}  {}", h.score, h.path.display(), h.snippet);
        }
    }
    Ok(())
}
