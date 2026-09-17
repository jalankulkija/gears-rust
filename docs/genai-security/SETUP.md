# GenAI Security pipeline — setup & run

## 1. One-time setup

```bash
make genai-security-setup   # fetches the ONNX PII model (gears/onnx-runner/models/)
                            # and the ONNX Runtime (.onnxruntime/)
```

## 2. Run

```bash
make genai-security
```

Starts `cf-gears-example-server` (forward-proxy + prompt-scanner + scanner
backends, one process):

- API: `http://127.0.0.1:8087/cf`
- Proxy: `127.0.0.1:8080`

Ctrl+C tears it down. Logs: `logs/genai-security/` and
`~/.cf-gears-genai-security/logs/cf-gears.log`.

## 3. Point Firefox at the proxy

1. `about:preferences#general` → Network Settings → Settings...
2. Manual proxy configuration → HTTP Proxy `127.0.0.1`, Port `8080` →
   check **"Also use this proxy for HTTPS"**
3. No proxy for: leave empty (or add your other local hosts)

## 4. Trust the proxy's CA cert

The proxy MITMs HTTPS to `chat.openai.com` / `chatgpt.com` /
`api.openai.com`, so Firefox needs to trust its root CA — cert/key live at
`data/genai-security/forward-proxy-ca.{cert,key}.pem` (generated on first
run, persisted across restarts).

1. `about:preferences#privacy` → Certificates → View Certificates...
2. Authorities tab → Import... → select `forward-proxy-ca.cert.pem`
3. Check **"Trust this CA to identify websites"** → OK

Without this step Firefox will show a cert-untrusted error on every HTTPS
request through the proxy.

## 5. Test it

Open `https://chatgpt.com` in Firefox and send a prompt, or see
[test-prompts.md](./test-prompts.md) for prompts that exercise each
scanner backend, and how to check the audit trail.
