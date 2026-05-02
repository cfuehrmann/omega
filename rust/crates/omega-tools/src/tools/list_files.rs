//! `list_files` — list directory entries, optionally recursive (DFS, dirs
//! first, alphabetical within each tier).

use std::fmt::Write as _;

use serde_json::Value;
use tokio_util::sync::CancellationToken;

const MAX_ENTRIES: usize = 1_000;

pub async fn execute(input: Value, _cancel: Option<&CancellationToken>) -> Result<String, String> {
    let path = input["path"]
        .as_str()
        .ok_or("list_files: path is required")?
        .to_owned();
    let recursive = input["recursive"].as_bool().unwrap_or(false);

    let output = tokio::task::spawn_blocking(move || {
        let mut results: Vec<String> = Vec::new();
        walk_sync(
            &path,
            std::path::Path::new(&path),
            true, // is_root
            recursive,
            &mut results,
        )?;
        let mut out = results.join("\n");
        if results.len() >= MAX_ENTRIES {
            // Write is infallible for String.
            let _ = write!(out, "\n\n[Truncated at {MAX_ENTRIES} entries]");
        }
        Ok::<String, String>(out)
    })
    .await
    .map_err(|e| format!("list_files: task failed: {e}"))??;

    Ok(output)
}

fn walk_sync(
    base: &str,
    dir: &std::path::Path,
    is_root: bool,
    recursive: bool,
    results: &mut Vec<String>,
) -> Result<(), String> {
    if results.len() >= MAX_ENTRIES {
        return Ok(());
    }

    let mut entries: Vec<std::fs::DirEntry> = std::fs::read_dir(dir)
        .map_err(|e| format!("list_files: cannot read directory {}: {e}", dir.display()))?
        .filter_map(Result::ok)
        .collect();

    entries.sort_by(|a, b| {
        let a_dir = a.file_type().is_ok_and(|ft| ft.is_dir());
        let b_dir = b.file_type().is_ok_and(|ft| ft.is_dir());
        match (a_dir, b_dir) {
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            _ => a.file_name().cmp(&b.file_name()),
        }
    });

    for entry in entries {
        if results.len() >= MAX_ENTRIES {
            break;
        }

        let name = entry.file_name();
        let name_str = name.to_string_lossy();

        if name_str == "node_modules" {
            continue;
        }
        // Hide dotfiles only at the top level of a non-recursive listing.
        // Recursive listings show everything under non-VCS directories so the
        // caller can see hidden files inside ordinary subdirectories.
        if name_str.starts_with('.') && is_root && !recursive {
            continue;
        }

        let Ok(ft) = entry.file_type() else {
            continue;
        };
        let is_dir = ft.is_dir();

        let full_path = entry.path();
        let rel = full_path
            .strip_prefix(base)
            .unwrap_or(&full_path)
            .to_string_lossy()
            .into_owned();

        if is_dir {
            results.push(format!("{rel}/"));
            if recursive && !name_str.starts_with(".git") && name_str != "node_modules" {
                walk_sync(base, &full_path, false, recursive, results)?;
            }
        } else {
            results.push(rel);
        }
    }

    Ok(())
}
