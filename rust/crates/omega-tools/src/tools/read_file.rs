//! `read_file` — read a file, optionally sliced by 1-indexed offset/limit.

use serde_json::Value;
use tokio_util::sync::CancellationToken;

const MAX_LINES: usize = 2_000;
const MAX_BYTES: usize = 50_000;

pub async fn execute(input: Value, _cancel: Option<&CancellationToken>) -> Result<String, String> {
    let path = input["path"]
        .as_str()
        .ok_or("read_file: path is required")?;

    let content = tokio::fs::read_to_string(path)
        .await
        .map_err(|e| format!("read_file: {e}"))?;

    let lines: Vec<&str> = content.split('\n').collect();
    let total = lines.len();

    // ---- offset / limit mode -----------------------------------------------
    let offset = input["offset"]
        .as_u64()
        .map(|n| usize::try_from(n).unwrap_or(usize::MAX));
    let limit = input["limit"]
        .as_u64()
        .map(|n| usize::try_from(n).unwrap_or(usize::MAX));

    if offset.is_some() || limit.is_some() {
        let start = offset.unwrap_or(1).saturating_sub(1);
        let end = limit.map_or(total, |l| (start + l).min(total)).max(start);
        let slice = lines.get(start..end).unwrap_or_default().join("\n");
        if end < total {
            return Ok(format!(
                "{slice}\n\n[{} more lines. Use offset={} to continue.]",
                total - end,
                end + 1
            ));
        }
        return Ok(slice);
    }

    // ---- full-file mode: lines first ----------------------------------------
    if lines.len() > MAX_LINES {
        return Ok(format!(
            "{}\n\n[Truncated. {} more lines. Use offset/limit to read more.]",
            lines[..MAX_LINES].join("\n"),
            lines.len() - MAX_LINES
        ));
    }

    // ---- then bytes ---------------------------------------------------------
    if content.len() > MAX_BYTES {
        let end = char_boundary_at_or_before(&content, MAX_BYTES);
        return Ok(format!(
            "{}\n\n[Truncated at {MAX_BYTES} bytes. Use offset/limit to read more.]",
            &content[..end]
        ));
    }

    Ok(content)
}

fn char_boundary_at_or_before(s: &str, max_bytes: usize) -> usize {
    let mut idx = max_bytes.min(s.len());
    while !s.is_char_boundary(idx) {
        idx -= 1;
    }
    idx
}

#[cfg(test)]
mod tests {
    use super::char_boundary_at_or_before;

    #[test]
    fn cbab_ascii_returns_exact_position() {
        let s = "hello world";
        // All bytes are ASCII → every position is a boundary
        assert_eq!(char_boundary_at_or_before(s, 5), 5);
        assert_eq!(char_boundary_at_or_before(s, 0), 0);
        assert_eq!(char_boundary_at_or_before(s, 11), 11);
    }

    #[test]
    fn cbab_multibyte_steps_back_past_continuation_byte() {
        // 'é' = U+00E9 = bytes [0xC3, 0xA9] at positions 2–3 in "abéc"
        // (bytes: 61 62 C3 A9 63, len=5)
        // max_bytes=3 lands on 0xA9 — a continuation byte, not a boundary.
        // Must back up to 2 (the 0xC3 start byte).
        let s = "ab\u{00E9}c";
        assert_eq!(char_boundary_at_or_before(s, 3), 2);
    }

    #[test]
    fn cbab_result_is_always_a_valid_char_boundary() {
        // '中' = U+4E2D = bytes [0xE4, 0xB8, 0xAD] at positions 2–4 in "ab中c"
        let s = "ab\u{4E2D}c";
        for max in 0..=s.len() + 1 {
            let idx = char_boundary_at_or_before(s, max);
            assert!(
                s.is_char_boundary(idx),
                "idx={idx} for max={max} must be a char boundary"
            );
            assert!(
                idx <= max.min(s.len()),
                "idx={idx} must not exceed min(max={max}, len)"
            );
        }
    }
}
