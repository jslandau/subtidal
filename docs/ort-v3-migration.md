# ORT and eight-speaker diarization update

The production inference backend remains ONNX Runtime. The separately approved
update uses parakeet-rs 0.4.1 and ORT 2.0.0-rc.14, with Nemotron-3 diarization v3.
Burn is not enabled. The preceding Linux comparison is recorded in
[inference-comparison.md](inference-comparison.md); its original production
baseline and production-isolation checks describe the experiment's historical
state, not this subsequent migration.

## Model and configuration compatibility

The English ASR model is unchanged: its four files retain the existing
`12bca7934f692a845057af06c5861d5e99495318` revision and SHA-256 digests. The
v3 diarizer has an independent revision,
`52f2deee40f20d7dc1f459416e9cd213fb9472df`, remote path
`nemotron-3-diarization/nemotron3_diar_v3.onnx`, SHA-256
`915e4fa23b0192ed9fadeb1cdd26847df986d50c92012d177be28d0343bbe03a`.
It retains the model's OpenMDW 1.1 license. The new filename leaves the old v2.1
file intact; startup continues to verify every required model before use.

Diarization uses the v3 low-latency streaming profile (9-frame chunk,
4-frame right context, FIFO/cache 264, cache update 222), approximately
1.04 seconds of algorithmic context. Omitted `diarization_preset` now selects
v3's default postprocessing. Explicit `callhome`, `dihard3`, and custom
threshold configurations remain supported and are not silently rewritten.
Set `diarization_preset = "default"` to opt an existing explicit preset into
the v3 defaults. Diarization stays opt-in.

The rename dialogs on Linux and macOS expose eight session-local speaker slots.
IDs remain session-relative, not persistent voice identities. Four-speaker
benchmark evidence is not a claim of eight-speaker identification accuracy.

## Runtime and validation

The pinned ORT rc.14 Linux CUDA distribution targets CUDA 13 and cuDNN 9;
CUDA 12 alone does not satisfy its GPU runtime requirements. Its core is linked
statically, but CUDA provider plugins still require matching runtime artifacts;
static core linkage does not imply a fully static installation.

ORT rc.14 requires an explicit environment shared by model sessions. Backend
selection remains Linux CUDA/CPU and macOS WebGPU/CPU; initialization fallback
behavior and caption/sample-coordinate contracts are preserved. The ORT GPU
profiling limitation remains: provider registration and requested inference do
not prove every model node runs on the GPU.

Linux startup stages CUDA/shared provider plugins only from the pinned rc.14
distribution (ORT 1.31.0), never from the newest cache entry or unverified
executable peers. If the exact pair cannot be staged, startup bypasses cached
GPU verdicts and selects CPU. Custom ORT system/library overrides are rejected
at build time because they cannot safely share the prebuilt provider identity.
A copied executable still needs access to the exact cached provider distribution
for GPU acceleration; CPU remains available when that cache is absent.

Linux library/unit, CLI/static/migration/provider tests and all-target checks
passed, as did a locked release build. Real production CPU and CUDA-requested
adapter smokes both produced captions and seven valid diarization segments
using the pinned English model and v3 diarizer. The CUDA smoke requires strict
GPU registration and asserts effective GPU construction for both adapters;
it does not prove GPU node execution. The release binary's CUDA model-load
probe also passed.

Injected tests verify GPU initialization failure retries CPU, both failures
propagate, successful GPU setup avoids CPU construction, and explicit CPU
selection avoids GPU. Pipeline regression tests verify successful fallback
clears a cached positive verdict and disables GPU requests on subsequent
rebuilds. The Linux tray's GPU indicator follows the effective loaded ASR
backend rather than the startup preference. Temporary-directory provider tests
exercise absent source, stale peers, partial replacement failure, and successful
complete-pair staging.

The local macOS cross-check is blocked in `onig_sys`: Linux `cc` rejects Apple's
`-arch` and `-mmacosx-version-min` options. The repository's native Apple Silicon
CI job is the required compilation check. Native bundle launch, WebGPU/CPU
inference, and the eight-row rename interaction have not been exercised on this
Linux host. No macOS native execution or Metal performance claim is made.
