# Frozen AMI comparison fixture

The approved fixture is official AMI meeting **ES2008a**, `Mix-Headset`, starting at 0 seconds. Selection is based only on manual annotations, before model inference. `ES2008a.manifest.json` freezes source provenance, byte lengths, SHA-256 digests, reference policy, conversion command/version, and eligibility checks.

The 0–120s candidate is ineligible: only A, B, D appear in manual transcriber segments (C is absent). The permitted 0–240s extension qualifies:

| Speaker | Scored seconds after DER collars | Raw annotated union seconds |
|---|---:|---:|
| A | 91.246 | 96.541 |
| B | 73.754 | 79.411 |
| C | 10.954 | 12.954 |
| D | 3.759 | 5.509 |

Eligibility uses each speaker's union support after the exact `pyannote.metrics==3.2.1` DER UEM transformation: full `[0, duration]` UEM, `collar=0.25`, `skip_overlap=False`. Collars are centered on **all reference boundaries** (±0.125s), excluding those times from every speaker, not just the speaker whose boundary it is. Overlap remains scored. Raw seconds are diagnostics only. At 120s the scored seconds are A 81.118, B 4.179, D 2.537, and C is absent, so only the permitted 240s extension qualifies.

The manifest received a deliberate annotation-only eligibility metadata correction before any actual inference. It records the prior manifest digest, exact scoring policy, and the fact that no source/audio/reference bytes or selected recording/window changed. The initial missing-binary launch was preparation failure only. Frozen reuse recomputes scored support and validates finite positive duration, interval bounds, and lexical word text in addition to byte identities.

There are 71 changes of the annotated active speaker set. Simultaneous endpoints are processed together; internal silence is an empty set, while silence outside the first/last annotated segment is excluded. Required eligibility: exactly four speakers, each >=2s, >=8 active-set transitions. Overlap is preserved. Duration never extends beyond 240s.

## External assets and reproducibility

Audio, annotation archives, model files, and derived assets are **not committed**. Current prepared assets are at `/tmp/subtidal-inference-ami/` (ephemeral; copy to persistent external storage for future use):

- `ES2008a.Mix-Headset.wav`: original official headset mix
- `annotations.zip`: official manual annotations release 1.6.2
- `fixture.wav`: mono 16kHz PCM16, 240 seconds
- `reference.json`: manual speaker segments and clipped timed lexical words
- `nemotron3_diar_v3.onnx`: independent candidate model, verified separately

Official AMI source: https://groups.inf.ed.ac.uk/ami/download/ (AMI corpus attribution/license CC BY 4.0; consult corpus terms). Individual source URLs are recorded in the manifest. Please cite the AMI Meeting Corpus and preserve source attribution when redistributing derived material.

```sh
# Existing frozen manifest: verify all bytes on every reuse, never regenerate silently.
tools/inference-comparison/python/.venv/bin/python tools/inference-comparison/python/fixture.py \
  --assets /tmp/subtidal-inference-ami \
  --manifest tools/inference-comparison/fixtures/ES2008a.manifest.json
```

For a deliberate new preparation, supply a **new manifest path** and `--download`; preparation checks annotations at 120s first, extends to 240s only on eligibility failure, generates reference/audio, then exclusively creates the manifest before inference. Frozen reuse always checks every source and derived file hash and exact WAV format. Recreating derived bytes with another ffmpeg version may differ: do not replace the committed hashes just to make a reuse check pass. Preserve verified prepared assets or deliberately review a new manifest.

## Isolated Python scoring

Use Python 3.11 and an isolated environment. All dependencies (including transitives) are version-pinned in `requirements.lock`; production dependencies are unaffected. No torch installation is required.

```sh
uv venv --python 3.11 tools/inference-comparison/python/.venv
uv pip sync --python tools/inference-comparison/python/.venv/bin/python \
  tools/inference-comparison/python/requirements.lock
tools/inference-comparison/python/.venv/bin/python -m pytest -q tools/inference-comparison/python
```

`python/integrity.py::verify_models(english_dir, v21_path, v3_path)` verifies every English file against the exact digests in `src/models/mod.rs`, verifies v2.1 independently, and requires v3 SHA-256 `915e4fa23b0192ed9fadeb1cdd26847df986d50c92012d177be28d0343bbe03a` and length 400506656. v3 remote path is `nemotron-3-diarization/nemotron3_diar_v3.onnx` in `altunenes/parakeet-rs` at revision `52f2deee40f20d7dc1f459416e9cd213fb9472df`; never use main or reuse the v2.1 filename. English/v2.1 revision remains `12bca7934f692a845057af06c5861d5e99495318`.

## Scoring contract

Hypothesis JSON:

```json
{"segments": [{"start": 1.0, "end": 2.0, "speaker": "0"}], "text": "chronological transcript"}
```

Times are seconds relative to fixture start, finite and within fixture duration. Segment speaker labels are arbitrary: `pyannote.metrics` performs optimal global speaker permutation. DER uses manual AMI transcriber segment boundaries, **0.25s collar**, and **overlap scored** (`skip_overlap=False`), over the complete fixture interval. Track IDs retain simultaneous segments. DER components are included in score output.

WER uses `jiwer`, is speaker-independent, and does not silently repair speaker attribution. Reference is timed lexical `<w>` elements only, excluding punctuation and nonlexical tags, ordered by `(start, speaker, end)`; intervals crossing the cut are clipped and retained. Normalization: Unicode NFKC, lowercase, curly apostrophe to ASCII, other punctuation to spaces, collapse whitespace. Apostrophes/contractions and filled pauses remain; numbers are not expanded. Overlap words are scored in the deterministic chronological ordering, so WER is not an overlap-aware or speaker-attributed metric.

```sh
tools/inference-comparison/python/.venv/bin/python tools/inference-comparison/python/score.py \
  --reference /tmp/subtidal-inference-ami/reference.json \
  --hypothesis /external/trial/hypothesis.json --output /external/trial/score.json
```

Or import `score(reference_dict, hypothesis_dict)`. `gates(baseline, candidate)` returns DER and WER nonregression booleans with zero tolerance by default; optional explicit tolerances are absolute fractions, not percentage points. Gate results are experimental evidence only, not permission to change production defaults. Overall approval must also require successful execution, verified fixture/models, and the parent experiment's resource/latency gates.

## Subprocess supervision

Import `supervisor.run_trial(command, output_dir, timeout_seconds, sampler_factory=None)`. A sampler factory receives the direct child PID, returns an object exposing `start()` and `finish()`; its finish return value is recorded verbatim in `memory`. This accepts the parent's `MemorySampler`. Each call starts a fresh process/session. Timeout kills the process group and waits for the child. stdout/stderr go to disk, not unbounded in-memory buffers. Existing execution reports cannot be overwritten.

CLI:

```sh
python3 tools/inference-comparison/python/supervisor.py --output /external/trial \
  --timeout 600 -- /path/to/runner --runner-options
```

`execution.json` schema version 1 includes `command`, `status` (`ok`, `nonzero`, `timeout`, `launch_error`), `returncode`, `elapsed_seconds`, `timeout_seconds`, `execution_gate`, `memory`, `error`. Only `ok` passes execution; CLI exits 1 otherwise. A success exit alone does not validate runner output: parent orchestration must validate expected schemas and completion and perform fixture/model checks before launch and scoring. Reference tests cover permutation invariance, overlap missed speech, collar handling, known WER, eligibility, tamper rejection, exact production pins, timeout/nonzero/launch failure, report reuse protection, and memory-sampler attachment.
