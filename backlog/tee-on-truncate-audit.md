# Omega tool-output audit

Two independent analyses follow. They are kept fully separate so that 
optimisation decisions for one scope are not contaminated by the other. 
The local scope is Omega working on its own repo (high risk of 
overfitting). The benchmark scope is Terminal-Bench 2.0 trials, which 
is the better signal for generalisation.

---

# Analysis 1 — Local interactive sessions

## Scope: local interactive sessions (.omega/sessions)

- Sessions scanned: **842**
- Total tool_results: **28155**
- Total output bytes: **85.6M** (~22,442,681 tokens)
- Truncation hits: **1073**

### Per-tool totals

| tool | calls | bytes | ~tokens | p50 | p95 | max | avg |
|---|---:|---:|---:|---:|---:|---:|---:|
| read_file | 6656 | 37.8M | 9,896,472 | 2.3K | 27.3K | 146.2K | 5.8K |
| run_command | 9189 | 24.7M | 6,466,024 | 405B | 8.0K | 97.8K | 2.7K |
| grep_files | 4939 | 14.2M | 3,717,601 | 1.1K | 12.7K | 97.8K | 2.9K |
| wait_for_output | 178 | 3.3M | 854,667 | 457B | 80.8K | 1.1M | 18.8K |
| fetch_url | 490 | 2.4M | 629,482 | 1.4K | 19.6K | 19.6K | 5.0K |
| list_files | 741 | 2.2M | 575,468 | 131B | 10.2K | 69.0K | 3.0K |
| web_search | 170 | 517.2K | 132,394 | 3.5K | 5.4K | 6.1K | 3.0K |
| edit_file | 4033 | 361.1K | 92,452 | 73B | 187B | 1017B | 91B |
| find_files | 758 | 241.2K | 61,753 | 25B | 1.2K | 13.6K | 325B |
| write_file | 504 | 33.9K | 8,666 | 66B | 98B | 125B | 68B |
| run_background | 305 | 19.7K | 5,048 | 69B | 70B | 71B | 66B |
| wait_process | 182 | 10.1K | 2,591 | 58B | 58B | 75B | 56B |
| write_stdin | 3 | 160B | 40 | 61B | 61B | 61B | 53B |
| kill_process | 7 | 77B | 19 | 0B | 26B | 26B | 11B |

### run_command by program (argv[0])

| program | calls | bytes | ~tokens | p50 | p95 | max |
|---|---:|---:|---:|---:|---:|---:|
| `git add` | 698 | 8.6M | 2,265,531 | 396B | 57.1K | 97.8K |
| `just gate` | 199 | 2.3M | 602,283 | 1.1K | 51.1K | 70.6K |
| `grep` | 1032 | 1.7M | 455,293 | 339B | 3.6K | 97.8K |
| `cat` | 556 | 1.7M | 437,415 | 673B | 8.6K | 97.8K |
| `cargo test` | 732 | 1.0M | 262,468 | 713B | 3.1K | 97.8K |
| `git commit` | 81 | 756.0K | 193,533 | 370B | 50.0K | 51.5K |
| `git diff` | 163 | 535.2K | 137,007 | 633B | 13.2K | 60.8K |
| `#` | 216 | 502.7K | 128,685 | 270B | 3.0K | 97.8K |
| `find` | 233 | 453.4K | 116,076 | 275B | 3.8K | 97.8K |
| `python3` | 231 | 442.0K | 113,161 | 375B | 6.0K | 97.6K |
| `sed` | 258 | 437.7K | 112,046 | 1.0K | 6.1K | 14.9K |
| `just test-fast` | 40 | 437.3K | 111,959 | 2.1K | 48.0K | 48.2K |
| `just gate;` | 7 | 362.8K | 92,883 | 52.8K | 53.0K | 53.0K |
| `just rust-gate` | 207 | 333.9K | 85,471 | 796B | 3.0K | 43.3K |
| `git show` | 161 | 309.2K | 79,154 | 1.0K | 5.1K | 27.4K |
| `bun test` | 151 | 291.5K | 74,635 | 1.3K | 4.0K | 46.5K |
| `git status` | 362 | 237.3K | 60,749 | 227B | 1.5K | 55.9K |
| `ls` | 353 | 211.3K | 54,105 | 197B | 1.8K | 19.5K |
| `bun run` | 19 | 205.9K | 52,710 | 32B | 80.6K | 97.8K |
| `just rust-gate;` | 4 | 200.7K | 51,374 | 54.0K | 54.0K | 54.0K |
| `echo` | 83 | 190.3K | 48,713 | 578B | 8.3K | 11.3K |
| `git log` | 231 | 189.8K | 48,585 | 624B | 2.2K | 5.7K |
| `just test` | 48 | 181.7K | 46,522 | 394B | 4.6K | 70.6K |
| `cargo fmt` | 141 | 176.2K | 45,103 | 239B | 4.0K | 35.7K |
| `just e2e` | 47 | 162.5K | 41,597 | 934B | 2.5K | 97.8K |
| `bunx tsc` | 50 | 138.8K | 35,544 | 278B | 7.2K | 62.9K |
| `for` | 67 | 134.4K | 34,399 | 920B | 6.0K | 29.7K |
| `head` | 61 | 132.5K | 33,916 | 1.3K | 5.2K | 21.5K |
| `cargo mutants` | 171 | 131.9K | 33,758 | 264B | 2.9K | 21.6K |
| `tail` | 56 | 125.3K | 32,084 | 694B | 6.9K | 40.9K |

### Symptom classes

| symptom | count | bytes | ~tokens |
|---|---:|---:|---:|
| truncated | 1073 | 16.4M | 4,306,061 |
| ansi/progress | 527 | 12.2M | 3,206,040 |
| big-dump | 455 | 14.2M | 3,733,740 |
| test-enum | 101 | 720.3K | 184,387 |
| repetition | 83 | 700.1K | 179,229 |
| compile-spam | 50 | 187.2K | 47,922 |
| git-progress | 1 | 21.0K | 5,367 |

### Top 25 largest individual outputs

| bytes | tool | preview |
|---:|---|---|
| 1.1M | wait_for_output | `{"exitCode":4,"matched":false,"minBytesReached":false,"output":"Found 271 mutants to test\nFAILED   Unmutated ` |
| 623.0K | wait_for_output | `{"exitCode":4,"matched":false,"minBytesReached":false,"output":"Found 771 mutants to test\nFAILED   Unmutated ` |
| 573.7K | wait_for_output | `{"matched":true,"minBytesReached":false,"output":"Found 688 mutants to test\nFAILED   Unmutated baseline in 42` |
| 146.2K | read_file | `by `SessionRenamed` / `SessionDeleted` envelopes. Folding would force\neither reducer to ignore most of its ow` |
| 107.3K | read_file | `# Omega — Rust Migration\n\n*Living document. Completed phases are summarised briefly; upcoming phases have fu` |
| 107.2K | read_file | `# Omega — Rust Migration\n\n*Living document. Completed phases are summarised briefly; upcoming phases have fu` |
| 107.2K | read_file | `# Omega — Rust Migration\n\n*Living document. Completed phases are summarised briefly; upcoming phases have fu` |
| 107.2K | read_file | `# Omega — Rust Migration\n\n*Living document. Completed phases are summarised briefly; upcoming phases have fu` |
| 106.9K | read_file | `# Omega — Rust Migration\n\n*Living document. Completed phases are summarised briefly; upcoming phases have fu` |
| 106.9K | read_file | `# Omega — Rust Migration\n\n*Living document. Completed phases are summarised briefly; upcoming phases have fu` |
| 106.9K | read_file | `# Omega — Rust Migration\n\n*Living document. Completed phases are summarised briefly; upcoming phases have fu` |
| 106.8K | read_file | `# Omega — Rust Migration\n\n*Living document. Completed phases are summarised briefly; upcoming phases have fu` |
| 105.8K | read_file | `# Omega — Rust Migration\n\n*Living document. Completed phases are summarised briefly; upcoming phases have fu` |
| 105.3K | read_file | `# Omega — Rust Migration\n\n*Living document. Completed phases are summarised briefly; upcoming phases have fu` |
| 102.5K | read_file | `\n### Co-existence strategy — "don't brick Omega before cutover"\n\nThe SolidJS UI stays the production fronte` |
| 97.8K | run_command | `npx playwright test e2e/session-picker.spec.ts --grep metadata --reporter=list\n\nRunning 1 test using 1 worke` |
| 97.8K | run_command | `8:import{createRequire as RRq}from"node:module";var ERq=Object.create;var{getPrototypeOf:LRq,defineProperty:PN` |
| 97.8K | run_command | `./.omega/sessions/2026-03-14T18-39-32-644-8a488858/events.jsonl\n./.omega/sessions/2026-03-14T18-39-32-644-8a4` |
| 97.8K | read_file | `{"type":"llm_call","time":"2026-05-02T22:53:15.457Z","url":"https://api.anthropic.com/v1/messages","model":"cl` |
| 97.8K | run_command | `thread 'run_command_large_stdout_shows_truncation_notice' (1159859) panicked at crates/omega-tools/tests/proce` |
| 97.8K | run_command | `test run_command_timeout_kills_process ... ok\ntest run_command_large_stdout_shows_truncation_notice ... FAILE` |
| 97.8K | wait_for_output | `{"output":"Found 172 mutants to test\nFAILED   Unmutated baseline in 19s build + 2s test\n\n*** baseline\n\n**` |
| 97.8K | run_command | `# dependencies (bun install)\nnode_modules\n\n# output\nout\ndist\n*.tgz\n\n# web UI build output (run `bun ru` |
| 97.8K | grep_files | `.omega/sessions/2026-04-20T06-18-05-569-f18c58ac/events.jsonl:2:{"type":"session_started","time":"2026-04-20T0` |
| 97.8K | grep_files | `.omega/sessions/2026-04-19T16-35-36-050-53d0740c/events.jsonl:6:{"type":"llm_call","time":"2026-04-19T16:36:09` |

### Top 10 sessions by total tool-output bytes

| bytes | ~tokens | session |
|---:|---:|---|
| 2.3M | 610,552 | `sessions/2026-03-28T20-31-52-099-2774f94a` |
| 1.9M | 498,642 | `sessions/2026-05-06T05-52-05-975-10f262ff` |
| 1.7M | 435,179 | `sessions/2026-05-07T08-23-09-153-b5a2952b` |
| 1.4M | 356,010 | `sessions/2026-05-11T05-06-28-850-2484eb0b` |
| 879.2K | 225,077 | `sessions/2026-05-12T21-47-55-249-8ef34940` |
| 851.5K | 217,974 | `sessions/2026-03-14T20-23-23-163-95866848` |
| 757.5K | 193,919 | `sessions/2026-04-05T17-37-23-876-4b2083fa` |
| 736.7K | 188,594 | `sessions/2026-04-03T09-16-11-146-8e79b27c` |
| 702.3K | 179,799 | `sessions/2026-05-05T20-52-56-077-ad3a74d2` |
| 668.9K | 171,251 | `sessions/2026-05-02T21-22-26-515-8d9348c4` |

### Unused-output heuristic (outputs ≥ 2 KB)

- Eligible: 77.0M (~20,174,218 tokens)
- Apparently unused (no 24-char window quoted in next assistant turn): **74.7M** (~19,589,941 tokens, 97%)
- Cheap heuristic; the model often uses content without quoting it verbatim.


---

# Analysis 2 — Benchmark trials (Terminal-Bench 2.0)

## Scope: benchmark trials (bench/jobs)

- Sessions scanned: **305**
- Total tool_results: **6824**
- Total output bytes: **10.9M** (~2,870,043 tokens)
- Truncation hits: **55**

### Per-tool totals

| tool | calls | bytes | ~tokens | p50 | p95 | max | avg |
|---|---:|---:|---:|---:|---:|---:|---:|
| run_command | 4977 | 5.9M | 1,534,224 | 256B | 3.9K | 97.8K | 1.2K |
| wait_for_output | 111 | 2.2M | 574,227 | 1.5K | 97.8K | 97.8K | 20.2K |
| read_file | 459 | 1.9M | 506,492 | 1.7K | 14.9K | 55.0K | 4.3K |
| grep_files | 136 | 600.0K | 153,590 | 17B | 12.0K | 97.8K | 4.4K |
| fetch_url | 227 | 240.2K | 61,479 | 337B | 8.1K | 8.4K | 1.1K |
| list_files | 137 | 89.3K | 22,858 | 42B | 811B | 63.2K | 667B |
| write_file | 444 | 22.5K | 5,752 | 51B | 67B | 83B | 51B |
| edit_file | 172 | 18.5K | 4,743 | 79B | 234B | 683B | 110B |
| find_files | 35 | 18.0K | 4,616 | 15B | 842B | 14.4K | 527B |
| run_background | 72 | 4.7K | 1,215 | 68B | 69B | 69B | 67B |
| web_search | 31 | 2.5K | 643 | 83B | 83B | 83B | 83B |
| write_stdin | 23 | 808B | 202 | 35B | 36B | 36B | 35B |

### run_command by program (argv[0])

| program | calls | bytes | ~tokens | p50 | p95 | max |
|---|---:|---:|---:|---:|---:|---:|
| `cat` | 320 | 818.4K | 209,498 | 657B | 8.9K | 91.7K |
| `python3` | 623 | 616.2K | 157,758 | 356B | 3.6K | 31.8K |
| `python3 <<` | 215 | 409.5K | 104,830 | 508B | 7.2K | 40.5K |
| `grep` | 211 | 326.6K | 83,609 | 372B | 4.7K | 48.9K |
| `ls` | 386 | 253.2K | 64,826 | 146B | 1.8K | 97.8K |
| `timeout` | 39 | 238.1K | 60,942 | 687B | 8.3K | 97.8K |
| `for` | 88 | 231.8K | 59,339 | 575B | 16.9K | 26.6K |
| `find` | 157 | 174.0K | 44,554 | 95B | 2.7K | 97.8K |
| `objdump` | 47 | 141.7K | 36,271 | 1.5K | 9.9K | 20.1K |
| `#` | 235 | 107.7K | 27,583 | 200B | 1.3K | 9.6K |
| `pnmtopng` | 2 | 98.1K | 25,106 | 296B | 97.8K | 97.8K |
| `python3 read_png.py` | 1 | 97.8K | 25,032 | 97.8K | 97.8K | 97.8K |
| `pip install` | 111 | 88.3K | 22,607 | 413B | 2.1K | 12.5K |
| `python3 analyze2.py` | 1 | 76.2K | 19,496 | 76.2K | 76.2K | 76.2K |
| `python3 front_view.py` | 1 | 63.3K | 16,205 | 63.3K | 63.3K | 63.3K |
| `rm` | 35 | 62.7K | 16,039 | 203B | 3.3K | 37.8K |
| `python3 parse_gcode4.py` | 1 | 61.7K | 15,783 | 61.7K | 61.7K | 61.7K |
| `bun parse_gcode4.ts` | 1 | 57.2K | 14,644 | 57.2K | 57.2K | 57.2K |
| `R` | 44 | 52.0K | 13,321 | 341B | 2.4K | 25.9K |
| `od` | 37 | 51.9K | 13,274 | 704B | 4.4K | 9.4K |
| `python3 render_side.py` | 1 | 50.2K | 12,848 | 50.2K | 50.2K | 50.2K |
| `apt-get` | 140 | 49.3K | 12,629 | 258B | 1020B | 1.3K |
| `echo` | 86 | 48.3K | 12,375 | 171B | 1.2K | 21.6K |
| `make` | 24 | 48.0K | 12,288 | 1.4K | 7.0K | 10.3K |
| `git diff` | 4 | 47.7K | 12,203 | 2.6K | 44.7K | 44.7K |
| `head` | 68 | 47.6K | 12,174 | 360B | 2.6K | 6.6K |
| `python` | 113 | 42.6K | 10,911 | 185B | 1.1K | 5.6K |
| `python3 /app/ascii_render.py` | 1 | 39.3K | 10,060 | 39.3K | 39.3K | 39.3K |
| `readelf` | 17 | 34.5K | 8,820 | 2.3K | 4.6K | 5.1K |
| `bun parse_gcode8.ts` | 1 | 34.1K | 8,721 | 34.1K | 34.1K | 34.1K |

### Symptom classes

| symptom | count | bytes | ~tokens |
|---|---:|---:|---:|
| ansi/progress | 147 | 371.1K | 95,007 |
| truncated | 55 | 2.9M | 758,346 |
| repetition | 32 | 341.3K | 87,362 |
| big-dump | 29 | 894.2K | 228,921 |
| compile-spam | 2 | 4.4K | 1,137 |

### Top 25 largest individual outputs

| bytes | tool | preview |
|---:|---|---|
| 97.8K | wait_for_output | `{"output":"\rRead 1M words\rRead 2M words\rRead 3M words\rRead 4M words\rRead 5M words\rRead 6M words\rRead 7M` |
| 97.8K | wait_for_output | `{"output":"\rRead 1M words\rRead 2M words\rRead 3M words\rRead 4M words\rRead 5M words\rRead 6M words\rRead 7M` |
| 97.8K | wait_for_output | `{"output":"\rRead 1M words\rRead 2M words\rRead 3M words\rRead 4M words\rRead 5M words\rRead 6M words\rRead 7M` |
| 97.8K | wait_for_output | `{"output":"\rRead 1M words\rRead 2M words\rRead 3M words\rRead 4M words\rRead 5M words\rRead 6M words\rRead 7M` |
| 97.8K | wait_for_output | `{"output":"\rRead 1M words\rRead 2M words\rRead 3M words\rRead 4M words\rRead 5M words\rRead 6M words\rRead 7M` |
| 97.8K | wait_for_output | `{"output":"\rRead 1M words\rRead 2M words\rRead 3M words\rRead 4M words\rRead 5M words\rRead 6M words\rRead 7M` |
| 97.8K | wait_for_output | `{"output":"\rRead 1M words\rRead 2M words\rRead 3M words\rRead 4M words\rRead 5M words\rRead 6M words\rRead 7M` |
| 97.8K | wait_for_output | `{"output":"\rRead 1M words\rRead 2M words\rRead 3M words\rRead 4M words\rRead 5M words\rRead 6M words\rRead 7M` |
| 97.8K | wait_for_output | `{"output":"\rRead 1M words\rRead 2M words\rRead 3M words\rRead 4M words\rRead 5M words\rRead 6M words\rRead 7M` |
| 97.8K | wait_for_output | `{"output":"\rRead 1M words\rRead 2M words\rRead 3M words\rRead 4M words\rRead 5M words\rRead 6M words\rRead 7M` |
| 97.8K | grep_files | `/app/dclm/setup.py:112:            #"model.bin": "https://wmtis.s3.eu-west-1.amazonaws.com/quality_prediction_` |
| 97.8K | grep_files | `1:[{"uuid":"RO9XQ","name":"10B","slug":"10b","seq":"MSKGEELFTGVVPILVELDGDVNGHKFSVSGEGEGDATYGKLTLKFICTTGKLPVPWP` |
| 97.8K | grep_files | `1:[{"uuid":"RO9XQ","name":"10B","slug":"10b","seq":"MSKGEELFTGVVPILVELDGDVNGHKFSVSGEGEGDATYGKLTLKFICTTGKLPVPWP` |
| 97.8K | grep_files | `1:[{"uuid":"RO9XQ","name":"10B","slug":"10b","seq":"MSKGEELFTGVVPILVELDGDVNGHKFSVSGEGEGDATYGKLTLKFICTTGKLPVPWP` |
| 97.8K | wait_for_output | `{"output":"MassiveIntentClassification: Missing subsets {'sv', 'nb', ...} for split test\nMassiveIntentClassif` |
| 97.8K | wait_for_output | `{"output":"AmazonReviewsClassification: Missing subsets {'de', 'fr', ...} for split test\nAmazonReviewsClassif` |
| 97.8K | run_command | `Loading ELF...\nELF loaded. Entry = 0x00400110\nStarting MIPS interpreter...\n[write#1 fd=1 pc=0x0043adf4 coun` |
| 97.8K | run_command | `Loading ELF...\nELF loaded. Entry = 0x00400110\nStarting MIPS interpreter...\nDoomGeneric initialized. Frames ` |
| 97.8K | run_command | `-rw-r--r-- 1 root root 130 Apr 26 16:00 /tmp/screen2.png\nP6\n256 200\n255\n���                               ` |
| 97.8K | run_command | `Image: 6989x600\nContent rows: 54 to 544\n\nRow density stats:\n  Max: 596\n  Dense rows (>50): [59 60 61 62 6` |
| 97.8K | run_command | `-rw-r--r-- 1 root root      880 Oct 31 02:51 /app/john/.ci/Dockerfile\n-rw-r--r-- 1 root root     5769 Oct 31 ` |
| 97.8K | wait_for_output | `{"output":"make proof\nmake[1]: Entering directory '/tmp/CompCert-3.13.1'\nCOQC flocq/Core/Ulp.v\nCOQC flocq/C` |
| 97.8K | wait_for_output | `{"output":"make proof\nmake[1]: Entering directory '/tmp/CompCert-3.13.1'\nCOQC flocq/Core/Ulp.v\nCOQC flocq/C` |
| 97.8K | wait_for_output | `{"output":"make proof\nmake[1]: Entering directory '/tmp/CompCert'\nCOQC flocq/Core/Round_NE.v\nCOQC flocq/Cal` |
| 97.8K | wait_for_output | `{"output":"make proof\nmake[1]: Entering directory '/tmp/CompCert'\nCOQC flocq/Core/Round_NE.v\nCOQC flocq/Cal` |

### Top 10 sessions by total tool-output bytes

| bytes | ~tokens | session |
|---:|---:|---|
| 658.9K | 168,667 | `train-fasttext__s5bik5s/agent` |
| 596.5K | 152,703 | `train-fasttext__hnNAh3p/agent` |
| 467.9K | 119,783 | `gcode-to-text__vM2NGFq/agent` |
| 345.2K | 88,363 | `protein-assembly__oL2oMZW/agent` |
| 344.2K | 88,117 | `gcode-to-text__yzcvHxu/agent` |
| 322.6K | 82,589 | `gcode-to-text__UoofxP4/agent` |
| 304.4K | 77,927 | `sanitize-git-repo__dxiW6BL/agent` |
| 262.7K | 67,247 | `mteb-leaderboard__teLbZUB/agent` |
| 257.9K | 66,023 | `make-mips-interpreter__7Ksc4Qn/agent` |
| 255.6K | 65,421 | `compile-compcert__Si3CCCh/agent` |

### Unused-output heuristic (outputs ≥ 2 KB)

- Eligible: 9.0M (~2,351,313 tokens)
- Apparently unused (no 24-char window quoted in next assistant turn): **9.0M** (~2,351,313 tokens, 100%)
- Cheap heuristic; the model often uses content without quoting it verbatim.
