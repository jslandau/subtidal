# Linux inference comparison (experimental)

Historical experiment: the operator subsequently approved an ORT library/model
update, documented in [ort-v3-migration.md](ort-v3-migration.md). Statements
below about unchanged production pins describe the comparison execution, not
that later migration. The benchmark runners and original results remain fixed.

This experiment compares production parakeet-rs 0.3.6 / ORT rc.12 (A),
parakeet-rs 0.4.1 / ORT rc.14 with Sortformer v3 (B), and
parakeet-rs 0.4.1 / Burn CUDA or CPU with the same v3 model (C).
It does **not** change Subtidal's production backend, dependencies, model
revision, configuration, speaker UI, or installation instructions.

## Isolation and preparation

`tools/inference-comparison/` is not a Cargo workspace. Its three runner
packages are independent workspaces with separate lockfiles and target trees.
Never combine them into a workspace: incompatible ORT versions share a native
`links` identity. `shared/` is included source, not a dependency workspace.

The observed default compiler is Rust 1.93.1. Burn's resolved dependencies
require Rust 1.95; an isolated `rustup toolchain install 1.95.0 --profile minimal`
was installed without changing the default compiler. `build.py` selects it for
Burn only. Build and test each package with its own target directory:

```sh
python3 tools/inference-comparison/build.py --output /tmp/inference-builds
python3 tools/inference-comparison/check_isolation.py
cargo check --locked --lib
```

The build script runs `cargo test --locked` and `cargo build --release --locked`
with each runner's manifest and distinct `CARGO_TARGET_DIR`. Build logs and
statuses are preserved even when a package fails. `check_isolation.py` checks
independent resolution roots, exact inference versions, an ORT-free Burn graph,
and the unchanged production manifest, lockfile, model pins, and adapters.

Large models, audio, Python environments, and reports belong outside Git.
Inference runners must not download anything. Fixture preparation performs
explicit downloads from the official AMI corpus and records source hashes,
URLs, attribution, annotations, resampling command, and interval selection.

## Comparison semantics

ASR uses exactly the four production English artifacts at HuggingFace
`altunenes/parakeet-rs` revision
`12bca7934f692a845057af06c5861d5e99495318`; compatibility with Burn must be
measured, not assumed. The existing v2.1 model remains unchanged in A.
The independent v3 artifact for B/C is
`nemotron-3-diarization/nemotron3_diar_v3.onnx` at revision
`52f2deee40f20d7dc1f459416e9cd213fb9472df`, SHA-256
`915e4fa23b0192ed9fadeb1cdd26847df986d50c92012d177be28d0343bbe03a`,
400506656 bytes. Retain its OpenMDW 1.1 license and attribution.

Use a single frozen AMI ES2008a headset-mix interval. Select 0–120 seconds
from annotations before inference; require all four speakers to have at least
two scored seconds and at least eight changes of active speaker sets. Extend
only to the continuous 0–240-second stream if needed. If that is still
ineligible, four-speaker validation is incomplete; do not select a better
recording after seeing results. The repository's macOS WebGPU smoke WAV is an
English sanity check, not the four-speaker quality fixture.

Feed unpaced 2560-sample blocks of 16 kHz mono PCM, diarization then ASR in
combined mode. ASR buffers 8960 samples; finish pads a nonempty residual once.
Flush diarization and clip absolute sample ranges to real audio duration.
Warmup is reset before measurement. Report model load and warmup separately;
combined RTF includes processing and finish but excludes load/warmup and uses
real, unpadded duration. First-output compute time is distinct from the audio
position submitted at first output; neither is live perceived latency.

WER uses fixed lowercase/punctuation/whitespace normalization. DER uses optimal
speaker permutation, a 250 ms collar, and scores overlapping speech. Preserve
raw transcript/segments alongside scores. A single meeting cannot establish
broad quality, and this experiment makes no eight-voice identification claim.

## Provider and runtime limitations

ORT wrappers add CPU after CUDA. A strict direct-ORT tiny-graph CUDA registration
preflight plus successful requested inference **does not prove GPU node
execution**. No vendor patch or finalized profiling access is included. Label
ORT trials CUDA-requested, not GPU-proven. Failures are not fallback successes.
Burn CUDA explicitly selects the CUDA backend; upstream per-token decoding is
CPU work, so it is not an all-GPU pipeline. Burn CPU is a separate explicit
trial. A crash, timeout, missing GPU, or incompatible model is a failed trial.

Do not delete existing tuning caches to manufacture a cold result. Without a
verified isolated cache mechanism, label first observed cache state unknown.
Use five fresh-process repetitions reusing caches, and report median and
dispersion rather than best throughput. RSS and GPU memory are sampled per
child; missing NVML process attribution is unknown, not zero. Background GPU
work can influence throughput. `host.json` records the observed hardware and
runtime versions. cuDNN has no pkg-config metadata on this host; its package
version was obtained from the package manager instead.

Stage ORT sidecars only from the corresponding runner build, never from the
newest arbitrary cache. Scope loader paths to each child. Inspect linked
libraries and archive binary/sidecar sizes; do not promise a wholly static Burn
binary. The rc.14 ORT core is statically linked and does not require a separate
`libonnxruntime.so`. No system CUDA/driver changes are authorized or performed.

## Recommendation gates

Compare C/B for backend effects, B/A for model/library migration, and C/A for
overall replacement impact. Do not attribute C/A diarization differences solely
to Burn. Candidate suitability requires C/B WER increase ≤1 absolute percentage
point and DER increase ≤2 points; C/A has the same tolerances. Median warmed
combined C CUDA RTF must be <1 and ≤1.25 times A's. These are exploratory gates,
not model accuracy claims. CPU completion and speaker capacity are separate.
If A CUDA preflight fails or B is unavailable, the comparison is incomplete.
Keep production ORT regardless of experiment outcome until a migration is
separately approved.

## Reproducible execution

Prepare the isolated scoring environment as documented in
[`fixtures/README.md`](../tools/inference-comparison/fixtures/README.md), verify
the frozen manifest, then run (each output directory must be new):

```sh
PY=tools/inference-comparison/python/.venv/bin/python
$PY tools/inference-comparison/run_trials.py \
  --fixture /tmp/subtidal-inference-ami/fixture.wav \
  --fixture-manifest tools/inference-comparison/fixtures/ES2008a.manifest.json \
  --model-dir "$HOME/.local/share/subtidal/models/nemotron" \
  --diar-v2 "$HOME/.local/share/subtidal/models/diarization/diar_streaming_sortformer_4spk-v2.1.onnx" \
  --diar-v3 /tmp/subtidal-inference-ami/nemotron3_diar_v3.onnx \
  --output /tmp/inference-comparison-warmed --repetitions 5 --warmup 1
$PY tools/inference-comparison/summarize.py \
  --trials /tmp/inference-comparison-warmed/trials.json \
  --reference /tmp/subtidal-inference-ami/reference.json \
  --output /tmp/inference-comparison-warmed/summary.json
```

Optional repeated `--runner`, `--provider`, and `--mode` flags restrict the
matrix. `--timeout` defaults to 1200 seconds per child. `--warmup 0` is useful
only for separately labeled first-observed measurements; it cannot pass the
warmed recommendation contract. Runners launch from their own staged install
tree with a scoped loader path; artifacts are hashed and linked-library
inspection is archived by `build.py`. Trial directories cannot be overwritten.
The supervisor preserves stdout/stderr, timeout/crash status, raw outputs and
memory observations. Summaries bind the scoring reference to the verified
fixture provenance and require five requested, successful, unique warmed
combined CUDA trials for each backend before evaluating adoption gates.

Tests:

```sh
python3 -m unittest discover -s tools/inference-comparison -p 'test_*.py' -v
$PY -m pytest -q tools/inference-comparison/python
```

## Execution results

**Recommendation: retain production ORT.** All 45 warmed CUDA-requested trials
completed (five repetitions × three processing modes × three runners). Burn
loaded the exact pinned English export and v3 diarizer without substitutions.
It passes the exploratory quality and realtime gates, but fails the relative
throughput gate: median combined RTF is **2.27× A**, above the allowed 1.25×.
C/B is **5.11×**, isolating the backend comparison with identical model/config.

### Warmed combined results

All metrics below use the frozen continuous 240-second ES2008a stream. WER/DER
are percentages, not fractions. RTF includes finite finish and excludes load
and warmup. These are unpaced compute measurements, not perceived live latency.

| Measurement | A: 0.3.6 / ORT rc.12 / v2.1 | B: 0.4.1 / ORT rc.14 / v3 | C: 0.4.1 / Burn CUDA / v3 |
|---|---:|---:|---:|
| WER | 13.958% | 11.281% | 11.281% |
| DER | 31.522% | 28.049% | 28.044% |
| Median combined RTF | 0.09983 | 0.04433 | 0.22670 |
| RTF min–max | 0.09890–0.10124 | 0.04255–0.04460 | 0.22236–0.34276 |
| RTF population std. dev. | 0.00087 | 0.00095 | 0.04671 |
| Median model load | 2.134 s | 2.954 s | 0.769 s |
| Median full-fixture warmup | 24.228 s | 10.416 s | 67.868 s |
| Median first caption compute | 3.159 s | 1.270 s | 7.372 s |
| First-caption submitted audio position | 33.12 s | 33.12 s | 33.12 s |
| ASR inference-tick p95 | 16.16 ms | 16.74 ms | 160.48 ms |
| Diarization inference-tick p95 | 37.95 ms | 15.30 ms | 57.34 ms |
| Median sampled peak RSS | 1649 MiB | 1428 MiB | 660 MiB |
| Median sampled child GPU allocation | 4554 MiB | 4044 MiB | 3150 MiB |
| Release executable bytes | 28,332,272 | 33,911,720 | 216,403,520 |

The 33.12-second input position reflects silence before the first caption in
this fixture; it is not a 33-second processing delay. C/B WER difference is
0.000 percentage points and DER difference is about −0.006 points. B/A differs
in library version and diarization model, so its apparent quality improvement
must not be attributed to Burn. All three first-observed combined trials also
completed with unknown cache state; they are archived separately and excluded
from warmed medians. C's first-observed RTF was 0.52244, with subsequent cache
reuse faster; no controlled cold-start claim is made.

Burn's lower sampled RSS/GPU allocation and faster model loading do not offset
its throughput and first-caption compute regressions under the approved gates.
The result is a valid negative suitability outcome, not an inference failure.
Keep the existing production pins. If a future migration is considered, first
investigate Burn's ASR decoding/feed costs and repeat on multiple meetings and
hardware. The B control is promising evidence for separately evaluating a
library/model update, not authorization to ship one.

### CPU fallback and capacity

All three explicit CPU runners completed the repository's short English sanity
fixture, producing the same nonempty caption (the leading reference word
“The” was omitted). All nine full 240-second CPU trials completed. Single-run
combined CPU RTF was A 0.6750, B 0.6507, C 1.3313: Burn CPU is feasible but
not realtime in this combined observation. CPU measurements are informational,
with one full-fixture repetition rather than the five CUDA repetitions; they
are not used to claim adoption suitability.
The fixture provides four-speaker scored support after the exact DER collar
(A 91.246 s, B 73.754 s, C 10.954 s, D 3.759 s). Outputs accept IDs 0–7, but
this establishes neither eight simultaneous-speaker quality nor voice identity.

### Evidence and limitations

Compact machine-readable results are committed in
[`result-2026-10-14.json`](../tools/inference-comparison/result-2026-10-14.json).
Full local evidence is durably archived outside Git at
`~/.local/share/subtidal/inference-comparison/2026-10-14/`, including
`inference-comparison-cuda-warmed/summary.json` and the corresponding CPU,
first-observed, smoke, and build directories. The early missing-release-binary
preparation failure is preserved separately and never counted as an inference
success. Earlier compiler-version/command-timeout build attempts were resolved
by the isolated Rust 1.95 toolchain and longer build allowance; final locked
builds succeeded. No inference timeout or crash occurred in the measured matrix.

The machine-readable CUDA summary, raw transcript/segments, per-feed timings,
process memory samples, fixture/model provenance, commands, stdout/stderr, and
binary/sidecar inspections are retained in the experiment archive described
below. No external ORT core appears in either binary's `ldd` output and no ORT
sidecars were emitted by these static-core builds; the archive records this
rather than borrowing libraries from unrelated caches. Burn remains dependent
on system runtime libraries and CUDA driver/toolkit support; it is not claimed
to be a fully static or GPU-only executable.

ORT strict CUDA preflights passed, but actual model GPU node execution remains
unproven under the accepted wrapper/profiling limitation. GPU memory attribution
is sampled by PID, with possible missed transients. CPU matrix work ran alongside
the GPU matrix, and normal desktop GPU use remained present, so host contention
may contribute to dispersion; these are local workflow measurements, not an
exclusive-machine laboratory benchmark. One fixture, fixed order, unknown
cache state, and the lack of ORT node profiling limit generalization. Production
manifest, lockfile, model pins, and adapters remain unchanged.
