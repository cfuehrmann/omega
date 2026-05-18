# Omega Mutation Testing Report

**Run started:** 2026-05-18T06:21:44Z  
**Run ended:** 2026-05-18T06:41:28Z  
**Tool:** cargo-mutants 26.0.0  
**Flags:** `-j1 --no-shuffle` (serial, deterministic)  

> omega-e2e is excluded globally (browser tests require live Chromium).

## 1. Executive Summary

| Crate | Mutants | Caught | Missed | Timeout | Unviable | Kill rate |
|-------|---------|--------|--------|---------|----------|-----------|
| `omega-types` | 5 | 4 | 1 | 0 | 0 | 80% |
| `omega-mock-server` | 15 | 2 | 0 | 0 | 13 | 100% |
| `omega-cli` | 20 | 13 | 0 | 0 | 7 | 100% |
| `omega-test-fixtures` | 31 | 9 | 0 | 0 | 22 | 100% |
| `omega-store` | 65 | 39 | 0 | 1 | 25 | 98% |
| `omega-core` | 108 | 65 | 0 | 2 | 41 | 97% |
| `omega-server` | 110 | 36 | 3 | 0 | 71 | 92% |
| `omega-agent` | 175 | 60 | 7 | 0 | 108 | 90% |
| `omega-tools` | 275 | 136 | 16 | 4 | 119 | 87% |
| **Total** | **804** | **364** | **27** | **7** | **406** | **91%** |

## 2. Surviving Mutants

Surviving mutants are the most actionable finding: they represent code paths that could change behaviour without any test failing.

### 2.1 `omega-types` — 1 survivor(s)

#### `OmegaEvent::time` — crates/omega-types/src/events.rs:510

- **Mutant:** replace `OmegaEvent::time -> &ISOTimestamp` with `Box::leak(Box::new(Default::default()))`
- **Genre:** FnValue
- **Location:** `crates/omega-types/src/events.rs:510:9`

```rust
   507 │     /// variant without a `time` field will fail here.
   508 │     #[must_use]
   509 │     pub fn time(&self) -> &ISOTimestamp {
→  510 │         match self {
   511 │             Self::SessionStarted(e) => &e.time,
   512 │             Self::ServerStarted(e) => &e.time,
   513 │             Self::ServerStopped(e) => &e.time,
```

**Analysis:** _No test currently asserts the value returned / side-effect_
**produced by this function with inputs that would distinguish the_
_replacement from the original. A targeted test is needed._

### 2.7 `omega-server` — 3 survivor(s)

#### `handle_reset` — crates/omega-server/src/router.rs:862

- **Mutant:** replace `handle_reset -> Result<(), String>` with ``
- **Genre:** UnaryOperator
- **Location:** `crates/omega-server/src/router.rs:862:8`

```rust
   859 │     // session and ask the client for confirmation.  The previous active
   860 │     // session (if any) is left untouched so "Cancel" in the UI is a true
   861 │     // no-op.
→  862 │     if !allow_dirty {
   863 │         let cwd = std::env::current_dir().unwrap_or_default();
   864 │         if git_has_pending_changes(&cwd) {
   865 │             let _ = tx.send(WsMessage::PendingChangesWarning {
```

**Analysis:** _No test currently asserts the value returned / side-effect_
**produced by this function with inputs that would distinguish the_
_replacement from the original. A targeted test is needed._

#### `handle_resume_session` — crates/omega-server/src/router.rs:924

- **Mutant:** replace `handle_resume_session -> Result<(), String>` with ``
- **Genre:** UnaryOperator
- **Location:** `crates/omega-server/src/router.rs:924:8`

```rust
   921 │     }
   922 │ 
   923 │     // Pre-flight dirty-tree gate — see `handle_reset` for the rationale.
→  924 │     if !allow_dirty {
   925 │         let cwd = std::env::current_dir().unwrap_or_default();
   926 │         if git_has_pending_changes(&cwd) {
   927 │             let _ = tx.send(WsMessage::PendingChangesWarning {
```

**Analysis:** _No test currently asserts the value returned / side-effect_
**produced by this function with inputs that would distinguish the_
_replacement from the original. A targeted test is needed._

#### `PendingChangesIntent::to_json` — crates/omega-server/src/ws_message.rs:110

- **Mutant:** replace `PendingChangesIntent::to_json -> serde_json::Value` with `Default::default()`
- **Genre:** FnValue
- **Location:** `crates/omega-server/src/ws_message.rs:110:9`

```rust
   107 │ 
   108 │ impl PendingChangesIntent {
   109 │     fn to_json(&self) -> serde_json::Value {
→  110 │         match self {
   111 │             Self::Reset { model, effort } => {
   112 │                 let mut obj = serde_json::Map::new();
   113 │                 obj.insert("kind".to_owned(), serde_json::Value::from("reset"));
```

**Analysis:** _No test currently asserts the value returned / side-effect_
**produced by this function with inputs that would distinguish the_
_replacement from the original. A targeted test is needed._

### 2.8 `omega-agent` — 7 survivor(s)

#### `gen_call_id` — crates/omega-agent/src/agent.rs:2089

- **Mutant:** replace `gen_call_id -> String` with `"xyzzy".into()`
- **Genre:** FnValue
- **Location:** `crates/omega-agent/src/agent.rs:2089:5`

```rust
  2086 │ /// embedded in tee-log filenames so that the two are bidirectionally
  2087 │ /// cross-referenceable without knowing the LLM provider's ID format.
  2088 │ fn gen_call_id() -> String {
→ 2089 │     let bytes: [u8; 4] = rand::random();
  2090 │     bytes.iter().fold(String::with_capacity(8), |mut s, b| {
  2091 │         use std::fmt::Write as _;
  2092 │         let _ = write!(s, "{b:02x}");
```

**Analysis:** _No test currently asserts the value returned / side-effect_
**produced by this function with inputs that would distinguish the_
_replacement from the original. A targeted test is needed._

#### `project_turn` — crates/omega-agent/src/session_resume.rs:226

- **Mutant:** replace `project_turn -> String` with `true`
- **Genre:** MatchArmGuard
- **Location:** `crates/omega-agent/src/session_resume.rs:226:48`

```rust
   223 │             OmegaEvent::TextBlock(e) => {
   224 │                 pending_text.push(e.text.clone());
   225 │             }
→  226 │             OmegaEvent::LlmResponseEnded(_) if !pending_text.is_empty() => {
   227 │                 let joined = pending_text.join("");
   228 │                 let text = joined.trim();
   229 │                 if !text.is_empty() {
```

**Analysis:** _No test currently asserts the value returned / side-effect_
**produced by this function with inputs that would distinguish the_
_replacement from the original. A targeted test is needed._

#### `project_turn` — crates/omega-agent/src/session_resume.rs:226

- **Mutant:** replace `project_turn -> String` with `false`
- **Genre:** MatchArmGuard
- **Location:** `crates/omega-agent/src/session_resume.rs:226:48`

```rust
   223 │             OmegaEvent::TextBlock(e) => {
   224 │                 pending_text.push(e.text.clone());
   225 │             }
→  226 │             OmegaEvent::LlmResponseEnded(_) if !pending_text.is_empty() => {
   227 │                 let joined = pending_text.join("");
   228 │                 let text = joined.trim();
   229 │                 if !text.is_empty() {
```

**Analysis:** _No test currently asserts the value returned / side-effect_
**produced by this function with inputs that would distinguish the_
_replacement from the original. A targeted test is needed._

#### `project_turn` — crates/omega-agent/src/session_resume.rs:226

- **Mutant:** replace `project_turn -> String` with ``
- **Genre:** UnaryOperator
- **Location:** `crates/omega-agent/src/session_resume.rs:226:48`

```rust
   223 │             OmegaEvent::TextBlock(e) => {
   224 │                 pending_text.push(e.text.clone());
   225 │             }
→  226 │             OmegaEvent::LlmResponseEnded(_) if !pending_text.is_empty() => {
   227 │                 let joined = pending_text.join("");
   228 │                 let text = joined.trim();
   229 │                 if !text.is_empty() {
```

**Analysis:** _No test currently asserts the value returned / side-effect_
**produced by this function with inputs that would distinguish the_
_replacement from the original. A targeted test is needed._

#### `project_turn` — crates/omega-agent/src/session_resume.rs:267

- **Mutant:** replace `project_turn -> String` with ``
- **Genre:** UnaryOperator
- **Location:** `crates/omega-agent/src/session_resume.rs:267:8`

```rust
   264 │     }
   265 │ 
   266 │     // Flush any text not followed by LlmResponseEnded (e.g. interrupted turns).
→  267 │     if !pending_text.is_empty() {
   268 │         let joined = pending_text.join("");
   269 │         let text = joined.trim();
   270 │         if !text.is_empty() {
```

**Analysis:** _No test currently asserts the value returned / side-effect_
**produced by this function with inputs that would distinguish the_
_replacement from the original. A targeted test is needed._

#### `global_agents_md_path` — crates/omega-agent/src/system_prompt.rs:133

- **Mutant:** replace `global_agents_md_path -> Option<PathBuf>` with `None`
- **Genre:** FnValue
- **Location:** `crates/omega-agent/src/system_prompt.rs:133:5`

```rust
   130 │ /// (very unusual — e.g. an unsandboxed CI worker with no `HOME`).
   131 │ #[must_use]
   132 │ pub fn global_agents_md_path() -> Option<PathBuf> {
→  133 │     global_agents_md_path_from_env(
   134 │         std::env::var_os("XDG_CONFIG_HOME").as_deref(),
   135 │         std::env::var_os("HOME").as_deref(),
   136 │     )
```

**Analysis:** _No test currently asserts the value returned / side-effect_
**produced by this function with inputs that would distinguish the_
_replacement from the original. A targeted test is needed._

#### `global_agents_md_path` — crates/omega-agent/src/system_prompt.rs:133

- **Mutant:** replace `global_agents_md_path -> Option<PathBuf>` with `Some(Default::default())`
- **Genre:** FnValue
- **Location:** `crates/omega-agent/src/system_prompt.rs:133:5`

```rust
   130 │ /// (very unusual — e.g. an unsandboxed CI worker with no `HOME`).
   131 │ #[must_use]
   132 │ pub fn global_agents_md_path() -> Option<PathBuf> {
→  133 │     global_agents_md_path_from_env(
   134 │         std::env::var_os("XDG_CONFIG_HOME").as_deref(),
   135 │         std::env::var_os("HOME").as_deref(),
   136 │     )
```

**Analysis:** _No test currently asserts the value returned / side-effect_
**produced by this function with inputs that would distinguish the_
_replacement from the original. A targeted test is needed._

### 2.9 `omega-tools` — 16 survivor(s)

#### `cap_and_tee` — crates/omega-tools/src/cap_and_tee.rs:127

- **Mutant:** replace `cap_and_tee -> io::Result<CappedOutput>` with `*`
- **Genre:** BinaryOperator
- **Location:** `crates/omega-tools/src/cap_and_tee.rs:127:32`

```rust
   124 │                 )
   125 │             }
   126 │             TruncationBias::Middle => {
→  127 │                 let half = cap / 2;
   128 │                 let head_end = utf8_boundary_forward(data, half);
   129 │                 let tail_len = utf8_boundary_backward(data, half);
   130 │                 let tail_start = total_bytes - tail_len;
```

**Analysis:** _No test currently asserts the value returned / side-effect_
**produced by this function with inputs that would distinguish the_
_replacement from the original. A targeted test is needed._

#### `cap_and_tee` — crates/omega-tools/src/cap_and_tee.rs:130

- **Mutant:** replace `cap_and_tee -> io::Result<CappedOutput>` with `/`
- **Genre:** BinaryOperator
- **Location:** `crates/omega-tools/src/cap_and_tee.rs:130:46`

```rust
   127 │                 let half = cap / 2;
   128 │                 let head_end = utf8_boundary_forward(data, half);
   129 │                 let tail_len = utf8_boundary_backward(data, half);
→  130 │                 let tail_start = total_bytes - tail_len;
   131 │                 let omitted = tail_start.saturating_sub(head_end);
   132 │                 let head = String::from_utf8_lossy(&data[..head_end]);
   133 │                 let tail = String::from_utf8_lossy(&data[tail_start..]);
```

**Analysis:** _No test currently asserts the value returned / side-effect_
**produced by this function with inputs that would distinguish the_
_replacement from the original. A targeted test is needed._

#### `utf8_boundary_forward` — crates/omega-tools/src/cap_and_tee.rs:183

- **Mutant:** replace `utf8_boundary_forward -> usize` with `==`
- **Genre:** BinaryOperator
- **Location:** `crates/omega-tools/src/cap_and_tee.rs:183:15`

```rust
   180 │ fn utf8_boundary_forward(data: &[u8], max: usize) -> usize {
   181 │     let mut end = max.min(data.len());
   182 │     // Back up past UTF-8 continuation bytes (0x80..=0xBF).
→  183 │     while end > 0 && is_utf8_continuation(data[end - 1]) {
   184 │         end -= 1;
   185 │     }
   186 │     end
```

**Analysis:** _No test currently asserts the value returned / side-effect_
**produced by this function with inputs that would distinguish the_
_replacement from the original. A targeted test is needed._

#### `utf8_boundary_forward` — crates/omega-tools/src/cap_and_tee.rs:183

- **Mutant:** replace `utf8_boundary_forward -> usize` with `/`
- **Genre:** BinaryOperator
- **Location:** `crates/omega-tools/src/cap_and_tee.rs:183:52`

```rust
   180 │ fn utf8_boundary_forward(data: &[u8], max: usize) -> usize {
   181 │     let mut end = max.min(data.len());
   182 │     // Back up past UTF-8 continuation bytes (0x80..=0xBF).
→  183 │     while end > 0 && is_utf8_continuation(data[end - 1]) {
   184 │         end -= 1;
   185 │     }
   186 │     end
```

**Analysis:** _No test currently asserts the value returned / side-effect_
**produced by this function with inputs that would distinguish the_
_replacement from the original. A targeted test is needed._

#### `utf8_boundary_forward` — crates/omega-tools/src/cap_and_tee.rs:184

- **Mutant:** replace `utf8_boundary_forward -> usize` with `+=`
- **Genre:** BinaryOperator
- **Location:** `crates/omega-tools/src/cap_and_tee.rs:184:13`

```rust
   181 │     let mut end = max.min(data.len());
   182 │     // Back up past UTF-8 continuation bytes (0x80..=0xBF).
   183 │     while end > 0 && is_utf8_continuation(data[end - 1]) {
→  184 │         end -= 1;
   185 │     }
   186 │     end
   187 │ }
```

**Analysis:** _No test currently asserts the value returned / side-effect_
**produced by this function with inputs that would distinguish the_
_replacement from the original. A targeted test is needed._

#### `utf8_boundary_backward` — crates/omega-tools/src/cap_and_tee.rs:197

- **Mutant:** replace `utf8_boundary_backward -> usize` with `==`
- **Genre:** BinaryOperator
- **Location:** `crates/omega-tools/src/cap_and_tee.rs:197:17`

```rust
   194 │     let raw_start = len.saturating_sub(max);
   195 │     // Advance past continuation bytes to find the next valid start.
   196 │     let mut start = raw_start;
→  197 │     while start < len && is_utf8_continuation(data[start]) {
   198 │         start += 1;
   199 │     }
   200 │     len - start
```

**Analysis:** _No test currently asserts the value returned / side-effect_
**produced by this function with inputs that would distinguish the_
_replacement from the original. A targeted test is needed._

#### `utf8_boundary_backward` — crates/omega-tools/src/cap_and_tee.rs:197

- **Mutant:** replace `utf8_boundary_backward -> usize` with `>`
- **Genre:** BinaryOperator
- **Location:** `crates/omega-tools/src/cap_and_tee.rs:197:17`

```rust
   194 │     let raw_start = len.saturating_sub(max);
   195 │     // Advance past continuation bytes to find the next valid start.
   196 │     let mut start = raw_start;
→  197 │     while start < len && is_utf8_continuation(data[start]) {
   198 │         start += 1;
   199 │     }
   200 │     len - start
```

**Analysis:** _No test currently asserts the value returned / side-effect_
**produced by this function with inputs that would distinguish the_
_replacement from the original. A targeted test is needed._

#### `utf8_boundary_backward` — crates/omega-tools/src/cap_and_tee.rs:197

- **Mutant:** replace `utf8_boundary_backward -> usize` with `<=`
- **Genre:** BinaryOperator
- **Location:** `crates/omega-tools/src/cap_and_tee.rs:197:17`

```rust
   194 │     let raw_start = len.saturating_sub(max);
   195 │     // Advance past continuation bytes to find the next valid start.
   196 │     let mut start = raw_start;
→  197 │     while start < len && is_utf8_continuation(data[start]) {
   198 │         start += 1;
   199 │     }
   200 │     len - start
```

**Analysis:** _No test currently asserts the value returned / side-effect_
**produced by this function with inputs that would distinguish the_
_replacement from the original. A targeted test is needed._

#### `utf8_boundary_backward` — crates/omega-tools/src/cap_and_tee.rs:198

- **Mutant:** replace `utf8_boundary_backward -> usize` with `-=`
- **Genre:** BinaryOperator
- **Location:** `crates/omega-tools/src/cap_and_tee.rs:198:15`

```rust
   195 │     // Advance past continuation bytes to find the next valid start.
   196 │     let mut start = raw_start;
   197 │     while start < len && is_utf8_continuation(data[start]) {
→  198 │         start += 1;
   199 │     }
   200 │     len - start
   201 │ }
```

**Analysis:** _No test currently asserts the value returned / side-effect_
**produced by this function with inputs that would distinguish the_
_replacement from the original. A targeted test is needed._

#### `utf8_boundary_backward` — crates/omega-tools/src/cap_and_tee.rs:198

- **Mutant:** replace `utf8_boundary_backward -> usize` with `*=`
- **Genre:** BinaryOperator
- **Location:** `crates/omega-tools/src/cap_and_tee.rs:198:15`

```rust
   195 │     // Advance past continuation bytes to find the next valid start.
   196 │     let mut start = raw_start;
   197 │     while start < len && is_utf8_continuation(data[start]) {
→  198 │         start += 1;
   199 │     }
   200 │     len - start
   201 │ }
```

**Analysis:** _No test currently asserts the value returned / side-effect_
**produced by this function with inputs that would distinguish the_
_replacement from the original. A targeted test is needed._

#### `is_utf8_continuation` — crates/omega-tools/src/cap_and_tee.rs:206

- **Mutant:** replace `is_utf8_continuation -> bool` with `|`
- **Genre:** BinaryOperator
- **Location:** `crates/omega-tools/src/cap_and_tee.rs:206:8`

```rust
   203 │ /// True for UTF-8 continuation bytes (0x80..=0xBF).
   204 │ #[inline]
   205 │ fn is_utf8_continuation(b: u8) -> bool {
→  206 │     (b & 0xC0) == 0x80
   207 │ }
   208 │ 
   209 │ // ---------------------------------------------------------------------------
```

**Analysis:** _No test currently asserts the value returned / side-effect_
**produced by this function with inputs that would distinguish the_
_replacement from the original. A targeted test is needed._

#### `is_utf8_continuation` — crates/omega-tools/src/cap_and_tee.rs:206

- **Mutant:** replace `is_utf8_continuation -> bool` with `^`
- **Genre:** BinaryOperator
- **Location:** `crates/omega-tools/src/cap_and_tee.rs:206:8`

```rust
   203 │ /// True for UTF-8 continuation bytes (0x80..=0xBF).
   204 │ #[inline]
   205 │ fn is_utf8_continuation(b: u8) -> bool {
→  206 │     (b & 0xC0) == 0x80
   207 │ }
   208 │ 
   209 │ // ---------------------------------------------------------------------------
```

**Analysis:** _No test currently asserts the value returned / side-effect_
**produced by this function with inputs that would distinguish the_
_replacement from the original. A targeted test is needed._

#### `crlf_normalize` — crates/omega-tools/src/output_cleaner.rs:70

- **Mutant:** replace `crlf_normalize -> Vec<u8>` with `<=`
- **Genre:** BinaryOperator
- **Location:** `crates/omega-tools/src/output_cleaner.rs:70:38`

```rust
    67 │     let mut result = Vec::with_capacity(data.len());
    68 │     let mut i = 0;
    69 │     while i < data.len() {
→   70 │         if data[i] == b'\r' && i + 1 < data.len() && data[i + 1] == b'\n' {
    71 │             result.push(b'\n');
    72 │             i += 2;
    73 │         } else {
```

**Analysis:** _No test currently asserts the value returned / side-effect_
**produced by this function with inputs that would distinguish the_
_replacement from the original. A targeted test is needed._

#### `crlf_normalize` — crates/omega-tools/src/output_cleaner.rs:70

- **Mutant:** replace `crlf_normalize -> Vec<u8>` with `*`
- **Genre:** BinaryOperator
- **Location:** `crates/omega-tools/src/output_cleaner.rs:70:34`

```rust
    67 │     let mut result = Vec::with_capacity(data.len());
    68 │     let mut i = 0;
    69 │     while i < data.len() {
→   70 │         if data[i] == b'\r' && i + 1 < data.len() && data[i + 1] == b'\n' {
    71 │             result.push(b'\n');
    72 │             i += 2;
    73 │         } else {
```

**Analysis:** _No test currently asserts the value returned / side-effect_
**produced by this function with inputs that would distinguish the_
_replacement from the original. A targeted test is needed._

#### `execute` — crates/omega-tools/src/tools/run_command.rs:187

- **Mutant:** replace `execute -> Result<String, String>` with `true`
- **Genre:** MatchArmGuard
- **Location:** `crates/omega-tools/src/tools/run_command.rs:187:39`

```rust
   184 │     // non-zero exit / timeout / abort → Tail (errors at end),
   185 │     // success → Head (interesting output starts at top).
   186 │     let bias = bias_override.unwrap_or_else(|| match &outcome {
→  187 │         Outcome::Finished(Some(s)) if s.success() => TruncationBias::Head,
   188 │         _ => TruncationBias::Tail,
   189 │     });
   190 │ 
```

**Analysis:** _No test currently asserts the value returned / side-effect_
**produced by this function with inputs that would distinguish the_
_replacement from the original. A targeted test is needed._

#### `execute` — crates/omega-tools/src/tools/run_command.rs:187

- **Mutant:** replace `execute -> Result<String, String>` with `false`
- **Genre:** MatchArmGuard
- **Location:** `crates/omega-tools/src/tools/run_command.rs:187:39`

```rust
   184 │     // non-zero exit / timeout / abort → Tail (errors at end),
   185 │     // success → Head (interesting output starts at top).
   186 │     let bias = bias_override.unwrap_or_else(|| match &outcome {
→  187 │         Outcome::Finished(Some(s)) if s.success() => TruncationBias::Head,
   188 │         _ => TruncationBias::Tail,
   189 │     });
   190 │ 
```

**Analysis:** _No test currently asserts the value returned / side-effect_
**produced by this function with inputs that would distinguish the_
_replacement from the original. A targeted test is needed._

## 3. Timeout Mutants

### `omega-store` — 1 timeout(s)

- `crates/omega-store/src/session_dir.rs:216:19`: `strip_jsonc_comments -> String` → `*=`

### `omega-core` — 2 timeout(s)

- `crates/omega-core/src/retry.rs:134:46`: `retry_loop -> impl Stream<Item = Result<AgentItem, LlmError>>+Send` → `*`
- `crates/omega-core/src/retry.rs:135:40`: `retry_loop -> impl Stream<Item = Result<AgentItem, LlmError>>+Send` → `&&`

### `omega-tools` — 4 timeout(s)

- `crates/omega-tools/src/output_cleaner.rs:72:15`: `crlf_normalize -> Vec<u8>` → `-=`
- `crates/omega-tools/src/output_cleaner.rs:75:15`: `crlf_normalize -> Vec<u8>` → `*=`
- `crates/omega-tools/src/tools/edit_file.rs:113:15`: `count_occurrences -> usize` → `*=`
- `crates/omega-tools/src/tools/read_file.rs:67:13`: `char_boundary_at_or_before -> usize` → `/=`

## 4. Unviable Mutants

Unviable mutants failed to compile. This is normal for type-system-constrained replacements. A high count can indicate missing feature flags or over-aggressive cargo-mutants genre coverage.

### `omega-mock-server` — 13 unviable

- `crates/omega-mock-server/src/main.rs:55:5`: `main -> std::io::Result<()>` → `Ok(())`
- `crates/omega-mock-server/src/control.rs:31:5`: `router -> Router` → `Default::default()`
- `crates/omega-mock-server/src/control.rs:44:5`: `llm_calls -> Json<Vec<CapturedCall>>` → `Json::new()`
- `crates/omega-mock-server/src/control.rs:44:5`: `llm_calls -> Json<Vec<CapturedCall>>` → `Json::from_iter([vec![]])`
- `crates/omega-mock-server/src/control.rs:44:5`: `llm_calls -> Json<Vec<CapturedCall>>` → `Json::new(vec![])`
- `crates/omega-mock-server/src/control.rs:44:5`: `llm_calls -> Json<Vec<CapturedCall>>` → `Json::from(vec![])`
- `crates/omega-mock-server/src/control.rs:44:5`: `llm_calls -> Json<Vec<CapturedCall>>` → `Json::from_iter([vec![Default::default()]])`
- `crates/omega-mock-server/src/control.rs:44:5`: `llm_calls -> Json<Vec<CapturedCall>>` → `Json::new(vec![Default::default()])`
- `crates/omega-mock-server/src/control.rs:44:5`: `llm_calls -> Json<Vec<CapturedCall>>` → `Json::from(vec![Default::default()])`
- `crates/omega-mock-server/src/control.rs:48:5`: `reset_calls -> &'static str` → `""`
- `crates/omega-mock-server/src/control.rs:48:5`: `reset_calls -> &'static str` → `"xyzzy"`
- `crates/omega-mock-server/src/control.rs:56:5`: `set_script -> &'static str` → `""`
- `crates/omega-mock-server/src/control.rs:56:5`: `set_script -> &'static str` → `"xyzzy"`

### `omega-cli` — 7 unviable

- `crates/omega-cli/src/main.rs:70:5`: `main ` → `()`
- `crates/omega-cli/src/main.rs:109:5`: `run -> i32` → `0`
- `crates/omega-cli/src/main.rs:109:5`: `run -> i32` → `1`
- `crates/omega-cli/src/main.rs:109:5`: `run -> i32` → `-1`
- `crates/omega-cli/src/main.rs:184:13`: `run -> i32` → ``
- `crates/omega-cli/src/main.rs:295:5`: `git_has_pending_changes -> bool` → `true`
- `crates/omega-cli/src/main.rs:295:5`: `git_has_pending_changes -> bool` → `false`

### `omega-test-fixtures` — 22 unviable

- `crates/omega-test-fixtures/src/lib.rs:139:5`: `script_from -> Script` → `Default::default()`
- `crates/omega-test-fixtures/src/lib.rs:180:9`: `CallHistory::push ` → `()`
- `crates/omega-test-fixtures/src/lib.rs:187:9`: `CallHistory::snapshot -> Vec<CapturedCall>` → `vec![Default::default()]`
- `crates/omega-test-fixtures/src/lib.rs:213:5`: `router -> Router` → `Default::default()`
- `crates/omega-test-fixtures/src/lib.rs:223:5`: `handle_messages -> Response` → `Default::default()`
- `crates/omega-test-fixtures/src/lib.rs:299:5`: `project_call -> CapturedCall` → `Default::default()`
- `crates/omega-test-fixtures/src/lib.rs:337:5`: `project_message -> CapturedMessage` → `Default::default()`
- `crates/omega-test-fixtures/src/lib.rs:342:17`: `project_message -> CapturedMessage` → `||`
- `crates/omega-test-fixtures/src/lib.rs:341:17`: `project_message -> CapturedMessage` → `||`
- `crates/omega-test-fixtures/src/lib.rs:380:9`: `<impl Drop for MockServer>::drop ` → `()`
- `crates/omega-test-fixtures/src/lib.rs:387:9`: `MockServer::start -> Self` → `Default::default()`
- `crates/omega-test-fixtures/src/lib.rs:393:9`: `MockServer::start_with_capture -> Self` → `Default::default()`
- `crates/omega-test-fixtures/src/lib.rs:397:9`: `MockServer::start_inner -> Self` → `Default::default()`
- `crates/omega-test-fixtures/src/lib.rs:419:5`: `sse_static_response -> Response` → `Default::default()`
- `crates/omega-test-fixtures/src/lib.rs:428:5`: `sse_slow_text_response -> Response` → `Default::default()`
- `crates/omega-test-fixtures/src/lib.rs:475:5`: `build_text_sse -> String` → `String::new()`
- `crates/omega-test-fixtures/src/lib.rs:475:5`: `build_text_sse -> String` → `"xyzzy".into()`
- `crates/omega-test-fixtures/src/lib.rs:509:5`: `build_tool_use_sse -> String` → `String::new()`
- `crates/omega-test-fixtures/src/lib.rs:509:5`: `build_tool_use_sse -> String` → `"xyzzy".into()`
- `crates/omega-test-fixtures/src/lib.rs:547:5`: `push_event ` → `()`
- `crates/omega-test-fixtures/src/lib.rs:556:5`: `format_event -> String` → `String::new()`
- `crates/omega-test-fixtures/src/lib.rs:556:5`: `format_event -> String` → `"xyzzy".into()`

### `omega-store` — 25 unviable

- `crates/omega-store/src/context_hash.rs:45:9`: `ContextHash::is_valid -> bool` → `true`
- `crates/omega-store/src/context_hash.rs:45:9`: `ContextHash::is_valid -> bool` → `false`
- `crates/omega-store/src/context_hash.rs:52:9`: `ContextHash::from_validated -> Self` → `Default::default()`
- `crates/omega-store/src/context_hash.rs:72:5`: `content_hash -> ContextHash` → `Default::default()`
- `crates/omega-store/src/context_hash.rs:87:5`: `hash_from_str -> Result<ContextHash>` → `Ok(Default::default())`
- `crates/omega-store/src/context_hash.rs:96:9`: `<impl fmt::Display for ContextHash>::fmt -> fmt::Result` → `Ok(Default::default())`
- `crates/omega-store/src/context_hash.rs:108:9`: `<impl From<ContextHash> for String>::from -> Self` → `Default::default()`
- `crates/omega-store/src/context_store.rs:80:9`: `ContextStore::append -> Result<ContextHash>` → `Ok(Default::default())`
- `crates/omega-store/src/context_store.rs:121:9`: `ContextStore::read_all -> Result<Vec<ContextRecord>>` → `Ok(vec![Default::default()])`
- `crates/omega-store/src/context_store.rs:123:23`: `ContextStore::read_all -> Result<Vec<ContextRecord>>` → `true`
- `crates/omega-store/src/context_store.rs:123:23`: `ContextStore::read_all -> Result<Vec<ContextRecord>>` → `false`
- `crates/omega-store/src/context_store.rs:141:9`: `ContextStore::build_record -> ContextRecord` → `Default::default()`
- `crates/omega-store/src/context_store.rs:168:9`: `ContextStore::verify_record -> Result<()>` → `Ok(())`
- `crates/omega-store/src/event_store.rs:48:23`: `EventStore::read_all -> Result<Vec<serde_json::Value>>` → `true`
- `crates/omega-store/src/event_store.rs:48:23`: `EventStore::read_all -> Result<Vec<serde_json::Value>>` → `false`
- `crates/omega-store/src/event_store.rs:68:9`: `EventStore::append -> Result<()>` → `Ok(())`
- `crates/omega-store/src/session_dir.rs:57:5`: `session_dir_re -> &'static Regex` → `Box::leak(Box::new(Default::default()))`
- `crates/omega-store/src/session_dir.rs:70:5`: `make_session_dir_name -> String` → `String::new()`
- `crates/omega-store/src/session_dir.rs:70:5`: `make_session_dir_name -> String` → `"xyzzy".into()`
- `crates/omega-store/src/session_dir.rs:104:5`: `make_session_dir -> Result<SessionPaths>` → `Ok(Default::default())`
- `crates/omega-store/src/session_dir.rs:153:5`: `read_session_metadata -> SessionMetadata` → `Default::default()`
- `crates/omega-store/src/session_dir.rs:169:5`: `write_session_metadata -> Result<()>` → `Ok(())`
- `crates/omega-store/src/session_dir.rs:183:5`: `update_session_metadata -> Result<()>` → `Ok(())`
- `crates/omega-store/src/session_dir.rs:203:5`: `strip_jsonc_comments -> String` → `String::new()`
- `crates/omega-store/src/session_dir.rs:203:5`: `strip_jsonc_comments -> String` → `"xyzzy".into()`

### `omega-core` — 41 unviable

- `crates/omega-core/src/anthropic.rs:53:9`: `AnthropicProvider::with_base_url -> Self` → `Default::default()`
- `crates/omega-core/src/anthropic.rs:61:9`: `AnthropicProvider::with_beta -> Self` → `Default::default()`
- `crates/omega-core/src/anthropic.rs:68:9`: `AnthropicProvider::with_client -> Self` → `Default::default()`
- `crates/omega-core/src/anthropic.rs:75:9`: `<impl Provider for AnthropicProvider>::stream -> AgentItemStream` → `Default::default()`
- `crates/omega-core/src/anthropic.rs:358:5`: `parse_data -> Result<T, LlmError>` → `Ok(Default::default())`
- `crates/omega-core/src/anthropic.rs:364:5`: `parse_retry_after -> Option<Duration>` → `None`
- `crates/omega-core/src/anthropic.rs:364:5`: `parse_retry_after -> Option<Duration>` → `Some(Default::default())`
- `crates/omega-core/src/anthropic.rs:392:5`: `extract_iterations -> Option<Vec<UsageIteration>>` → `None`
- `crates/omega-core/src/anthropic.rs:392:5`: `extract_iterations -> Option<Vec<UsageIteration>>` → `Some(vec![])`
- `crates/omega-core/src/anthropic.rs:392:5`: `extract_iterations -> Option<Vec<UsageIteration>>` → `Some(vec![Default::default()])`
- `crates/omega-core/src/anthropic.rs:417:9`: `BlockAccum::from_start -> Option<Self>` → `None`
- `crates/omega-core/src/anthropic.rs:417:9`: `BlockAccum::from_start -> Option<Self>` → `Some(Default::default())`
- `crates/omega-core/src/anthropic.rs:501:5`: `to_wire_block -> WireBlock<'_>` → `Default::default()`
- `crates/omega-core/src/anthropic.rs:567:5`: `build_system_blocks -> Option<Vec<SystemBlock<'_>>>` → `None`
- `crates/omega-core/src/anthropic.rs:567:5`: `build_system_blocks -> Option<Vec<SystemBlock<'_>>>` → `Some(vec![])`
- `crates/omega-core/src/anthropic.rs:567:5`: `build_system_blocks -> Option<Vec<SystemBlock<'_>>>` → `Some(vec![Default::default()])`
- `crates/omega-core/src/anthropic.rs:598:5`: `build_wire_messages -> Vec<WireMessage<'_>>` → `vec![]`
- `crates/omega-core/src/anthropic.rs:598:5`: `build_wire_messages -> Vec<WireMessage<'_>>` → `vec![Default::default()]`
- `crates/omega-core/src/anthropic.rs:632:5`: `build_wire_tools -> Vec<WireTool<'_>>` → `vec![]`
- `crates/omega-core/src/anthropic.rs:632:5`: `build_wire_tools -> Vec<WireTool<'_>>` → `vec![Default::default()]`
- `crates/omega-core/src/anthropic.rs:697:5`: `build_request_body -> AnthropicRequestBody<'_>` → `Default::default()`
- `crates/omega-core/src/ollama.rs:44:9`: `OllamaProvider::with_base_url -> Self` → `Default::default()`
- `crates/omega-core/src/ollama.rs:51:9`: `OllamaProvider::with_client -> Self` → `Default::default()`
- `crates/omega-core/src/ollama.rs:64:9`: `<impl Provider for OllamaProvider>::stream -> AgentItemStream` → `Default::default()`
- `crates/omega-core/src/ollama.rs:210:5`: `parse_retry_after -> Option<Duration>` → `None`
- `crates/omega-core/src/ollama.rs:210:5`: `parse_retry_after -> Option<Duration>` → `Some(Default::default())`
- `crates/omega-core/src/ollama.rs:265:5`: `build_request_body -> OllamaRequestBody<'_>` → `Default::default()`
- `crates/omega-core/src/ollama.rs:267:9`: `build_request_body -> OllamaRequestBody<'_>` → `||`
- `crates/omega-core/src/ollama.rs:305:5`: `flatten_message ` → `()`
- `crates/omega-core/src/ollama.rs:356:5`: `tool_to_ollama -> Value` → `Default::default()`
- `crates/omega-core/src/provider.rs:37:9`: `<impl Provider for std::sync::Arc<P>>::stream -> AgentItemStream` → `Default::default()`
- `crates/omega-core/src/provider.rs:43:9`: `<impl Provider for Box<P>>::stream -> AgentItemStream` → `Default::default()`
- `crates/omega-core/src/retry.rs:84:9`: `<impl Provider for RetryingProvider<P>>::stream -> AgentItemStream` → `Default::default()`
- `crates/omega-core/src/retry.rs:176:5`: `compute_backoff -> (Duration, Option<LlmRetryReason>)` → `(Default::default(), None)`
- `crates/omega-core/src/retry.rs:176:5`: `compute_backoff -> (Duration, Option<LlmRetryReason>)` → `(Default::default(), Some(Default::default()))`
- `crates/omega-core/src/retry.rs:199:5`: `build_retry_event -> OmegaEvent` → `Default::default()`
- `crates/omega-core/src/retry.rs:201:13`: `build_retry_event -> OmegaEvent` → `*`
- `crates/omega-core/src/types.rs:155:9`: `AgentItem::event -> Self` → `Default::default()`
- `crates/omega-core/src/types.rs:161:9`: `AgentItem::as_event -> Option<&OmegaEvent>` → `Some(Box::leak(Box::new(Default::default())))`
- `crates/omega-core/src/types.rs:170:9`: `<impl From<StreamSignal> for AgentItem>::from -> Self` → `Default::default()`
- `crates/omega-core/src/types.rs:176:9`: `<impl From<OmegaEvent> for AgentItem>::from -> Self` → `Default::default()`

### `omega-server` — 71 unviable

- `crates/omega-server/src/lib.rs:79:9`: `AppState::with_leptos_dir -> Self` → `Default::default()`
- `crates/omega-server/src/lib.rs:99:5`: `serve -> std::io::Result<()>` → `Ok(())`
- `crates/omega-server/src/lib.rs:133:5`: `shutdown_signal ` → `()`
- `crates/omega-server/src/lib.rs:152:5`: `perform_shutdown ` → `()`
- `crates/omega-server/src/router.rs:68:5`: `should_replay -> bool` → `true`
- `crates/omega-server/src/router.rs:68:5`: `should_replay -> bool` → `false`
- `crates/omega-server/src/router.rs:82:5`: `build_router -> Router` → `Default::default()`
- `crates/omega-server/src/router.rs:99:5`: `health -> Json<serde_json::Value>` → `Json::new()`
- `crates/omega-server/src/router.rs:99:5`: `health -> Json<serde_json::Value>` → `Json::from_iter([Default::default()])`
- `crates/omega-server/src/router.rs:99:5`: `health -> Json<serde_json::Value>` → `Json::new(Default::default())`
- `crates/omega-server/src/router.rs:99:5`: `health -> Json<serde_json::Value>` → `Json::from(Default::default())`
- `crates/omega-server/src/router.rs:126:5`: `folder_name_to_timestamp -> String` → `String::new()`
- `crates/omega-server/src/router.rs:126:5`: `folder_name_to_timestamp -> String` → `"xyzzy".into()`
- `crates/omega-server/src/router.rs:149:5`: `list_sessions -> Vec<SessionListItem>` → `vec![]`
- `crates/omega-server/src/router.rs:149:5`: `list_sessions -> Vec<SessionListItem>` → `vec![Default::default()]`
- `crates/omega-server/src/router.rs:156:13`: `list_sessions -> Vec<SessionListItem>` → `||`
- `crates/omega-server/src/router.rs:181:5`: `get_sessions -> Response` → `Default::default()`
- `crates/omega-server/src/router.rs:197:5`: `create_active_session -> Result<(ActiveSession, String), String>` → `Ok((Default::default(), String::new()))`
- `crates/omega-server/src/router.rs:197:5`: `create_active_session -> Result<(ActiveSession, String), String>` → `Ok((Default::default(), "xyzzy".into()))`
- `crates/omega-server/src/router.rs:276:5`: `post_session -> Response` → `Default::default()`
- `crates/omega-server/src/router.rs:360:5`: `ws_handler -> Response` → `Default::default()`
- `crates/omega-server/src/router.rs:374:5`: `build_session_info -> WsMessage` → `Default::default()`
- `crates/omega-server/src/router.rs:383:5`: `cache_into_message -> WsMessage` → `Default::default()`
- `crates/omega-server/src/router.rs:402:5`: `next_turn_state_for -> Option<&'static str>` → `None`
- `crates/omega-server/src/router.rs:402:5`: `next_turn_state_for -> Option<&'static str>` → `Some("")`
- `crates/omega-server/src/router.rs:402:5`: `next_turn_state_for -> Option<&'static str>` → `Some("xyzzy")`
- `crates/omega-server/src/router.rs:416:5`: `read_history_events -> Vec<OmegaEvent>` → `vec![]`
- `crates/omega-server/src/router.rs:416:5`: `read_history_events -> Vec<OmegaEvent>` → `vec![Default::default()]`
- `crates/omega-server/src/router.rs:448:5`: `send_session_info_and_history ` → `()`
- `crates/omega-server/src/router.rs:503:5`: `handle_socket ` → `()`
- `crates/omega-server/src/router.rs:532:13`: `handle_socket ` → ``
- `crates/omega-server/src/router.rs:554:5`: `install_ws_tx ` → `()`
- `crates/omega-server/src/router.rs:562:5`: `clear_ws_tx ` → `()`
- `crates/omega-server/src/router.rs:574:5`: `send_to_active ` → `()`
- `crates/omega-server/src/router.rs:576:9`: `send_to_active ` → `||`
- `crates/omega-server/src/router.rs:589:5`: `dispatch_text_frame -> Result<(), String>` → `Ok(())`
- `crates/omega-server/src/router.rs:599:5`: `dispatch_client_frame -> Result<(), String>` → `Ok(())`
- `crates/omega-server/src/router.rs:634:5`: `handle_set_model -> Result<(), String>` → `Ok(())`
- `crates/omega-server/src/router.rs:675:5`: `handle_set_effort -> Result<(), String>` → `Ok(())`
- `crates/omega-server/src/router.rs:697:5`: `handle_delete_session -> Result<(), String>` → `Ok(())`
- `crates/omega-server/src/router.rs:720:5`: `handle_user_message -> Result<(), String>` → `Ok(())`
- `crates/omega-server/src/router.rs:772:5`: `handle_pause -> Result<(), String>` → `Ok(())`
- `crates/omega-server/src/router.rs:820:5`: `handle_continue -> Result<(), String>` → `Ok(())`
- `crates/omega-server/src/router.rs:831:5`: `handle_abort -> Result<(), String>` → `Ok(())`
- `crates/omega-server/src/router.rs:862:5`: `handle_reset -> Result<(), String>` → `Ok(())`
- `crates/omega-server/src/router.rs:919:5`: `handle_resume_session -> Result<(), String>` → `Ok(())`
- `crates/omega-server/src/router.rs:1037:5`: `handle_rename_session -> Result<(), String>` → `Ok(())`
- `crates/omega-server/src/router.rs:1083:5`: `get_context -> Json<Vec<ContextRecord>>` → `Json::new()`
- `crates/omega-server/src/router.rs:1083:5`: `get_context -> Json<Vec<ContextRecord>>` → `Json::from_iter([vec![]])`
- `crates/omega-server/src/router.rs:1083:5`: `get_context -> Json<Vec<ContextRecord>>` → `Json::new(vec![])`
- `crates/omega-server/src/router.rs:1083:5`: `get_context -> Json<Vec<ContextRecord>>` → `Json::from(vec![])`
- `crates/omega-server/src/router.rs:1083:5`: `get_context -> Json<Vec<ContextRecord>>` → `Json::from_iter([vec![Default::default()]])`
- `crates/omega-server/src/router.rs:1083:5`: `get_context -> Json<Vec<ContextRecord>>` → `Json::new(vec![Default::default()])`
- `crates/omega-server/src/router.rs:1083:5`: `get_context -> Json<Vec<ContextRecord>>` → `Json::from(vec![Default::default()])`
- `crates/omega-server/src/router.rs:1127:5`: `get_files -> Json<Vec<String>>` → `Json::new()`
- `crates/omega-server/src/router.rs:1127:5`: `get_files -> Json<Vec<String>>` → `Json::from_iter([vec![]])`
- `crates/omega-server/src/router.rs:1127:5`: `get_files -> Json<Vec<String>>` → `Json::new(vec![])`
- `crates/omega-server/src/router.rs:1127:5`: `get_files -> Json<Vec<String>>` → `Json::from(vec![])`
- `crates/omega-server/src/router.rs:1127:5`: `get_files -> Json<Vec<String>>` → `Json::from_iter([vec![String::new()]])`
- `crates/omega-server/src/router.rs:1127:5`: `get_files -> Json<Vec<String>>` → `Json::new(vec![String::new()])`
- `crates/omega-server/src/router.rs:1127:5`: `get_files -> Json<Vec<String>>` → `Json::from(vec![String::new()])`
- `crates/omega-server/src/router.rs:1127:5`: `get_files -> Json<Vec<String>>` → `Json::from_iter([vec!["xyzzy".into()]])`
- `crates/omega-server/src/router.rs:1127:5`: `get_files -> Json<Vec<String>>` → `Json::new(vec!["xyzzy".into()])`
- `crates/omega-server/src/router.rs:1127:5`: `get_files -> Json<Vec<String>>` → `Json::from(vec!["xyzzy".into()])`
- `crates/omega-server/src/router.rs:1148:5`: `dir_first_then_alpha -> std::cmp::Ordering` → `Default::default()`
- `crates/omega-server/src/router.rs:1169:5`: `list_files_for_completion -> Vec<String>` → `vec![]`
- `crates/omega-server/src/router.rs:1169:5`: `list_files_for_completion -> Vec<String>` → `vec![String::new()]`
- `crates/omega-server/src/router.rs:1169:5`: `list_files_for_completion -> Vec<String>` → `vec!["xyzzy".into()]`
- `crates/omega-server/src/router.rs:1667:5`: `git_has_pending_changes -> bool` → `true`
- `crates/omega-server/src/router.rs:1667:5`: `git_has_pending_changes -> bool` → `false`
- `crates/omega-server/src/ws_message.rs:137:9`: `WsMessage::to_json -> serde_json::Value` → `Default::default()`

### `omega-agent` — 108 unviable

- `crates/omega-agent/src/agent.rs:119:5`: `append_text_slot ` → `()`
- `crates/omega-agent/src/agent.rs:136:5`: `append_thinking_slot ` → `()`
- `crates/omega-agent/src/agent.rs:151:5`: `seal_text_slot ` → `()`
- `crates/omega-agent/src/agent.rs:164:5`: `seal_thinking_slot ` → `()`
- `crates/omega-agent/src/agent.rs:189:5`: `open_tool_use_slot -> String` → `String::new()`
- `crates/omega-agent/src/agent.rs:189:5`: `open_tool_use_slot -> String` → `"xyzzy".into()`
- `crates/omega-agent/src/agent.rs:215:5`: `seal_tool_use_slot -> String` → `String::new()`
- `crates/omega-agent/src/agent.rs:215:5`: `seal_tool_use_slot -> String` → `"xyzzy".into()`
- `crates/omega-agent/src/agent.rs:258:5`: `make_abandonment_closers -> Vec<OmegaEvent>` → `vec![]`
- `crates/omega-agent/src/agent.rs:258:5`: `make_abandonment_closers -> Vec<OmegaEvent>` → `vec![Default::default()]`
- `crates/omega-agent/src/agent.rs:365:5`: `build_context_management -> serde_json::Value` → `Default::default()`
- `crates/omega-agent/src/agent.rs:520:9`: `Agent::init -> omega_store::Result<()>` → `Ok(())`
- `crates/omega-agent/src/agent.rs:589:9`: `Agent::controls -> ControlHandle` → `Default::default()`
- `crates/omega-agent/src/agent.rs:599:9`: `Agent::set_model -> OmegaEvent` → `Default::default()`
- `crates/omega-agent/src/agent.rs:613:9`: `Agent::set_effort -> OmegaEvent` → `Default::default()`
- `crates/omega-agent/src/agent.rs:639:9`: `Agent::seed_history ` → `()`
- `crates/omega-agent/src/agent.rs:672:9`: `Agent::seed_with_resumption_summary -> Result<OmegaEvent, omega_store::StoreError>` → `Ok(Default::default())`
- `crates/omega-agent/src/agent.rs:714:9`: `Agent::history -> &[Message]` → `Vec::leak(vec![Default::default()])`
- `crates/omega-agent/src/agent.rs:729:9`: `Agent::send_message -> Pin<Box<dyn Stream<Item = AgentItem>+Send +'a>>` → `Pin::new()`
- `crates/omega-agent/src/agent.rs:729:9`: `Agent::send_message -> Pin<Box<dyn Stream<Item = AgentItem>+Send +'a>>` → `Pin::from_iter([Box::new(Default::default())])`
- `crates/omega-agent/src/agent.rs:729:9`: `Agent::send_message -> Pin<Box<dyn Stream<Item = AgentItem>+Send +'a>>` → `Pin::new(Box::new(Default::default()))`
- `crates/omega-agent/src/agent.rs:729:9`: `Agent::send_message -> Pin<Box<dyn Stream<Item = AgentItem>+Send +'a>>` → `Pin::from(Box::new(Default::default()))`
- `crates/omega-agent/src/agent.rs:1683:9`: `Agent::perform_resumption -> Pin<Box<dyn Stream<Item = AgentItem>+Send +'a>>` → `Pin::new()`
- `crates/omega-agent/src/agent.rs:1683:9`: `Agent::perform_resumption -> Pin<Box<dyn Stream<Item = AgentItem>+Send +'a>>` → `Pin::from_iter([Box::new(Default::default())])`
- `crates/omega-agent/src/agent.rs:1683:9`: `Agent::perform_resumption -> Pin<Box<dyn Stream<Item = AgentItem>+Send +'a>>` → `Pin::new(Box::new(Default::default()))`
- `crates/omega-agent/src/agent.rs:1683:9`: `Agent::perform_resumption -> Pin<Box<dyn Stream<Item = AgentItem>+Send +'a>>` → `Pin::from(Box::new(Default::default()))`
- `crates/omega-agent/src/agent.rs:2080:5`: `now_iso -> String` → `String::new()`
- `crates/omega-agent/src/agent.rs:2080:5`: `now_iso -> String` → `"xyzzy".into()`
- `crates/omega-agent/src/agent.rs:2113:5`: `elide_request -> Value` → `Default::default()`
- `crates/omega-agent/src/agent.rs:2167:35`: `elide_request -> Value` → `<`
- `crates/omega-agent/src/agent.rs:2167:35`: `elide_request -> Value` → `>=`
- `crates/omega-agent/src/config.rs:25:5`: `max_output_tokens_for_model -> u32` → `0`
- `crates/omega-agent/src/config.rs:25:5`: `max_output_tokens_for_model -> u32` → `1`
- `crates/omega-agent/src/config.rs:47:5`: `cap_effort_for_model -> &'a str` → `""`
- `crates/omega-agent/src/config.rs:47:5`: `cap_effort_for_model -> &'a str` → `"xyzzy"`
- `crates/omega-agent/src/controls.rs:127:9`: `ControlHandle::request_pause ` → `()`
- `crates/omega-agent/src/controls.rs:148:9`: `ControlHandle::request_continue ` → `()`
- `crates/omega-agent/src/controls.rs:253:9`: `ControlHandle::take_pending_continue -> Option<PendingContinue>` → `Some(Default::default())`
- `crates/omega-agent/src/controls.rs:266:9`: `ControlHandle::lock_state -> std::sync::MutexGuard<'_, ControlState>` → `MutexGuard::new()`
- `crates/omega-agent/src/controls.rs:266:9`: `ControlHandle::lock_state -> std::sync::MutexGuard<'_, ControlState>` → `MutexGuard::from_iter([Default::default()])`
- `crates/omega-agent/src/controls.rs:266:9`: `ControlHandle::lock_state -> std::sync::MutexGuard<'_, ControlState>` → `MutexGuard::new(Default::default())`
- `crates/omega-agent/src/controls.rs:266:9`: `ControlHandle::lock_state -> std::sync::MutexGuard<'_, ControlState>` → `MutexGuard::from(Default::default())`
- `crates/omega-agent/src/controls.rs:270:9`: `ControlHandle::lock_cancel -> std::sync::MutexGuard<'_, CancellationToken>` → `MutexGuard::new()`
- `crates/omega-agent/src/controls.rs:270:9`: `ControlHandle::lock_cancel -> std::sync::MutexGuard<'_, CancellationToken>` → `MutexGuard::from_iter([Default::default()])`
- `crates/omega-agent/src/controls.rs:270:9`: `ControlHandle::lock_cancel -> std::sync::MutexGuard<'_, CancellationToken>` → `MutexGuard::new(Default::default())`
- `crates/omega-agent/src/controls.rs:270:9`: `ControlHandle::lock_cancel -> std::sync::MutexGuard<'_, CancellationToken>` → `MutexGuard::from(Default::default())`
- `crates/omega-agent/src/controls.rs:303:9`: `<impl Drop for TurnGuard>::drop ` → `()`
- `crates/omega-agent/src/error_classify.rs:25:5`: `is_invalid_tool_json -> bool` → `true`
- `crates/omega-agent/src/error_classify.rs:25:5`: `is_invalid_tool_json -> bool` → `false`
- `crates/omega-agent/src/error_classify.rs:40:5`: `is_context_too_long -> bool` → `true`
- `crates/omega-agent/src/error_classify.rs:40:5`: `is_context_too_long -> bool` → `false`
- `crates/omega-agent/src/session_resume.rs:71:5`: `first_meaningful_line -> String` → `String::new()`
- `crates/omega-agent/src/session_resume.rs:71:5`: `first_meaningful_line -> String` → `"xyzzy".into()`
- `crates/omega-agent/src/session_resume.rs:82:5`: `primary_tool_arg -> String` → `String::new()`
- `crates/omega-agent/src/session_resume.rs:82:5`: `primary_tool_arg -> String` → `"xyzzy".into()`
- `crates/omega-agent/src/session_resume.rs:152:5`: `group_into_turns -> Vec<Turn>` → `vec![]`
- `crates/omega-agent/src/session_resume.rs:152:5`: `group_into_turns -> Vec<Turn>` → `vec![Default::default()]`
- `crates/omega-agent/src/session_resume.rs:199:5`: `project_turn -> String` → `String::new()`
- `crates/omega-agent/src/session_resume.rs:199:5`: `project_turn -> String` → `"xyzzy".into()`
- `crates/omega-agent/src/session_resume.rs:210:13`: `project_turn -> String` → ``
- `crates/omega-agent/src/session_resume.rs:234:13`: `project_turn -> String` → ``
- `crates/omega-agent/src/session_resume.rs:237:13`: `project_turn -> String` → ``
- `crates/omega-agent/src/session_resume.rs:257:13`: `project_turn -> String` → ``
- `crates/omega-agent/src/session_resume.rs:303:5`: `extract_block -> Option<String>` → `None`
- `crates/omega-agent/src/session_resume.rs:303:5`: `extract_block -> Option<String>` → `Some(String::new())`
- `crates/omega-agent/src/session_resume.rs:303:5`: `extract_block -> Option<String>` → `Some("xyzzy".into())`
- `crates/omega-agent/src/session_resume.rs:324:5`: `extract_last_model_and_effort -> (Option<String>, Option<String>)` → `(None, None)`
- `crates/omega-agent/src/session_resume.rs:324:5`: `extract_last_model_and_effort -> (Option<String>, Option<String>)` → `(None, Some(String::new()))`
- `crates/omega-agent/src/session_resume.rs:324:5`: `extract_last_model_and_effort -> (Option<String>, Option<String>)` → `(None, Some("xyzzy".into()))`
- `crates/omega-agent/src/session_resume.rs:324:5`: `extract_last_model_and_effort -> (Option<String>, Option<String>)` → `(Some(String::new()), None)`
- `crates/omega-agent/src/session_resume.rs:324:5`: `extract_last_model_and_effort -> (Option<String>, Option<String>)` → `(Some(String::new()), Some(String::new()))`
- `crates/omega-agent/src/session_resume.rs:324:5`: `extract_last_model_and_effort -> (Option<String>, Option<String>)` → `(Some(String::new()), Some("xyzzy".into()))`
- `crates/omega-agent/src/session_resume.rs:324:5`: `extract_last_model_and_effort -> (Option<String>, Option<String>)` → `(Some("xyzzy".into()), None)`
- `crates/omega-agent/src/session_resume.rs:324:5`: `extract_last_model_and_effort -> (Option<String>, Option<String>)` → `(Some("xyzzy".into()), Some(String::new()))`
- `crates/omega-agent/src/session_resume.rs:324:5`: `extract_last_model_and_effort -> (Option<String>, Option<String>)` → `(Some("xyzzy".into()), Some("xyzzy".into()))`
- `crates/omega-agent/src/session_resume.rs:328:13`: `extract_last_model_and_effort -> (Option<String>, Option<String>)` → ``
- `crates/omega-agent/src/session_resume.rs:329:13`: `extract_last_model_and_effort -> (Option<String>, Option<String>)` → ``
- `crates/omega-agent/src/session_resume.rs:361:5`: `extract_resumption_basis -> String` → `String::new()`
- `crates/omega-agent/src/session_resume.rs:361:5`: `extract_resumption_basis -> String` → `"xyzzy".into()`
- `crates/omega-agent/src/session_resume.rs:370:9`: `extract_resumption_basis -> String` → `||`
- `crates/omega-agent/src/session_resume.rs:411:5`: `extract_summary_from_response -> String` → `String::new()`
- `crates/omega-agent/src/session_resume.rs:411:5`: `extract_summary_from_response -> String` → `"xyzzy".into()`
- `crates/omega-agent/src/session_resume.rs:422:5`: `extract_description_from_response -> Option<String>` → `None`
- `crates/omega-agent/src/session_resume.rs:422:5`: `extract_description_from_response -> Option<String>` → `Some(String::new())`
- `crates/omega-agent/src/session_resume.rs:422:5`: `extract_description_from_response -> Option<String>` → `Some("xyzzy".into())`
- `crates/omega-agent/src/system_prompt.rs:84:5`: `discover_instruction_files -> Vec<InstructionFile>` → `vec![]`
- `crates/omega-agent/src/system_prompt.rs:84:5`: `discover_instruction_files -> Vec<InstructionFile>` → `vec![Default::default()]`
- `crates/omega-agent/src/system_prompt.rs:101:5`: `discover_instruction_files_with_env -> Vec<InstructionFile>` → `vec![]`
- `crates/omega-agent/src/system_prompt.rs:101:5`: `discover_instruction_files_with_env -> Vec<InstructionFile>` → `vec![Default::default()]`
- `crates/omega-agent/src/system_prompt.rs:104:9`: `discover_instruction_files_with_env -> Vec<InstructionFile>` → `||`
- `crates/omega-agent/src/system_prompt.rs:114:9`: `discover_instruction_files_with_env -> Vec<InstructionFile>` → `||`
- `crates/omega-agent/src/system_prompt.rs:145:5`: `global_agents_md_path_from_env -> Option<PathBuf>` → `None`
- `crates/omega-agent/src/system_prompt.rs:145:5`: `global_agents_md_path_from_env -> Option<PathBuf>` → `Some(Default::default())`
- `crates/omega-agent/src/system_prompt.rs:157:5`: `repo_agents_md_path -> Option<PathBuf>` → `None`
- `crates/omega-agent/src/system_prompt.rs:157:5`: `repo_agents_md_path -> Option<PathBuf>` → `Some(Default::default())`
- `crates/omega-agent/src/system_prompt.rs:163:5`: `find_git_root -> Option<PathBuf>` → `None`
- `crates/omega-agent/src/system_prompt.rs:163:5`: `find_git_root -> Option<PathBuf>` → `Some(Default::default())`
- `crates/omega-agent/src/system_prompt.rs:177:5`: `read_existing -> Option<String>` → `None`
- `crates/omega-agent/src/system_prompt.rs:177:5`: `read_existing -> Option<String>` → `Some(String::new())`
- `crates/omega-agent/src/system_prompt.rs:177:5`: `read_existing -> Option<String>` → `Some("xyzzy".into())`
- `crates/omega-agent/src/system_prompt.rs:196:5`: `build_system_blocks -> Vec<SystemBlock>` → `vec![]`
- `crates/omega-agent/src/system_prompt.rs:196:5`: `build_system_blocks -> Vec<SystemBlock>` → `vec![Default::default()]`
- `crates/omega-agent/src/system_prompt.rs:234:5`: `join_blocks -> String` → `String::new()`
- `crates/omega-agent/src/system_prompt.rs:234:5`: `join_blocks -> String` → `"xyzzy".into()`
- `crates/omega-agent/src/system_prompt.rs:252:5`: `runtime_context -> String` → `String::new()`
- `crates/omega-agent/src/system_prompt.rs:252:5`: `runtime_context -> String` → `"xyzzy".into()`
- `crates/omega-agent/src/system_prompt.rs:271:5`: `core_prompt -> String` → `String::new()`
- `crates/omega-agent/src/system_prompt.rs:271:5`: `core_prompt -> String` → `"xyzzy".into()`

### `omega-tools` — 119 unviable

- `crates/omega-tools/src/lib.rs:45:9`: `ToolResult::ok -> Self` → `Default::default()`
- `crates/omega-tools/src/lib.rs:53:9`: `ToolResult::err -> Self` → `Default::default()`
- `crates/omega-tools/src/lib.rs:75:5`: `execute_tool -> ToolResult` → `Default::default()`
- `crates/omega-tools/src/cap_and_tee.rs:56:9`: `TruncationBias::parse_bias -> Self` → `Default::default()`
- `crates/omega-tools/src/cap_and_tee.rs:96:5`: `cap_and_tee -> io::Result<CappedOutput>` → `Ok(Default::default())`
- `crates/omega-tools/src/cap_and_tee.rs:167:5`: `format_size -> String` → `String::new()`
- `crates/omega-tools/src/cap_and_tee.rs:167:5`: `format_size -> String` → `"xyzzy".into()`
- `crates/omega-tools/src/cap_and_tee.rs:181:5`: `utf8_boundary_forward -> usize` → `0`
- `crates/omega-tools/src/cap_and_tee.rs:181:5`: `utf8_boundary_forward -> usize` → `1`
- `crates/omega-tools/src/cap_and_tee.rs:183:15`: `utf8_boundary_forward -> usize` → `<`
- `crates/omega-tools/src/cap_and_tee.rs:183:15`: `utf8_boundary_forward -> usize` → `>=`
- `crates/omega-tools/src/cap_and_tee.rs:193:5`: `utf8_boundary_backward -> usize` → `0`
- `crates/omega-tools/src/cap_and_tee.rs:193:5`: `utf8_boundary_backward -> usize` → `1`
- `crates/omega-tools/src/cap_and_tee.rs:206:5`: `is_utf8_continuation -> bool` → `true`
- `crates/omega-tools/src/cap_and_tee.rs:206:5`: `is_utf8_continuation -> bool` → `false`
- `crates/omega-tools/src/format.rs:18:5`: `format_tool_call -> String` → `String::new()`
- `crates/omega-tools/src/format.rs:18:5`: `format_tool_call -> String` → `"xyzzy".into()`
- `crates/omega-tools/src/format.rs:133:5`: `string_field -> String` → `String::new()`
- `crates/omega-tools/src/format.rs:133:5`: `string_field -> String` → `"xyzzy".into()`
- `crates/omega-tools/src/format.rs:141:5`: `num_field -> Option<f64>` → `None`
- `crates/omega-tools/src/format.rs:141:5`: `num_field -> Option<f64>` → `Some(0.0)`
- `crates/omega-tools/src/format.rs:141:5`: `num_field -> Option<f64>` → `Some(1.0)`
- `crates/omega-tools/src/format.rs:141:5`: `num_field -> Option<f64>` → `Some(-1.0)`
- `crates/omega-tools/src/output_cleaner.rs:52:5`: `clean_output -> Vec<u8>` → `vec![]`
- `crates/omega-tools/src/output_cleaner.rs:52:5`: `clean_output -> Vec<u8>` → `vec![0]`
- `crates/omega-tools/src/output_cleaner.rs:52:5`: `clean_output -> Vec<u8>` → `vec![1]`
- `crates/omega-tools/src/output_cleaner.rs:67:5`: `crlf_normalize -> Vec<u8>` → `vec![]`
- `crates/omega-tools/src/output_cleaner.rs:67:5`: `crlf_normalize -> Vec<u8>` → `vec![0]`
- `crates/omega-tools/src/output_cleaner.rs:67:5`: `crlf_normalize -> Vec<u8>` → `vec![1]`
- `crates/omega-tools/src/output_cleaner.rs:92:5`: `cr_collapse -> Vec<u8>` → `vec![]`
- `crates/omega-tools/src/output_cleaner.rs:92:5`: `cr_collapse -> Vec<u8>` → `vec![0]`
- `crates/omega-tools/src/output_cleaner.rs:92:5`: `cr_collapse -> Vec<u8>` → `vec![1]`
- `crates/omega-tools/src/output_cleaner.rs:124:5`: `ansi_strip -> Vec<u8>` → `vec![]`
- `crates/omega-tools/src/output_cleaner.rs:124:5`: `ansi_strip -> Vec<u8>` → `vec![0]`
- `crates/omega-tools/src/output_cleaner.rs:124:5`: `ansi_strip -> Vec<u8>` → `vec![1]`
- `crates/omega-tools/src/schemas.rs:16:5`: `tool_definitions -> Vec<ToolDefinition>` → `vec![]`
- `crates/omega-tools/src/schemas.rs:16:5`: `tool_definitions -> Vec<ToolDefinition>` → `vec![Default::default()]`
- `crates/omega-tools/src/schemas.rs:37:5`: `read_file -> ToolDefinition` → `Default::default()`
- `crates/omega-tools/src/schemas.rs:55:5`: `write_file -> ToolDefinition` → `Default::default()`
- `crates/omega-tools/src/schemas.rs:76:5`: `run_command -> ToolDefinition` → `Default::default()`
- `crates/omega-tools/src/schemas.rs:112:5`: `edit_file -> ToolDefinition` → `Default::default()`
- `crates/omega-tools/src/schemas.rs:146:5`: `list_files -> ToolDefinition` → `Default::default()`
- `crates/omega-tools/src/schemas.rs:163:5`: `web_search -> ToolDefinition` → `Default::default()`
- `crates/omega-tools/src/schemas.rs:179:5`: `fetch_url -> ToolDefinition` → `Default::default()`
- `crates/omega-tools/src/schemas.rs:208:5`: `grep_files -> ToolDefinition` → `Default::default()`
- `crates/omega-tools/src/schemas.rs:232:5`: `find_files -> ToolDefinition` → `Default::default()`
- `crates/omega-tools/src/schemas.rs:255:5`: `run_background -> ToolDefinition` → `Default::default()`
- `crates/omega-tools/src/schemas.rs:278:5`: `wait_for_output -> ToolDefinition` → `Default::default()`
- `crates/omega-tools/src/schemas.rs:310:5`: `write_stdin -> ToolDefinition` → `Default::default()`
- `crates/omega-tools/src/state.rs:39:5`: `processes -> &'static Registry` → `Box::leak(Box::new(Default::default()))`
- `crates/omega-tools/src/state.rs:52:5`: `next_id -> u64` → `0`
- `crates/omega-tools/src/state.rs:52:5`: `next_id -> u64` → `1`
- `crates/omega-tools/src/tools/edit_file.rs:11:5`: `execute -> Result<String, String>` → `Ok(String::new())`
- `crates/omega-tools/src/tools/edit_file.rs:11:5`: `execute -> Result<String, String>` → `Ok("xyzzy".into())`
- `crates/omega-tools/src/tools/edit_file.rs:100:5`: `count_occurrences -> usize` → `0`
- `crates/omega-tools/src/tools/edit_file.rs:100:5`: `count_occurrences -> usize` → `1`
- `crates/omega-tools/src/tools/fetch_url.rs:35:5`: `run_subprocess -> Result<SubprocOutput, String>` → `Ok(Default::default())`
- `crates/omega-tools/src/tools/fetch_url.rs:67:5`: `cache_dir -> &'static PathBuf` → `Box::leak(Box::new(Default::default()))`
- `crates/omega-tools/src/tools/fetch_url.rs:80:5`: `execute -> Result<String, String>` → `Ok(String::new())`
- `crates/omega-tools/src/tools/fetch_url.rs:80:5`: `execute -> Result<String, String>` → `Ok("xyzzy".into())`
- `crates/omega-tools/src/tools/fetch_url.rs:230:5`: `make_fetch_pp_log_path -> PathBuf` → `Default::default()`
- `crates/omega-tools/src/tools/fetch_url.rs:251:5`: `html_to_text -> String` → `String::new()`
- `crates/omega-tools/src/tools/fetch_url.rs:251:5`: `html_to_text -> String` → `"xyzzy".into()`
- `crates/omega-tools/src/tools/find_files.rs:16:5`: `execute -> Result<String, String>` → `Ok(String::new())`
- `crates/omega-tools/src/tools/find_files.rs:16:5`: `execute -> Result<String, String>` → `Ok("xyzzy".into())`
- `crates/omega-tools/src/tools/find_files.rs:60:5`: `walk -> Result<Vec<String>, String>` → `Ok(vec![])`
- `crates/omega-tools/src/tools/find_files.rs:60:5`: `walk -> Result<Vec<String>, String>` → `Ok(vec![String::new()])`
- `crates/omega-tools/src/tools/find_files.rs:60:5`: `walk -> Result<Vec<String>, String>` → `Ok(vec!["xyzzy".into()])`
- `crates/omega-tools/src/tools/grep_files.rs:27:5`: `execute -> Result<String, String>` → `Ok(String::new())`
- `crates/omega-tools/src/tools/grep_files.rs:27:5`: `execute -> Result<String, String>` → `Ok("xyzzy".into())`
- `crates/omega-tools/src/tools/grep_files.rs:92:5`: `search -> Result<(Vec<String>, bool), String>` → `Ok((vec![], true))`
- `crates/omega-tools/src/tools/grep_files.rs:92:5`: `search -> Result<(Vec<String>, bool), String>` → `Ok((vec![], false))`
- `crates/omega-tools/src/tools/grep_files.rs:92:5`: `search -> Result<(Vec<String>, bool), String>` → `Ok((vec![String::new()], true))`
- `crates/omega-tools/src/tools/grep_files.rs:92:5`: `search -> Result<(Vec<String>, bool), String>` → `Ok((vec![String::new()], false))`
- `crates/omega-tools/src/tools/grep_files.rs:92:5`: `search -> Result<(Vec<String>, bool), String>` → `Ok((vec!["xyzzy".into()], true))`
- `crates/omega-tools/src/tools/grep_files.rs:92:5`: `search -> Result<(Vec<String>, bool), String>` → `Ok((vec!["xyzzy".into()], false))`
- `crates/omega-tools/src/tools/grep_files.rs:120:13`: `search -> Result<(Vec<String>, bool), String>` → `||`
- `crates/omega-tools/src/tools/grep_files.rs:162:5`: `search_file -> bool` → `true`
- `crates/omega-tools/src/tools/grep_files.rs:162:5`: `search_file -> bool` → `false`
- `crates/omega-tools/src/tools/list_files.rs:12:5`: `execute -> Result<String, String>` → `Ok(String::new())`
- `crates/omega-tools/src/tools/list_files.rs:12:5`: `execute -> Result<String, String>` → `Ok("xyzzy".into())`
- `crates/omega-tools/src/tools/list_files.rs:47:5`: `walk_sync -> Result<(), String>` → `Ok(())`
- `crates/omega-tools/src/tools/read_file.rs:10:5`: `execute -> Result<String, String>` → `Ok(String::new())`
- `crates/omega-tools/src/tools/read_file.rs:10:5`: `execute -> Result<String, String>` → `Ok("xyzzy".into())`
- `crates/omega-tools/src/tools/read_file.rs:65:5`: `char_boundary_at_or_before -> usize` → `0`
- `crates/omega-tools/src/tools/read_file.rs:65:5`: `char_boundary_at_or_before -> usize` → `1`
- `crates/omega-tools/src/tools/run_background.rs:12:5`: `execute -> Result<String, String>` → `Ok(String::new())`
- `crates/omega-tools/src/tools/run_background.rs:12:5`: `execute -> Result<String, String>` → `Ok("xyzzy".into())`
- `crates/omega-tools/src/tools/run_command.rs:51:5`: `execute -> Result<String, String>` → `Ok(String::new())`
- `crates/omega-tools/src/tools/run_command.rs:51:5`: `execute -> Result<String, String>` → `Ok("xyzzy".into())`
- `crates/omega-tools/src/tools/run_command.rs:248:5`: `make_run_log_path -> PathBuf` → `Default::default()`
- `crates/omega-tools/src/tools/run_command.rs:267:5`: `sanitize_tag -> String` → `String::new()`
- `crates/omega-tools/src/tools/run_command.rs:267:5`: `sanitize_tag -> String` → `"xyzzy".into()`
- `crates/omega-tools/src/tools/run_command.rs:283:5`: `kill_group ` → `()`
- `crates/omega-tools/src/tools/wait_for_output.rs:31:5`: `execute -> Result<String, String>` → `Ok(String::new())`
- `crates/omega-tools/src/tools/wait_for_output.rs:31:5`: `execute -> Result<String, String>` → `Ok("xyzzy".into())`
- `crates/omega-tools/src/tools/wait_for_output.rs:49:46`: `execute -> Result<String, String>` → `*`
- `crates/omega-tools/src/tools/wait_for_output.rs:128:5`: `make_wait_log_path -> PathBuf` → `Default::default()`
- `crates/omega-tools/src/tools/wait_for_output.rs:154:5`: `done -> Result<String, String>` → `Ok(String::new())`
- `crates/omega-tools/src/tools/wait_for_output.rs:154:5`: `done -> Result<String, String>` → `Ok("xyzzy".into())`
- `crates/omega-tools/src/tools/wait_for_output.rs:175:5`: `read_log -> String` → `String::new()`
- `crates/omega-tools/src/tools/wait_for_output.rs:175:5`: `read_log -> String` → `"xyzzy".into()`
- `crates/omega-tools/src/tools/wait_for_output.rs:179:5`: `check_exit -> Option<i32>` → `None`
- `crates/omega-tools/src/tools/wait_for_output.rs:179:5`: `check_exit -> Option<i32>` → `Some(0)`
- `crates/omega-tools/src/tools/wait_for_output.rs:179:5`: `check_exit -> Option<i32>` → `Some(1)`
- `crates/omega-tools/src/tools/wait_for_output.rs:179:5`: `check_exit -> Option<i32>` → `Some(-1)`
- `crates/omega-tools/src/tools/wait_for_output.rs:199:5`: `evaluate -> (bool, bool)` → `(true, true)`
- `crates/omega-tools/src/tools/wait_for_output.rs:199:5`: `evaluate -> (bool, bool)` → `(true, false)`
- `crates/omega-tools/src/tools/wait_for_output.rs:199:5`: `evaluate -> (bool, bool)` → `(false, true)`
- `crates/omega-tools/src/tools/wait_for_output.rs:199:5`: `evaluate -> (bool, bool)` → `(false, false)`
- `crates/omega-tools/src/tools/web_search.rs:12:5`: `execute -> Result<String, String>` → `Ok(String::new())`
- `crates/omega-tools/src/tools/web_search.rs:12:5`: `execute -> Result<String, String>` → `Ok("xyzzy".into())`
- `crates/omega-tools/src/tools/web_search.rs:64:5`: `check_status -> Result<(), String>` → `Ok(())`
- `crates/omega-tools/src/tools/web_search.rs:73:5`: `render_results -> String` → `String::new()`
- `crates/omega-tools/src/tools/web_search.rs:73:5`: `render_results -> String` → `"xyzzy".into()`
- `crates/omega-tools/src/tools/write_file.rs:7:5`: `execute -> Result<String, String>` → `Ok(String::new())`
- `crates/omega-tools/src/tools/write_file.rs:7:5`: `execute -> Result<String, String>` → `Ok("xyzzy".into())`
- `crates/omega-tools/src/tools/write_stdin.rs:10:5`: `execute -> Result<String, String>` → `Ok(String::new())`
- `crates/omega-tools/src/tools/write_stdin.rs:10:5`: `execute -> Result<String, String>` → `Ok("xyzzy".into())`

## 5. Kills That May Not Reflect Real-Life Calls

These caught mutants were flagged by heuristics as potentially being killed only by test infrastructure rather than by tests that exercise production code paths. **They should be reviewed**: if the flag is correct, the apparent coverage is illusory.

Heuristics applied:
1. Mutant is in `omega-test-fixtures` (fixture-tests are circular).
2. All call-sites of the mutated function within the same crate are in test files or `#[cfg(test)]` blocks.

### `omega-test-fixtures` — 9 flagged kill(s)

#### `default_input_tokens` — crates/omega-test-fixtures/src/lib.rs:109:5

- **Mutant:** `0`
- **Flag reason:** This mutant lives in **omega-test-fixtures**, which is test-only infrastructure. The only tests that exercise it are its own unit tests — those kills confirm the fixture behaves as written, not that production code is covered.

```rust
   106 │ }
   107 │ 
   108 │ const fn default_input_tokens() -> i64 {
→  109 │     10
   110 │ }
   111 │ 
   112 │ const fn default_output_tokens() -> i64 {
```

#### `default_input_tokens` — crates/omega-test-fixtures/src/lib.rs:109:5

- **Mutant:** `1`
- **Flag reason:** This mutant lives in **omega-test-fixtures**, which is test-only infrastructure. The only tests that exercise it are its own unit tests — those kills confirm the fixture behaves as written, not that production code is covered.

```rust
   106 │ }
   107 │ 
   108 │ const fn default_input_tokens() -> i64 {
→  109 │     10
   110 │ }
   111 │ 
   112 │ const fn default_output_tokens() -> i64 {
```

#### `default_input_tokens` — crates/omega-test-fixtures/src/lib.rs:109:5

- **Mutant:** `-1`
- **Flag reason:** This mutant lives in **omega-test-fixtures**, which is test-only infrastructure. The only tests that exercise it are its own unit tests — those kills confirm the fixture behaves as written, not that production code is covered.

```rust
   106 │ }
   107 │ 
   108 │ const fn default_input_tokens() -> i64 {
→  109 │     10
   110 │ }
   111 │ 
   112 │ const fn default_output_tokens() -> i64 {
```

#### `default_output_tokens` — crates/omega-test-fixtures/src/lib.rs:113:5

- **Mutant:** `0`
- **Flag reason:** This mutant lives in **omega-test-fixtures**, which is test-only infrastructure. The only tests that exercise it are its own unit tests — those kills confirm the fixture behaves as written, not that production code is covered.

```rust
   110 │ }
   111 │ 
   112 │ const fn default_output_tokens() -> i64 {
→  113 │     5
   114 │ }
   115 │ 
   116 │ // ---------------------------------------------------------------------------
```

#### `default_output_tokens` — crates/omega-test-fixtures/src/lib.rs:113:5

- **Mutant:** `1`
- **Flag reason:** This mutant lives in **omega-test-fixtures**, which is test-only infrastructure. The only tests that exercise it are its own unit tests — those kills confirm the fixture behaves as written, not that production code is covered.

```rust
   110 │ }
   111 │ 
   112 │ const fn default_output_tokens() -> i64 {
→  113 │     5
   114 │ }
   115 │ 
   116 │ // ---------------------------------------------------------------------------
```

#### `default_output_tokens` — crates/omega-test-fixtures/src/lib.rs:113:5

- **Mutant:** `-1`
- **Flag reason:** This mutant lives in **omega-test-fixtures**, which is test-only infrastructure. The only tests that exercise it are its own unit tests — those kills confirm the fixture behaves as written, not that production code is covered.

```rust
   110 │ }
   111 │ 
   112 │ const fn default_output_tokens() -> i64 {
→  113 │     5
   114 │ }
   115 │ 
   116 │ // ---------------------------------------------------------------------------
```

#### `CallHistory::snapshot` — crates/omega-test-fixtures/src/lib.rs:187:9

- **Mutant:** `vec![]`
- **Flag reason:** This mutant lives in **omega-test-fixtures**, which is test-only infrastructure. The only tests that exercise it are its own unit tests — those kills confirm the fixture behaves as written, not that production code is covered.

```rust
   184 │ 
   185 │     #[must_use]
   186 │     pub fn snapshot(&self) -> Vec<CapturedCall> {
→  187 │         self.inner.lock().map(|g| g.clone()).unwrap_or_default()
   188 │     }
   189 │ 
   190 │     pub fn reset(&self) {
```

#### `CallHistory::reset` — crates/omega-test-fixtures/src/lib.rs:191:9

- **Mutant:** `()`
- **Flag reason:** This mutant lives in **omega-test-fixtures**, which is test-only infrastructure. The only tests that exercise it are its own unit tests — those kills confirm the fixture behaves as written, not that production code is covered.

```rust
   188 │     }
   189 │ 
   190 │     pub fn reset(&self) {
→  191 │         if let Ok(mut g) = self.inner.lock() {
   192 │             g.clear();
   193 │         }
   194 │     }
```

#### `project_message` — crates/omega-test-fixtures/src/lib.rs:341:62

- **Mutant:** `!=`
- **Flag reason:** This mutant lives in **omega-test-fixtures**, which is test-only infrastructure. The only tests that exercise it are its own unit tests — those kills confirm the fixture behaves as written, not that production code is covered.

```rust
   338 │         Value::String(s) => s.clone(),
   339 │         Value::Array(arr) => {
   340 │             if let [block] = arr.as_slice()
→  341 │                 && block.get("type").and_then(Value::as_str) == Some("text")
   342 │                 && let Some(t) = block.get("text").and_then(Value::as_str)
   343 │             {
   344 │                 return CapturedMessage {
```

### `omega-store` — 4 flagged kill(s)

#### `ContextStore::read_all` — crates/omega-store/src/context_store.rs:121:9

- **Mutant:** `Ok(vec![])`
- **Flag reason:** All call-sites of `ContextStore::read_all` found in this crate appear to be inside test code (tests/context_store.rs:268). The kill may not reflect production behaviour.

```rust
   118 │     ///
   119 │     /// Returns an error only for I/O failures other than "file not found".
   120 │     pub async fn read_all(&self) -> Result<Vec<ContextRecord>> {
→  121 │         let text = match tokio::fs::read_to_string(&self.path).await {
   122 │             Ok(t) => t,
   123 │             Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
   124 │             Err(e) => return Err(StoreError::Io(e)),
```

#### `ContextStore::read_all` — crates/omega-store/src/context_store.rs:123:32

- **Mutant:** `!=`
- **Flag reason:** All call-sites of `ContextStore::read_all` found in this crate appear to be inside test code (tests/context_store.rs:268). The kill may not reflect production behaviour.

```rust
   120 │     pub async fn read_all(&self) -> Result<Vec<ContextRecord>> {
   121 │         let text = match tokio::fs::read_to_string(&self.path).await {
   122 │             Ok(t) => t,
→  123 │             Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
   124 │             Err(e) => return Err(StoreError::Io(e)),
   125 │         };
   126 │         let records = text
```

#### `ContextStore::read_all` — crates/omega-store/src/context_store.rs:128:25

- **Mutant:** ``
- **Flag reason:** All call-sites of `ContextStore::read_all` found in this crate appear to be inside test code (tests/context_store.rs:268). The kill may not reflect production behaviour.

```rust
   125 │         };
   126 │         let records = text
   127 │             .lines()
→  128 │             .filter(|l| !l.trim().is_empty())
   129 │             .filter_map(|l| serde_json::from_str(l).ok())
   130 │             .collect();
   131 │         Ok(records)
```

#### `ContextStore::verify_record` — crates/omega-store/src/context_store.rs:169:23

- **Mutant:** `!=`
- **Flag reason:** All call-sites of `ContextStore::verify_record` found in this crate appear to be inside test code (tests/context_store.rs:371, tests/context_store.rs:395, tests/context_store.rs:426, tests/context_store.rs:540, tests/context_store.rs:563). The kill may not reflect production behaviour.

```rust
   166 │     /// [`StoreError::HashMismatch`]: crate::StoreError::HashMismatch
   167 │     pub fn verify_record(record: &ContextRecord) -> Result<()> {
   168 │         let recomputed = content_hash(&record.role, &record.content);
→  169 │         if recomputed == record.hash {
   170 │             Ok(())
   171 │         } else {
   172 │             Err(StoreError::HashMismatch {
```

### `omega-agent` — 4 flagged kill(s)

#### `extract_resumption_basis` — crates/omega-agent/src/session_resume.rs:373:12

- **Mutant:** ``
- **Flag reason:** All call-sites of `extract_resumption_basis` found in this crate appear to be inside test code (src/session_resume.rs:905, src/session_resume.rs:915, src/session_resume.rs:927, src/session_resume.rs:934, src/session_resume.rs:951). The kill may not reflect production behaviour.

```rust
   370 │         && let OmegaEvent::SessionResumed(e) = &events[idx]
   371 │     {
   372 │         let summary = e.summary.trim();
→  373 │         if !summary.is_empty() {
   374 │             parts.push(format!("## Carried-forward context\n\n{summary}"));
   375 │         }
   376 │     }
```

#### `extract_resumption_basis` — crates/omega-agent/src/session_resume.rs:383:8

- **Mutant:** ``
- **Flag reason:** All call-sites of `extract_resumption_basis` found in this crate appear to be inside test code (src/session_resume.rs:905, src/session_resume.rs:915, src/session_resume.rs:927, src/session_resume.rs:934, src/session_resume.rs:951). The kill may not reflect production behaviour.

```rust
   380 │     let relevant = &events[start..];
   381 │     let turns = group_into_turns(relevant);
   382 │ 
→  383 │     if !turns.is_empty() {
   384 │         let turn_strs: Vec<String> = turns
   385 │             .iter()
   386 │             .enumerate()
```

#### `extract_resumption_basis` — crates/omega-agent/src/session_resume.rs:387:45

- **Mutant:** `-`
- **Flag reason:** All call-sites of `extract_resumption_basis` found in this crate appear to be inside test code (src/session_resume.rs:905, src/session_resume.rs:915, src/session_resume.rs:927, src/session_resume.rs:934, src/session_resume.rs:951). The kill may not reflect production behaviour.

```rust
   384 │         let turn_strs: Vec<String> = turns
   385 │             .iter()
   386 │             .enumerate()
→  387 │             .map(|(i, t)| project_turn(t, i + 1))
   388 │             .collect();
   389 │         parts.push(format!("## Session events\n\n{}", turn_strs.join("\n\n")));
   390 │     }
```

#### `extract_resumption_basis` — crates/omega-agent/src/session_resume.rs:387:45

- **Mutant:** `*`
- **Flag reason:** All call-sites of `extract_resumption_basis` found in this crate appear to be inside test code (src/session_resume.rs:905, src/session_resume.rs:915, src/session_resume.rs:927, src/session_resume.rs:934, src/session_resume.rs:951). The kill may not reflect production behaviour.

```rust
   384 │         let turn_strs: Vec<String> = turns
   385 │             .iter()
   386 │             .enumerate()
→  387 │             .map(|(i, t)| project_turn(t, i + 1))
   388 │             .collect();
   389 │         parts.push(format!("## Session events\n\n{}", turn_strs.join("\n\n")));
   390 │     }
```

### `omega-tools` — 3 flagged kill(s)

#### `TruncationBias::parse_bias` — crates/omega-tools/src/cap_and_tee.rs:57:13

- **Mutant:** ``
- **Flag reason:** All call-sites of `TruncationBias::parse_bias` found in this crate appear to be inside test code (src/cap_and_tee.rs:474, src/cap_and_tee.rs:475, src/cap_and_tee.rs:476, src/cap_and_tee.rs:481, src/cap_and_tee.rs:482). The kill may not reflect production behaviour.

```rust
    54 │     #[must_use]
    55 │     pub fn parse_bias(s: &str) -> Self {
    56 │         match s {
→   57 │             "tail" => TruncationBias::Tail,
    58 │             "middle" => TruncationBias::Middle,
    59 │             _ => TruncationBias::Head,
    60 │         }
```

#### `TruncationBias::parse_bias` — crates/omega-tools/src/cap_and_tee.rs:58:13

- **Mutant:** ``
- **Flag reason:** All call-sites of `TruncationBias::parse_bias` found in this crate appear to be inside test code (src/cap_and_tee.rs:474, src/cap_and_tee.rs:475, src/cap_and_tee.rs:476, src/cap_and_tee.rs:481, src/cap_and_tee.rs:482). The kill may not reflect production behaviour.

```rust
    55 │     pub fn parse_bias(s: &str) -> Self {
    56 │         match s {
    57 │             "tail" => TruncationBias::Tail,
→   58 │             "middle" => TruncationBias::Middle,
    59 │             _ => TruncationBias::Head,
    60 │         }
    61 │     }
```

#### `format_tool_call` — crates/omega-tools/src/format.rs:42:35

- **Mutant:** `!=`
- **Flag reason:** All call-sites of `format_tool_call` found in this crate appear to be inside test code (src/format.rs:152, src/format.rs:159, src/format.rs:168, src/format.rs:174, src/format.rs:179). The kill may not reflect production behaviour.

```rust
    39 │                 .get("replacements")
    40 │                 .and_then(Value::as_array)
    41 │                 .map_or(0, Vec::len);
→   42 │             let suffix = if count == 1 { "" } else { "s" };
    43 │             format!(
    44 │                 "edit_file: {} ({count} replacement{suffix})",
    45 │                 string_field(input, "path"),
```

## 6. Skipped Mutants Review (`#[mutants::skip]`)

Every `#[mutants::skip]` annotation suppresses an entire function's mutant generation. Each one should have a documented, still-valid rationale. Annotations are listed below with the comment found immediately above them.

**Total skipped functions: 8**

### `pending_continue_ready` — `crates/omega-agent/src/controls.rs:232`

**Rationale from source comment:**
> /// True if a pending continue is recorded. Re-checked under lock at /// the top of each suspend-loop iteration. /// /// `cargo mutants` flags `-> true` as a surviving mutant: the WS /// pause tests can't distinguish "agent skipped its wait loop" from /// "agent waited and was woken", because both produce the same /// observable frame sequence. Manual review confirms the wait-loop /// invariant is required for correctness; flagged as accepted dead /// code at the mutation-testing level.

```rust
   227 │     /// pause tests can't distinguish "agent skipped its wait loop" from
   228 │     /// "agent waited and was woken", because both produce the same
   229 │     /// observable frame sequence. Manual review confirms the wait-loop
   230 │     /// invariant is required for correctness; flagged as accepted dead
   231 │     /// code at the mutation-testing level.
→  232 │     #[mutants::skip]
   233 │     pub(crate) fn pending_continue_ready(&self) -> bool {
   234 │         self.lock_state().pending_continue.is_some()
   235 │     }
   236 │ 
   237 │     /// Clear the `suspended` flag once the seam wakes.
```

**Review:** ⚠️  The annotation documents a mutant that is untestable via the current test harness (the two behaviours produce identical observable output, or the covering test is an out-of-process / browser spec not reachable by `cargo mutants`). The in-source comment confirms this was reviewed manually. Accepted for now, **but should be re-evaluated** if the test infrastructure around this code changes.

### `exit_suspend` — `crates/omega-agent/src/controls.rs:246`

**Rationale from source comment:**
> /// Clear the `suspended` flag once the seam wakes. /// /// `cargo mutants` flags the empty-body mutant as surviving: the /// WS pause tests only exercise a single pause cycle per turn, so /// leaving `suspended` stuck-true never re-enters `try_enter_suspend` /// inside the same turn and therefore goes unnoticed. The flag is /// still load-bearing for multi-pause cycles (covered by the /// `multiple_pause_cycles_in_one_turn` Playwright spec, which mutates /// out-of-process and isn't reachable from `cargo mutants`).

```rust
   241 │     /// leaving `suspended` stuck-true never re-enters `try_enter_suspend`
   242 │     /// inside the same turn and therefore goes unnoticed. The flag is
   243 │     /// still load-bearing for multi-pause cycles (covered by the
   244 │     /// `multiple_pause_cycles_in_one_turn` Playwright spec, which mutates
   245 │     /// out-of-process and isn't reachable from `cargo mutants`).
→  246 │     #[mutants::skip]
   247 │     pub(crate) fn exit_suspend(&self) {
   248 │         self.lock_state().suspended = false;
   249 │     }
   250 │ 
   251 │     /// Take the pending continue (if any) for the seam to act on.
```

**Review:** ⚠️  The annotation documents a mutant that is untestable via the current test harness (the two behaviours produce identical observable output, or the covering test is an out-of-process / browser spec not reachable by `cargo mutants`). The in-source comment confirms this was reviewed manually. Accepted for now, **but should be re-evaluated** if the test infrastructure around this code changes.

### `now_iso` — `crates/omega-agent/src/controls.rs:336`

**Rationale from source comment:**
> // --------------------------------------------------------------------------- // Time helper // --------------------------------------------------------------------------- /// Wall-clock ISO-8601 timestamp helper for control events. /// /// `cargo mutants` flags both string-replacement mutants as surviving: /// every event carrying a `time` field is redacted in WS / CLI snapshots /// (timestamps would make snapshots flaky), so a corrupted timestamp /// never fails a downstream assertion. Accepted dead code at the /// mutation-testing level — the format is exercised manually and by /// `chrono`'s own tests.

```rust
   331 │ /// every event carrying a `time` field is redacted in WS / CLI snapshots
   332 │ /// (timestamps would make snapshots flaky), so a corrupted timestamp
   333 │ /// never fails a downstream assertion. Accepted dead code at the
   334 │ /// mutation-testing level — the format is exercised manually and by
   335 │ /// `chrono`'s own tests.
→  336 │ #[mutants::skip]
   337 │ fn now_iso() -> String {
   338 │     chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
   339 │ }
   340 │ 
   341 │ // ---------------------------------------------------------------------------
```

**Review:** ✅ The annotation suppresses a mutant for timestamp formatting delegated to a well-tested library (`chrono`). Testing it would require mocking wall-clock time. Rationale is sound.

### `slice_start_after` — `crates/omega-agent/src/session_resume.rs:291`

**Rationale from source comment:**
> // --------------------------------------------------------------------------- // Private helper — slice-start calculation // --------------------------------------------------------------------------- /// Returns the index of the first event to include in the relevant slice — /// the position immediately *after* `resumed_idx`. /// /// `#[mutants::skip]`: the `i + 1 → i * 1` mutation is behaviourally /// equivalent. `session_resumed` events are transparent to /// [`group_into_turns`]: they fall into the `_ => {}` drop branch outside /// any open turn, so whether the relevant slice starts at `i` (the /// `session_resumed` itself) or at `i + 1` (the next event), the rendered /// turns are identical.

```rust
   286 │ /// equivalent. `session_resumed` events are transparent to
   287 │ /// [`group_into_turns`]: they fall into the `_ => {}` drop branch outside
   288 │ /// any open turn, so whether the relevant slice starts at `i` (the
   289 │ /// `session_resumed` itself) or at `i + 1` (the next event), the rendered
   290 │ /// turns are identical.
→  291 │ #[mutants::skip]
   292 │ fn slice_start_after(resumed_idx: Option<usize>) -> usize {
   293 │     resumed_idx.map_or(0, |i| i + 1)
   294 │ }
   295 │ 
   296 │ // ---------------------------------------------------------------------------
```

**Review:** ✅ The annotation documents an *equivalent mutant* — a code change that cannot alter observable behaviour. The comment explains the equivalence. Rationale appears sound; no action needed unless the surrounding code changes.

### _(anonymous)_ — `crates/omega-core/src/retry.rs:161`

**Rationale from source comment:**
> // --------------------------------------------------------------------------- // Backoff + event construction // --------------------------------------------------------------------------- /// Apply a jitter factor to a wait duration in milliseconds. /// /// `x / f` is mathematically indistinguishable from `x * f` for `f ∈ [0.9, 1.1]` /// because the two output ranges overlap completely and the factor is chosen by a /// non-deterministic RNG. Suppressed rather than tested with a fragile statistical /// assertion or a seeded-RNG refactor.

```rust
   156 │ ///
   157 │ /// `x / f` is mathematically indistinguishable from `x * f` for `f ∈ [0.9, 1.1]`
   158 │ /// because the two output ranges overlap completely and the factor is chosen by a
   159 │ /// non-deterministic RNG. Suppressed rather than tested with a fragile statistical
   160 │ /// assertion or a seeded-RNG refactor.
→  161 │ #[mutants::skip]
   162 │ #[allow(
   163 │     clippy::cast_precision_loss,
   164 │     clippy::cast_possible_truncation,
   165 │     clippy::cast_sign_loss
   166 │ )]
```

**Review:** ✅ The annotation suppresses a mutant in non-deterministic (RNG-dependent) code that cannot be meaningfully tested by a deterministic mutant check. Rationale is sound.

### `main` — `crates/omega-server/src/main.rs:19`

**Rationale from source comment:**
> /// All logic lives in helpers in `lib.rs`; `main` is pure glue.

```rust
    14 │ use clap::Parser as _;
    15 │ use omega_core::{AnthropicProvider, RetryConfig, RetryingProvider};
    16 │ use omega_server::{AppState, Args, cli, serve};
    17 │ 
    18 │ /// All logic lives in helpers in `lib.rs`; `main` is pure glue.
→   19 │ #[mutants::skip]
    20 │ #[tokio::main]
    21 │ async fn main() -> std::io::Result<()> {
    22 │     // Load .env files in priority order (first writer wins for each key):
    23 │     //   1. CWD .env  — project-level overrides (e.g. local mock API URL)
    24 │     //   2. ~/.config/omega/.env — user-level secrets (API keys, etc.)
```

**Review:** ✅ `main()` is pure OS-level glue (bind/spawn). Mutation-testing it would require process-level infrastructure. Coverage of the logic it delegates to is provided by integration tests.

### `new_script` — `crates/omega-test-fixtures/src/lib.rs:132`

**Rationale from source comment:**
> // `Default::default()` for `Arc<Mutex<VecDeque<MockResponse>>>` produces // `Arc::new(Mutex::new(VecDeque::new()))` — byte-identical to the body // below. The mutation `replace new_script -> Script with Default::default()` // is therefore value-equivalent and impossible to kill via behaviour. Skip // it explicitly rather than carry a survivor that no test can ever fail on.

```rust
   127 │ // `Default::default()` for `Arc<Mutex<VecDeque<MockResponse>>>` produces
   128 │ // `Arc::new(Mutex::new(VecDeque::new()))` — byte-identical to the body
   129 │ // below. The mutation `replace new_script -> Script with Default::default()`
   130 │ // is therefore value-equivalent and impossible to kill via behaviour. Skip
   131 │ // it explicitly rather than carry a survivor that no test can ever fail on.
→  132 │ #[mutants::skip]
   133 │ pub fn new_script() -> Script {
   134 │     Arc::new(Mutex::new(VecDeque::new()))
   135 │ }
   136 │ 
   137 │ #[must_use]
```

**Review:** ✅ The annotation documents an *equivalent mutant* — a code change that cannot alter observable behaviour. The comment explains the equivalence. Rationale appears sound; no action needed unless the surrounding code changes.

### `not_a_regular_file` — `crates/omega-tools/src/tools/grep_files.rs:148`

**Rationale from source comment:**
> /// Search a single file for `re`, appending formatted match/context lines to /// `results`.  Returns `true` if `max` was reached (caller should stop). /// /// Output format per line: /// - match:   `<path>:<1-based-line-number>:<line-text>` /// - context: `<path>:<1-based-line-number>-<line-text>` /// /// Returns `true` when `ft` is not a regular file. /// /// The guard exists to avoid pointless `read_to_string` syscalls on directory /// entries.  Removing the `!` produces an *equivalent* mutation because /// `read_to_string` on a directory fails silently and yields zero matches — /// the observable output is identical.  The function is isolated here so that /// `#[mutants::skip]` suppresses only this one expression.

```rust
   143 │ /// The guard exists to avoid pointless `read_to_string` syscalls on directory
   144 │ /// entries.  Removing the `!` produces an *equivalent* mutation because
   145 │ /// `read_to_string` on a directory fails silently and yields zero matches —
   146 │ /// the observable output is identical.  The function is isolated here so that
   147 │ /// `#[mutants::skip]` suppresses only this one expression.
→  148 │ #[mutants::skip]
   149 │ fn not_a_regular_file(ft: std::fs::FileType) -> bool {
   150 │     !ft.is_file()
   151 │ }
   152 │ 
   153 │ /// A `--` separator is emitted between non-adjacent match groups.
```

**Review:** ✅ The annotation documents an *equivalent mutant* — a code change that cannot alter observable behaviour. The comment explains the equivalence. Rationale appears sound; no action needed unless the surrounding code changes.

## 7. `exclude_re` Patterns in `.cargo/mutants.toml`

These regexes match against the mutant description string (as shown by `cargo mutants --list`) and suppress matching mutants globally. They are harder to trace than `#[mutants::skip]` because they are not co-located with the code they affect.

- `delete match arm Message::Close\\(_\\) in handle_socket`

### Review

Each pattern below is reviewed against the current source to confirm the rationale is still accurate.

#### `delete match arm Message::Close\\(_\\) in handle_socket`

**Review:** ✅ This suppresses the `delete match arm Message::Close(_)` mutant in `handle_socket`. The documented equivalence is that dropping the `break` causes the WebSocket `reader.next()` to return `None` on the very next iteration, exiting the `while-let` identically. The in-source comment in `router.rs` confirms this. Rationale is sound; no action needed unless the `handle_socket` control-flow changes.

## 8. Per-Crate Coverage Narrative

Brief qualitative notes on what the killed mutants tell us about test coverage quality in each crate.

### `omega-types`

Generated 5 mutants: **4 caught** / **1 missed** / 0 timeout / 0 unviable.  
Kill rate: **80%**.  

The kill rate is good (80–94%). A small number of survivors remain — see Section 2 for details and suggested remediation.
  
Survivor functions: `OmegaEvent::time`.

### `omega-mock-server`

Generated 15 mutants: **2 caught** / **0 missed** / 0 timeout / 13 unviable.  
Kill rate: **100%**.  

The kill rate is excellent (≥ 95%). Test coverage for this crate is strong at the mutation level.

### `omega-cli`

Generated 20 mutants: **13 caught** / **0 missed** / 0 timeout / 7 unviable.  
Kill rate: **100%**.  

The kill rate is excellent (≥ 95%). Test coverage for this crate is strong at the mutation level.

### `omega-test-fixtures`

Generated 31 mutants: **9 caught** / **0 missed** / 0 timeout / 22 unviable.  
Kill rate: **100%**.  

The kill rate is excellent (≥ 95%). Test coverage for this crate is strong at the mutation level.

### `omega-store`

Generated 65 mutants: **39 caught** / **0 missed** / 1 timeout / 25 unviable.  
Kill rate: **98%**.  

The kill rate is excellent (≥ 95%). Test coverage for this crate is strong at the mutation level.

### `omega-core`

Generated 108 mutants: **65 caught** / **0 missed** / 2 timeout / 41 unviable.  
Kill rate: **97%**.  

The kill rate is excellent (≥ 95%). Test coverage for this crate is strong at the mutation level.

### `omega-server`

Generated 110 mutants: **36 caught** / **3 missed** / 0 timeout / 71 unviable.  
Kill rate: **92%**.  

The kill rate is good (80–94%). A small number of survivors remain — see Section 2 for details and suggested remediation.
  
Survivor functions: `PendingChangesIntent::to_json`, `handle_reset`, `handle_resume_session`.

### `omega-agent`

Generated 175 mutants: **60 caught** / **7 missed** / 0 timeout / 108 unviable.  
Kill rate: **90%**.  

The kill rate is good (80–94%). A small number of survivors remain — see Section 2 for details and suggested remediation.
  
Survivor functions: `gen_call_id`, `global_agents_md_path`, `project_turn`.

### `omega-tools`

Generated 275 mutants: **136 caught** / **16 missed** / 4 timeout / 119 unviable.  
Kill rate: **87%**.  

The kill rate is good (80–94%). A small number of survivors remain — see Section 2 for details and suggested remediation.
  
Survivor functions: `cap_and_tee`, `crlf_normalize`, `execute`, `is_utf8_continuation`, `utf8_boundary_backward`, `utf8_boundary_forward`.

## 9. Recommendations

### High priority — add tests to kill 27 surviving mutant(s)

For each survivor in Section 2, write a test that asserts the specific value / side-effect that distinguishes the original from the replacement. Use `cargo mutants -p <crate> --in-place` to confirm the new test kills the mutant before committing.

- **`omega-tools`** — 16 survivor(s):
  - `cap_and_tee` at `crates/omega-tools/src/cap_and_tee.rs:127` → `*`
  - `cap_and_tee` at `crates/omega-tools/src/cap_and_tee.rs:130` → `/`
  - `utf8_boundary_forward` at `crates/omega-tools/src/cap_and_tee.rs:183` → `==`
  - `utf8_boundary_forward` at `crates/omega-tools/src/cap_and_tee.rs:183` → `/`
  - `utf8_boundary_forward` at `crates/omega-tools/src/cap_and_tee.rs:184` → `+=`
  - `utf8_boundary_backward` at `crates/omega-tools/src/cap_and_tee.rs:197` → `==`
  - `utf8_boundary_backward` at `crates/omega-tools/src/cap_and_tee.rs:197` → `>`
  - `utf8_boundary_backward` at `crates/omega-tools/src/cap_and_tee.rs:197` → `<=`
  - `utf8_boundary_backward` at `crates/omega-tools/src/cap_and_tee.rs:198` → `-=`
  - `utf8_boundary_backward` at `crates/omega-tools/src/cap_and_tee.rs:198` → `*=`
  - `is_utf8_continuation` at `crates/omega-tools/src/cap_and_tee.rs:206` → `|`
  - `is_utf8_continuation` at `crates/omega-tools/src/cap_and_tee.rs:206` → `^`
  - `crlf_normalize` at `crates/omega-tools/src/output_cleaner.rs:70` → `<=`
  - `crlf_normalize` at `crates/omega-tools/src/output_cleaner.rs:70` → `*`
  - `execute` at `crates/omega-tools/src/tools/run_command.rs:187` → `true`
  - `execute` at `crates/omega-tools/src/tools/run_command.rs:187` → `false`
- **`omega-agent`** — 7 survivor(s):
  - `gen_call_id` at `crates/omega-agent/src/agent.rs:2089` → `"xyzzy".into()`
  - `project_turn` at `crates/omega-agent/src/session_resume.rs:226` → `true`
  - `project_turn` at `crates/omega-agent/src/session_resume.rs:226` → `false`
  - `project_turn` at `crates/omega-agent/src/session_resume.rs:226` → ``
  - `project_turn` at `crates/omega-agent/src/session_resume.rs:267` → ``
  - `global_agents_md_path` at `crates/omega-agent/src/system_prompt.rs:133` → `None`
  - `global_agents_md_path` at `crates/omega-agent/src/system_prompt.rs:133` → `Some(Default::default())`
- **`omega-server`** — 3 survivor(s):
  - `handle_reset` at `crates/omega-server/src/router.rs:862` → ``
  - `handle_resume_session` at `crates/omega-server/src/router.rs:924` → ``
  - `PendingChangesIntent::to_json` at `crates/omega-server/src/ws_message.rs:110` → `Default::default()`
- **`omega-types`** — 1 survivor(s):
  - `OmegaEvent::time` at `crates/omega-types/src/events.rs:510` → `Box::leak(Box::new(Default::default()))`

### Medium priority — review flagged kills (Section 5)

Each kill flagged in Section 5 should be manually verified. If the kill relies on a test that uses mock infrastructure but the production path is untested, write a complementary test that exercises the real call-site.

### Low priority — verify invariant-based skips (Section 6)

- `pending_continue_ready` in `crates/omega-agent/src/controls.rs:232` — confirm the invariant still holds.

