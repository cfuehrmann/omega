//! `write_file` — create or overwrite a file, creating parent directories.

use serde_json::Value;
use tokio_util::sync::CancellationToken;

pub async fn execute(
    input: Value,
    _cancel: Option<&CancellationToken>,
) -> Result<String, String> {
    let path = input["path"].as_str().ok_or("write_file: path is required")?;
    let content = input["content"]
        .as_str()
        .ok_or("write_file: content is required")?;

    // Create parent directories, but only when there is a non-empty parent.
    if let Some(parent) = std::path::Path::new(path)
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
    {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| format!("write_file: failed to create directories: {e}"))?;
    }

    tokio::fs::write(path, content)
        .await
        .map_err(|e| format!("write_file: {e}"))?;

    let lines = content.split('\n').count();
    Ok(format!(
        "Wrote {} bytes ({lines} lines) to {path}",
        content.len()
    ))
}
