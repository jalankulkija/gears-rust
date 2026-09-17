Created:  2026-08-24 by Constructor Tech
Updated:  2026-08-27 by Constructor Tech

# GenAI Security Scanner — Documentation Index

Protects an organization when its people use AI chat tools: intercepts prompts and files
before they reach the model, checks them for sensitive or risky content, and returns a
verdict — allow, warn, redact, or block.

Two product gears: [`prompt-scanner`](../../gears/prompt-scanner) (the inspection
platform) and [`onnx-runner`](../../gears/onnx-runner) (generic ONNX execution). A third,
[`forward-proxy`](../../gears/forward-proxy), is demo and test infrastructure only — not a
deliverable.

## Start Here

| If you want to... | Read |
|---|---|
| Run the pipeline locally | [SETUP.md](./SETUP.md) |
| Try prompts that trigger each detector | [test-prompts.md](./test-prompts.md) |
| Understand the architecture | [DESIGN.md](./DESIGN.md) |
| Know why something was built this way | [`onnx-runner/docs/ADR/`](../../gears/onnx-runner/docs/ADR/) |

## Process

This project follows the repo's standard chain:

```
upstream requirements  →  DESIGN  →  code  →  ADR
```

Requirements are **referenced, not copied**. Product requirements are maintained outside
this repository; this documentation cites their stable identifiers only. See
[DESIGN.md](./DESIGN.md) §1.2 for the convention and §4 for requirement ownership.

## Document Map

**Program level** (`docs/genai-security/`) — what spans gears:

| Document | Contents |
|---|---|
| [DESIGN.md](./DESIGN.md) | Architecture, principles, extension contracts, requirement ownership |
| [SETUP.md](./SETUP.md) | Local run instructions |
| [test-prompts.md](./test-prompts.md) | Manual test prompts per detector |

**Gear level** (`gears/<gear>/docs/`) — what belongs to one gear:

| Gear | Design | ADRs |
|---|---|---|
| prompt-scanner *(product)* | [DESIGN.md](../../gears/prompt-scanner/docs/DESIGN.md) | — |
| onnx-runner *(product)* | [DESIGN.md](../../gears/onnx-runner/docs/DESIGN.md) | [ADR/](../../gears/onnx-runner/docs/ADR/) |
| forward-proxy *(demo/test)* | [DESIGN.md](../../gears/forward-proxy/docs/DESIGN.md) | — |
