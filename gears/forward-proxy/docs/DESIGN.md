# forward-proxy — Design

`forward-proxy` is a generic forward/MITM proxy gear. It runs the proxy listener, manages a
local root CA, matches target hosts, and dispatches intercepted traffic through an ordered
chain of handler plugins. Provider-specific logic — prompt-slot extraction, attachment
handling, the scanner call — lives in those plugins, not in the core gear.

It is demo and test infrastructure for the GenAI Security scanning platform, not a product
deliverable. See the [program DESIGN](../../../docs/genai-security/DESIGN.md) §1.3 for that
scope statement and what it explicitly excludes.

## Background

`ChatGPT`'s web app and the OpenAI API carry prompt text and file uploads in several
distinct shapes, and understanding all of them is what a scanning proxy has to do:

- inline base64 attachments in the JSON body (`input_file.file_data`, `image_url`),
- OpenAI API `POST /v1/files` multipart uploads,
- the web app's own upload flow, which `PUT`s to a SAS-signed `*.oaiusercontent.com`
  host after `POST /backend-api/files` returns a `file_id`.

The gear is built on [`hudsucker`](https://github.com/omjadas/hudsucker), a forward/MITM
proxy library that terminates TLS on both legs of a `CONNECT` tunnel, dynamically mints a
leaf certificate per SNI signed by a locally generated root CA, and exposes a programmatic
`HttpHandler` trait for request/response interception.

## Why a generic proxy core + plugins, not a single OpenAI-specific gear

Rather than build one gear that both runs the proxy *and* knows about OpenAI/prompts/
attachments/prompt-scanner, the core gear (`forward-proxy`) stays completely generic:
it only knows how to run a hudsucker listener, manage the CA, match target hosts, and
dispatch intercepted traffic through an **ordered chain of handler plugins**. This
mirrors `prompt-scanner`'s own detector-plugin fan-out architecture
(`gears/prompt-scanner/docs/DESIGN.md`), except handlers compose **sequentially**
(each can rewrite the request the next handler sees) rather than fan out to
independent voters, since request mutation is order-sensitive.

The OpenAI/ChatGPT-specific logic — prompt-slot extraction, the three attachment
paths, the prompt-scanner call — becomes `openai-interceptor-plugin`, one handler
plugin among potentially many. It, not the core gear, depends on `prompt-scanner`. A
future non-AI use case (e.g. a generic egress DLP handler) could reuse the same
proxy core with zero changes.

## Directory structure

```
gears/forward-proxy/
├── forward-proxy-sdk/
│   └── src/
│       ├── lib.rs
│       ├── models.rs         # ProxyRequest, ProxyResponse, ProxyOutcome
│       ├── plugin_api.rs     # ProxyHandlerPluginClient (implemented by each handler)
│       ├── errors.rs         # ProxyHandlerError
│       └── gts.rs            # ProxyHandlerPluginSpecV1 GTS schema
├── forward-proxy/
│   └── src/
│       ├── gear.rs            # capabilities=[stateful, rest], lifecycle(entry="serve")
│       ├── config.rs          # listen_addr, ca_cert_path, HostMatcher config
│       ├── domain/
│       │   ├── error.rs
│       │   ├── pipeline.rs    # runs the ordered Vec<Arc<dyn ProxyHandlerPluginClient>>
│       │   ├── ca.rs          # CertificateAuthority port trait
│       │   └── host_matcher.rs
│       ├── infra/
│       │   ├── hudsucker_handler.rs  # implements hudsucker::HttpHandler; hyper<->domain
│       │   │                          # type conversion happens ONLY here
│       │   └── ca_rcgen.rs           # CertificateAuthority impl backed by hudsucker's rcgen-ca
│       └── api/rest/                 # GET /forward-proxy/v1/ca.pem, /status, banner SSE feed
└── plugins/
    └── openai-interceptor-plugin/
        └── src/
            ├── gear.rs         # registers with host_patterns + priority via types-registry
            ├── domain/
            │   ├── prompt_slots.rs     # prompt-text discovery -> JSON Pointer paths
            │   ├── attachments.rs      # 3 attachment paths (multipart, inline base64, web-upload PUT)
            │   └── service.rs
            └── infra/
                └── prompt_scanner_client.rs  # ClientHub-resolved PromptScannerClientV1
```

## SDK (`forward-proxy-sdk`)

```rust
/// Generic, protocol-level view of an intercepted exchange — no domain
/// concept of "prompt" or "attachment" lives here.
pub struct ProxyRequest {
    pub method: String,
    pub uri: String,
    pub target_host: String,
    pub headers: HeaderMap,   // transport-agnostic, not hyper's
    pub body: Bytes,
}

pub struct ProxyResponse {
    pub status: u16,
    pub headers: HeaderMap,
    pub body: Bytes,
}

pub enum ProxyOutcome<T> {
    Continue(T),                 // pass through, possibly modified
    Block { status: u16, body: Bytes, reason: String },
}

/// Implemented by each handler plugin — the sequential analog of
/// prompt-scanner's `DetectorPluginClient`.
#[async_trait]
pub trait ProxyHandlerPluginClient: Send + Sync {
    async fn on_request(
        &self,
        ctx: &ProxyContext,
        req: ProxyRequest,
    ) -> Result<ProxyOutcome<ProxyRequest>, ProxyHandlerError>;

    /// Non-blocking by convention in v1 — see "Response handling" below.
    /// Default: pass through unchanged.
    async fn on_response(
        &self,
        ctx: &ProxyContext,
        resp: ProxyResponse,
    ) -> Result<ProxyOutcome<ProxyResponse>, ProxyHandlerError> {
        Ok(ProxyOutcome::Continue(resp))
    }
}
```

### Registration (host-pattern pre-filter)

Each plugin registers with a host-match pattern (like prompt-scanner's `vendor`/
`priority`), so the core skips invoking handlers for hosts they've declared no
interest in, instead of calling every handler on every request through the proxy:

```rust
PluginV1::<ProxyHandlerPluginSpecV1>::build_registration(
    "cf.builtin.openai_interceptor.v1",
    vendor,
    priority,
    host_patterns: &["chatgpt.com", "api.openai.com", "*.oaiusercontent.com"],
)
```

### `ProxyContext`

Shared state passed explicitly to handlers rather than held as process-level mutable
state:

- `banner: Arc<BannerSink>` — generic (not OpenAI-specific) push channel for a
  browser-side "here's what happened" banner; any handler can use it.
- `upload_correlation: Arc<UploadCorrelationCache>` — `file_id` → filename correlation
  across the `POST /backend-api/files` request/response pair and the later signed-URL
  `PUT`.
- Per-plugin `ClientHub` access, so e.g. `openai-interceptor-plugin` resolves
  `PromptScannerClientV1` itself — the core gear has no dependency on `prompt-scanner`.

## Extension points

| Change | What to touch |
|---|---|
| Support a new AI provider (Claude, Gemini) | New plugin gear implementing `ProxyHandlerPluginClient`, own host patterns — core untouched |
| Non-AI use case (generic egress DLP, credential-leak scanning on any traffic) | New plugin, no dependency on prompt-scanner if not needed |
| Swap CA backend (rcgen → OpenSSL → customer-supplied) | New `impl CertificateAuthority` in `forward-proxy`'s `infra/`, swapped in `gear.rs` |
| Point at different target hosts without a rebuild | `config.rs`'s `HostMatcher` (exact/suffix/glob) |
| Change enforcement (banner vs. event-broker audit vs. hard block) | Handler-plugin-local decision — core just executes `Continue`/`Block` |
| New attachment upload shape | New recognizer inside `openai-interceptor-plugin`'s `attachments.rs` — core and other plugins untouched |

## Response handling — decision

Two different concerns hide under "handle responses":

1. **Plumbing/UX (in scope for v1, always non-blocking).** Capturing the `file_id`
   out of `POST /backend-api/files`'s JSON response, and rewriting HTML to inject
   the banner script. Both are small, non-streamed rewrites.
2. **Response *content* scanning** — inspecting what the model says back for
   leaked secrets/PII, jailbreak-success indicators, etc. The obstacle: ChatGPT/
   OpenAI responses are streamed token-by-token (SSE); once bytes reach the
   browser you can't unsend them, so "block a bad response" doesn't mean the same
   thing it does for requests.

   Three postures were considered, in increasing order of intrusiveness:
   - **Off (request-side only).** Zero latency, zero streaming impact.
   - **Stream-and-audit.** Forward chunks live (no UX impact), scan a rolling
     buffer async, log/alert on hits but can't retract already-sent content.
   - **Buffer-and-block.** Hold the entire response until scanned, then release
     or block. Real prevention, but destroys streaming UX.

**Decision: ship "off" for v1.** `on_response` exists on the trait now (needed for
plumbing regardless) but is non-blocking in every built-in handler; no content
scanning/blocking on responses in this iteration. `ProxyOutcome::Block` remains
available on the response path in the trait signature for a future handler that
wants stream-and-audit or buffer-and-block — the core architecture does not need
to change to add that later, only a specific plugin's behavior does.

## Open questions / not yet decided

- Whether `forward-proxy` needs its own DB-backed audit trail (a `scan_record`-like
  table) or whether that responsibility stays entirely with `prompt-scanner` /
  whichever handler plugin calls it.
- Async job model for handlers that need to do slow work (e.g. large attachment
  parsing) without blocking the proxy's request path — prompt-scanner already has
  `scan_async`/`get_verdict` for this; `openai-interceptor-plugin` may need a
  similar synchronous-vs-async size threshold.
