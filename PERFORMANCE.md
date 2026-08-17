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

## Version 0.7 pre-release verification

Issue #36 adds cached loose Osiris goals, declarations, calls, database
occurrences, and source-backed type evidence. The verification used the same
machine, schema revision, base modules, project, and five-trial method as the
version 0.6 measurement. It used `tree-sitter-bg3` 0.3.0 with parser ABI 15.

The index contained 3,451 documents and 89,139 definitions. Loose Osiris
sources contributed 920 documents and 41,339 definitions compared with the
version 0.6 index.

| Metric | p50 | p95 | Change from version 0.6 p95 |
| --- | ---: | ---: | ---: |
| Cold indexing | 1.334 s | 1.429 s | -88.9% |
| Warm indexing | 426 ms | 427 ms | +14.2% |
| Navigation | 1.125 µs | 1.208 µs | +3.5% |

Additional measurements:

- Warm cache hit rate: 100%
- Repeated-trial RSS high-water mark after ten builds: 1,299,922,944 bytes
- Cache size: 94,220,801 bytes in 3,459 files

The warm p95 result remains within the 15% regression limit while the index
loads 36% more documents. Cold indexing is faster because disposable cache
objects no longer call `fsync` once per source. Cache writes still use
checksums and atomic renames. An interrupted or incomplete write becomes a
cache miss. Parser-context fingerprints are also computed once per source kind
instead of once per file.

## Version 0.11 packaged Thoth verification

Issue #58 adds a cached, read-only catalog for selected Thoth sources in
configured base-module and patch packages. The verification used five cold and
five warm trials on the same machine, data revision, base modules, project, and
source composition as an isolated build of main at `854b541b`. Both builds used
`tree-sitter-bg3` 0.4.3 with parser ABI 15.

The loose index contained 3,450 documents and 89,136 definitions in both
builds. The new virtual catalog contained two packaged sources, 110,015 source
bytes, and one contributing package.

| Metric | Main p50 | Main p95 | Issue #58 p50 | Issue #58 p95 | p95 change |
| --- | ---: | ---: | ---: | ---: | ---: |
| Cold indexing | 1.555 s | 1.624 s | 1.409 s | 1.454 s | -10.5% |
| Warm indexing | 494 ms | 509 ms | 432 ms | 447 ms | -12.2% |
| Navigation | 1.083 µs | 1.125 µs | 1.125 µs | 1.167 µs | +3.7% |

Additional measurements:

- Warm cache hit rate: 100% for both builds
- Issue #58 repeated-trial RSS high-water mark from a separate unrestricted
  run of the same build after ten builds:
  1,342,898,176 bytes
- Main cache: 94,217,988 bytes in 3,458 files
- Issue #58 cache: 94,329,398 bytes in 3,459 files

The isolated main run could not inspect RSS, so it has no directly comparable
memory value. The nearest published repeated-trial measurement was
1,299,922,944 bytes for version 0.7, but its loose source composition differed
by one document and three definitions. The issue #58 value is 3.3% higher than
that contextual measurement. Timing remains within the repository regression
limits, and the packaged catalog adds one cache file and 111,410 bytes.
