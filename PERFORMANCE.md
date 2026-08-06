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

## Version 0.4 pre-release verification

Issue #26 adds the cached English base localization catalog. The verification
used the same machine, data revision, modules, grammar, and five-trial method as
the baseline.

| Metric | p50 | p95 | Change from baseline p95 |
| --- | ---: | ---: | ---: |
| Cold indexing | 12.663 s | 12.712 s | -9.8% |
| Warm indexing | 378 ms | 379 ms | +12.5% |
| Navigation | 1.125 µs | 1.209 µs | +3.6% |

Additional measurements:

- Warm cache hit rate: 100%
- Repeated-trial RSS high-water mark after ten builds: 906,838,016 bytes
- Cache size: 69,358,463 bytes in 2,538 files
- Indexed localization handles: 232,878

The warm and cold p95 results remain within the repository regression limits.
The cache and repeated-trial RSS increase because the server now retains the
configured base localization catalog. Normal operation keeps one published
catalog; the repeated-trial RSS value also includes allocator pages from ten
rebuilds.

## Version 0.6 pre-release verification

Issue #32 adds cached Thoth helper declarations and call references. The
verification used the same machine, schema revision, base modules, project,
and five-trial method as the version 0.5 measurement. It used
`tree-sitter-bg3` 0.2.0 with parser ABI 15.

The version 0.5 comparison values were 13.208 seconds cold p95, 377 ms warm
p95, and 1.083 microseconds navigation p95.

The index contained 2,531 documents and 47,800 definitions. The added Thoth
source contributed one document and 77 declarations.

| Metric | p50 | p95 | Change from version 0.5 p95 |
| --- | ---: | ---: | ---: |
| Cold indexing | 12.731 s | 12.844 s | -2.8% |
| Warm indexing | 369 ms | 374 ms | -0.8% |
| Navigation | 1.125 µs | 1.167 µs | +7.8% |

Additional measurements:

- Warm cache hit rate: 100%
- Repeated-trial RSS high-water mark after ten builds: 918,667,264 bytes
- Cache size: 71,119,548 bytes in 2,539 files

The cold and warm p95 results remain within the repository regression limits.
The cache stores helper parameter lists and call ranges so warm workspaces do
not parse unchanged `.khn` files again.
