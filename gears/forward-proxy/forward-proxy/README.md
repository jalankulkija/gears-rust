# forward-proxy

Generic forward/MITM HTTPS proxy gear, built on [`hudsucker`](https://github.com/omjadas/hudsucker).
Dispatches intercepted traffic through an ordered chain of handler plugins
(`cf-gears-forward-proxy-sdk`'s `ProxyHandlerPluginClient`) — the core gear
has no built-in knowledge of any specific traffic (e.g. OpenAI/ChatGPT); see
`gears/forward-proxy/plugins/openai-interceptor-plugin/` for that.

See `../docs/DESIGN.md` for the full design.
