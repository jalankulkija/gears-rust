# forward-proxy SDK

Transport-agnostic public API for the `forward-proxy` gear: the
`ProxyHandlerPluginClient` trait, request/response models, and error types.
Handler plugins (e.g. `openai-interceptor-plugin`) implement this trait and
register with `types-registry` to be discovered by `forward-proxy`'s core
dispatch loop.

See `../docs/DESIGN.md` for the full design.
