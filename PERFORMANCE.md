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

## Version 0.11 installed Thoth fact verification

Issue #54 adds cached fact extraction for declarations, calls, assignments,
returns, and member-access evidence from loose and packaged Thoth sources. The
verification used five cold and five warm trials on the same machine, data
revision, modules, project, and source composition as the Issue #58 run. Both
builds used `tree-sitter-bg3` 0.4.3 with parser ABI 15.

The index contained 3,450 loose documents and 89,136 definitions. The packaged
catalog contained two sources, 110,015 source bytes, and one contributing
package.

| Metric | Issue #54 p50 | Issue #54 p95 | Issue #58 p95 | p95 change |
| --- | ---: | ---: | ---: | ---: |
| Cold indexing | 1.386 s | 1.405 s | 1.454 s | -3.4% |
| Warm indexing | 415 ms | 419 ms | 447 ms | -6.3% |
| Navigation | 1.083 µs | 1.208 µs | 1.167 µs | +3.5% |

Additional measurements:

- Warm cache hit rate: 100%
- Issue #54 repeated-trial RSS high-water mark from a separate unrestricted
  run of the same build after ten builds: 1,304,215,552 bytes
- Issue #54 cache: 94,811,556 bytes in 3,460 files
- Issue #58 cache: 94,329,398 bytes in 3,459 files

The fact cache adds one file and 482,158 bytes compared with Issue #58. Cold
and warm indexing remain below the repository regression limits. The separate
unrestricted run is provided for context; its timing was 1.336/1.339 seconds
cold p50/p95, 411/420 ms warm p50/p95, and 1.083/1.125 microseconds navigation
p50/p95.

## Version 0.11 Thoth type-flow verification

Issue #66 adds structured expression and control-flow facts used for
conservative query-time type propagation. The verification used five cold and
five warm trials with the same machine, data revision, modules, project, and
source composition as the Issue #54 run. Both builds used `tree-sitter-bg3`
0.4.3 with parser ABI 15.

| Metric | Issue #66 p50 | Issue #66 p95 | Issue #54 p95 | p95 change |
| --- | ---: | ---: | ---: | ---: |
| Cold indexing | 1.452 s | 1.488 s | 1.405 s | +5.9% |
| Warm indexing | 427 ms | 432 ms | 419 ms | +3.1% |
| Navigation | 1.125 µs | 1.208 µs | 1.208 µs | 0.0% |

Additional measurements:

- Warm cache hit rate: 100%
- Repeated-trial RSS high-water mark after ten builds: 1,362,984,960 bytes
- Cache size: 95,563,422 bytes in 3,460 files

The structured facts add 751,866 bytes without adding cache files. Cold and
warm p95 remain within the repository regression limits. Query-time type flow
does not change the exact navigation benchmark result.

## Version 0.14 packaged Stats verification

Issue #83 adds the configured base-module packaged Stats catalog. The
verification used five cold and five warm trials with the same machine, data
revision, modules, project, and source composition as the Issue #66 run. Both
builds used `tree-sitter-bg3` 0.5.0 with parser ABI 15.

| Metric | Issue #83 p50 | Issue #83 p95 | Issue #66 p95 | p95 change |
| --- | ---: | ---: | ---: | ---: |
| Cold indexing | 1.661 s | 1.786 s | 1.488 s | +20.0% |
| Warm indexing | 387 ms | 391 ms | 432 ms | -9.5% |
| Navigation | 875 ns | 958 ns | 1.208 µs | -20.7% |

Additional measurements:

- Warm cache hit rate: 100%
- Packaged Stats declarations indexed: 11,382 from 75 package sources
- Repeated-trial RSS high-water mark after ten builds: 1,343,193,088 bytes
- Cache size: 95,806,396 bytes in 3,324 files

Cold indexing includes the first parallel parse of every packaged Stats
entry, which costs about 300 milliseconds once per empty cache and reaches
the documented cold regression boundary. The increase is accepted deliberately:
issue #83 requests this source family, and the finished catalog persists in
its own content-addressed cache entry, so later builds reuse it and warm
indexing improves.

## Issue #144 Osiris variable contract verification

Issue #144 adds generated, versioned contracts for Osiris engine calls and
queries. The comparison used five cold and five warm trials on the same
machine, data revision, configured modules, project, and source composition.
Both builds used `tree-sitter-bg3` 0.5.3 with parser ABI 15. The index
contained 3,451 documents and 89,133 definitions.

| Metric | Main p50 | Main p95 | Issue #144 p50 | Issue #144 p95 | p95 change |
| --- | ---: | ---: | ---: | ---: | ---: |
| Cold indexing | 4,639 ms | 4,645 ms | 4,778 ms | 4,833 ms | +4.0% |
| Warm indexing | 900 ms | 915 ms | 907 ms | 922 ms | +0.8% |
| Navigation | 1,291 ns | 1,292 ns | 1,209 ns | 1,250 ns | -3.3% |

Additional measurements:

- Warm cache hit rate: 100% for both builds
- Main unrestricted-run RSS: 2,110,537,728 bytes
- Issue #144 unrestricted-run RSS: 2,123,726,848 bytes (+0.6%)
- Main cache: 167,641,265 bytes in 3,463 files
- Issue #144 cache: 168,318,479 bytes in 3,463 files (+0.4%)

Both builds used schema revision
`ab8dfe38b6547cba2bbf3897774f5437919193d4237892e4cc3c5306bb46177d`.
The cold p95 increase remains below the 20% limit, and the warm p95 increase
remains below the 15% limit. Navigation also improves, so the issue passes the
repository regression thresholds.
