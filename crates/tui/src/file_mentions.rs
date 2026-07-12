//! Inline `@path` file mentions: discovery, completion, and expansion into file contents.

use std::path::{Component, Path, PathBuf};

const DEFAULT_MAX_FILE_BYTES: usize = 512 * 1024;
const DISCOVER_MAX_FILES: usize = 4000;
const DISCOVER_MAX_DEPTH: usize = 12;

fn skip_dir_name(name: &str) -> bool {
    matches!(
        name,
        ".git" | "target" | "node_modules" | ".nca" | "dist" | "build"
    )
}

/// Walk workspace (bounded) and collect relative file paths for `@` completion.
pub fn discover_workspace_files(workspace: &Path) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut stack: Vec<(PathBuf, usize)> = vec![(workspace.to_path_buf(), 0)];

    while let Some((dir, depth)) = stack.pop() {
        if out.len() >= DISCOVER_MAX_FILES {
            break;
        }
        if depth > DISCOVER_MAX_DEPTH {
            continue;
        }
        let read_dir = match std::fs::read_dir(&dir) {
            Ok(r) => r,
            Err(_) => continue,
        };
        for ent in read_dir.flatten() {
            if out.len() >= DISCOVER_MAX_FILES {
                break;
            }
            let name = ent.file_name().to_string_lossy().to_string();
            if skip_dir_name(&name) {
                continue;
            }
            let path = ent.path();
            let Ok(ft) = ent.file_type() else {
                continue;
            };
            if ft.is_dir() {
                stack.push((path, depth + 1));
            } else if ft.is_file()
                && let Ok(rel) = path.strip_prefix(workspace)
            {
                let s = rel.to_string_lossy().replace('\\', "/");
                if !s.is_empty() && !s.starts_with("../") {
                    out.push(s);
                }
            }
        }
    }

    out.sort();
    out.dedup();
    out
}

/// Paths whose relative path (unix-style) starts with `prefix` (case-sensitive on Unix).
pub fn filter_paths_prefix(paths: &[String], prefix: &str) -> Vec<String> {
    let p = prefix.trim();
    if p.is_empty() {
        return paths.iter().take(200).cloned().collect();
    }
    let mut v: Vec<String> = paths
        .iter()
        .filter(|s| s.starts_with(p))
        .take(200)
        .cloned()
        .collect();
    if v.len() < 50 {
        let pl = p.to_ascii_lowercase();
        for s in paths {
            if v.len() >= 200 {
                break;
            }
            if s.to_ascii_lowercase().starts_with(&pl) && !v.iter().any(|x| x == s) {
                v.push(s.clone());
            }
        }
    }
    v
}

fn is_at_mention_boundary(prev: Option<char>) -> bool {
    match prev {
        None => true,
        Some(c) => c.is_whitespace() || matches!(c, '(' | '[' | '{' | '<' | '"' | '\'' | '`'),
    }
}

fn is_path_char(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, '/' | '.' | '_' | '-' | ':' | '\\')
}

/// Byte range and path string for one `@mention` (path does not include `@`).
pub fn parse_at_mentions(text: &str) -> Vec<(usize, usize, String)> {
    let mut out = Vec::new();
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let mut i = 0;
    while i < chars.len() {
        let (byte_idx, c) = chars[i];
        if c == '@' {
            let prev = i.checked_sub(1).map(|j| chars[j].1);
            if is_at_mention_boundary(prev) {
                let start = byte_idx;
                i += 1;
                let path_start = i;
                while i < chars.len() && is_path_char(chars[i].1) {
                    i += 1;
                }
                if i > path_start {
                    let end_byte = if i < chars.len() {
                        chars[i].0
                    } else {
                        text.len()
                    };
                    let path: String = chars[path_start..i].iter().map(|(_, ch)| *ch).collect();
                    let path = path.replace('\\', "/");
                    if !path.is_empty() && !path.starts_with('@') {
                        out.push((start, end_byte, path));
                    }
                }
                continue;
            }
        }
        i += 1;
    }
    out
}

fn normalize_join(workspace: &Path, rel: &str) -> PathBuf {
    let rel = rel.trim_start_matches("./");
    let mut base = workspace.to_path_buf();
    for comp in Path::new(rel).components() {
        match comp {
            Component::Normal(s) => base.push(s),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return PathBuf::new();
            }
        }
    }
    base
}

/// Expand each `@rel/path` into a fenced code block with file contents (or an error note).
pub fn expand_at_file_mentions(
    text: &str,
    workspace: &Path,
    max_file_bytes: usize,
) -> anyhow::Result<String> {
    let mentions = parse_at_mentions(text);
    if mentions.is_empty() {
        return Ok(text.to_string());
    }

    let mut inserts: Vec<(usize, usize, String)> = Vec::new();
    for (start, end, rel) in mentions {
        let full = normalize_join(workspace, &rel);
        if full.as_os_str().is_empty() || !full.starts_with(workspace) {
            inserts.push((start, end, format!("[nca: skipped unsafe path @{rel}]")));
            continue;
        }
        match std::fs::read(&full) {
            Ok(bytes) => {
                let n = bytes.len().min(max_file_bytes);
                let slice = &bytes[..n];
                let content = String::from_utf8_lossy(slice);
                let note = if bytes.len() > max_file_bytes {
                    format!(
                        "\n… truncated ({} bytes, showing first {})\n",
                        bytes.len(),
                        n
                    )
                } else {
                    String::new()
                };
                let block = format!("\n\n```file:{rel}\n{}{}\n```\n\n", content, note);
                inserts.push((start, end, block));
            }
            Err(e) => {
                inserts.push((start, end, format!("[nca: could not read @{rel}: {e}]")));
            }
        }
    }

    inserts.sort_by_key(|(s, _, _)| *s);
    let mut offset: isize = 0;
    let mut result = text.to_string();
    for (start, end, replacement) in inserts {
        let s = ((start as isize) + offset).max(0) as usize;
        let e = ((end as isize) + offset).max(0) as usize;
        if s <= e && e <= result.len() {
            result.replace_range(s..e, &replacement);
            offset += replacement.len() as isize - (end - start) as isize;
        }
    }
    Ok(result)
}

/// Expand mentions with default size limit.
pub fn expand_at_file_mentions_default(text: &str, workspace: &Path) -> anyhow::Result<String> {
    expand_at_file_mentions(text, workspace, DEFAULT_MAX_FILE_BYTES)
}

/// Active `@`-token immediately before `cursor_byte` (UTF-8 byte index in `line`).
pub fn at_token_before_cursor(line: &str, cursor_byte: usize) -> Option<(usize, String)> {
    let before = line.get(..cursor_byte.min(line.len()))?;
    let at_rel = before.rfind('@')?;
    let prev = at_rel
        .checked_sub(1)
        .and_then(|i| before.get(i..))
        .and_then(|s| s.chars().next());
    if !is_at_mention_boundary(prev) {
        return None;
    }
    let after = &before[at_rel + 1..];
    if after.chars().any(|c| c.is_whitespace()) {
        return None;
    }
    let token = after.replace('\\', "/");
    Some((at_rel, token))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_two_mentions() {
        let s = "See @crates/foo.rs and @README.md ok";
        let m = parse_at_mentions(s);
        assert_eq!(m.len(), 2);
        assert_eq!(m[0], (4, 18, "crates/foo.rs".into()));
        assert_eq!(m[1], (23, 33, "README.md".into()));
    }

    #[test]
    fn at_token_detects_active_path() {
        let line = "hi @src/main.rs";
        let cur = line.len();
        let (i, tok) = at_token_before_cursor(line, cur).unwrap();
        assert_eq!(i, 3);
        assert_eq!(tok, "src/main.rs");
    }
}
