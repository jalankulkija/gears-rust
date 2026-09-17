# onnx-runner

Generic ONNX model execution gear: model lifecycle, session hosting, and HuggingFace
tokenization. Tensors in, tensors out — it knows nothing about model families.

Inference runs off the async runtime, every call is bounded in size, time and
concurrency, a failed model artifact is unavailable rather than fatal, and the replica
pool's sizing is set from measurement. See [docs/DESIGN.md](../docs/DESIGN.md) for the
architecture and the decisions behind it.

Comments here credit measurements and terminology to **dlp-bench**
(<https://github.com/MikeFalcon77/dlp-bench>, MIT), a public Rust benchmark for local
PII-detection models. No code is taken from it in this crate.

## What it does

| | |
|---|---|
| Loads | ONNX models configured by an operator, verified by SHA-256 and graph contract |
| Serves | `run(model_id, tensors)`, `encode(model_id, text)`, `encode_fragment`, `list_models` |
| Hosts | N session replicas per model, dispatched round-robin |
| Never does | model-family decoding, model fetching in production, caller-supplied paths, or serving HTTP |

Prompt construction, output decoding, and label sets belong to consumers — today
`onnx-scanner` in `prompt-scanner`.

There is **no HTTP API**: this is an in-process gear reached through `ClientHub`, like
`authn-resolver` or `tenant-resolver`. Operator readiness comes from the platform
healthcheck aggregated into `/health` and `/readyz`.

## Using it

Depend on both crates: `onnx-runner-sdk` for the trait and types, `onnx-runner` so
`#[toolkit::gear(deps = [onnx_runner])]` resolves at compile time.

```rust
let runner = client_hub.get::<dyn OnnxRunnerClientV1>()?;
let encoding = runner.encode(ctx, "my-model", text).await?;
let outputs = runner.run(ctx, "my-model", inputs).await?;
```

Models are addressed by id and configured at startup; a caller never supplies a path.

## Configuration

A full operator-facing example covering every knob — the bounds, the deadline, and the
overload policy that this section leaves out — is maintained alongside the design docs at
[`docs/config-example.yaml`](../docs/config-example.yaml). The short version:

```yaml
gears:
  onnx-runner:
    config:
      models:
        - model_id: "my-model"
          model_dir: "./models/my-model"
          model_file: "model.onnx"          # default
          tokenizer_file: "tokenizer.json"  # default
          sha256: "<hex>"
          labels_file: "config.json"        # default; optional id2label sidecar
          expected_inputs: ["input_ids", "attention_mask"]
          expected_outputs: ["logits"]
          output_dtypes:
            logits: f32
          replicas: 4                       # default
          intra_threads: 1                  # default
```

`replicas × intra_threads` is the thread budget and `replicas × model size` the memory
cost. Both matter, and the trade between them is measured rather than assumed:

| Deployment | `replicas` | `intra_threads` |
|---|---|---|
| Concurrent load, server, large model | 12 | 4 |
| Concurrent load, server, small model | 24 | 2 |
| Concurrent load, laptop-class | 4 | 1 |
| Serial / single caller | 1 | all available cores |

The split is by *concurrency*, not by input size. A lone caller cannot keep several
replicas busy, so threads win; at four or more concurrent callers replicas win, and by a
wide margin — at a fixed 48-thread budget, 24 thin replicas sustain roughly four times the
throughput of one 48-thread session. Input size moves the margin, never the winner.

Do not simply maximize replicas: 24 buys 23% more throughput than 6 for 3.6x the memory,
and is slower at long prompts.

## Models

Model artifacts are not in version control. Fetch the pinned set for local work:

```sh
make onnx-models-fetch     # or: bash gears/onnx-runner/scripts/fetch-models.sh
```

Production operators place verified files themselves; the gear never fetches anything.

## Runtime requirement

The ONNX Runtime shared library is resolved at process start from `ORT_DYLIB_PATH`, not
linked at build time — the workspace pins `ort` with its `load-dynamic` feature. The
library must be **1.24 or newer**, matching the `api-24` ABI that pin requires.

```sh
make onnxruntime-fetch                       # into .onnxruntime/
export ORT_DYLIB_PATH="$(bash scripts/fetch-onnxruntime.sh --path)"
```

The same script provisions `file-parser`'s `magika` feature and is what CI uses, so the
pinned version cannot drift between a developer's machine and the build.

If the variable is unset, or points at a missing or too-old library, `ort` 2.0.0-rc.12
**hangs instead of erroring**. The gear bounds that with a timeout at startup rather than
wedging forever — see `load_models` in `src/gear.rs` — but the fix is to point the
variable at a compatible runtime.

## Docs

| | |
|---|---|
| [DESIGN.md](../docs/DESIGN.md) | Architecture, principles, decisions, and their reasoning |
| [ADR/](../docs/ADR/) | The decision record — eleven ADRs, each with its alternatives |
| [benchmarks/](../docs/benchmarks/) | Measured detection quality per candidate model, and the method behind it |
