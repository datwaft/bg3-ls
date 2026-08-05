# Performance baseline

The release baseline uses five cold and five warm trials against the installed
BG3 Toolkit data. The benchmark records the schema digest, grammar version, and
parser ABI so results from different data or parser revisions are not compared.

Baseline date: 2026-08-05

Environment:

- macOS 26.5.2 on ARM64
- Rust 1.97.1, optimized `--release` build
- Neovim 0.12.4 for protocol verification
- BG3 schema revision
  `ab8dfe38b6547cba2bbf3897774f5437919193d4237892e4cc3c5306bb46177d`
- tree-sitter-bg3 0.1.0, parser ABI 15
- 2,530 indexed documents and 47,723 definitions

| Metric | p50 | p95 |
| --- | ---: | ---: |
| Cold indexing | 13.622 s | 14.098 s |
| Warm indexing | 337 ms | 337 ms |
| Navigation | 1.125 µs | 1.167 µs |

Additional measurements:

- Warm cache hit rate: 100%
- Observed resident memory after one cold and one warm build: 496,123,904 bytes
- Repeated-trial RSS high-water mark after ten builds: 709,623,808 bytes
- Cache size: 43,010,720 bytes in 2,537 files

The RSS values are post-build observations from `ps`. The repeated-trial value
includes allocator pages retained across all ten builds. It is a reload stress
measurement, not the normal first-start working set.

On the same machine and data revision, a later release must not regress warm
p95 by more than 15% or cold p95 by more than 20%.
