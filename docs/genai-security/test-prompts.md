# GenAI Security pipeline — manual test prompts

Send these through the proxy (either via the ChatGPT/OpenAI web UI with
Firefox pointed at `127.0.0.1:8080`, or via `curl --proxy` directly) to
exercise `prompt-scanner`'s scanner backends. Each one is chosen to isolate
a specific backend or category so you can tell what actually fired.

After sending, check the audit trail to see which backend matched and at
what confidence:

```
curl -s "http://127.0.0.1:8087/cf/prompt-scanner/v1/scans" | python3 -m json.tool
```

## 1. Email — both `regex-scanner` and `onnx-scanner` should catch it (expected: `redact`)

> Hi, my email is jane.doe@example.com, please get back to me.

## 2. Person name only — ML-only (`regex-scanner` has no name detection)

> My name is John Smith and I'll be out of office next week.

## 3. SSN — ML-only (`regex-scanner` has no SSN pattern)

> My social security number is 123-45-6789, can you help me file this form?

## 4. Physical address — ML-only

> Please ship it to 742 Evergreen Terrace, Springfield, IL 62704.

## 5. Phone number — both backends (regex-scanner's pattern is looser/lower-confidence than the model)

> You can reach me at +1 555-234-5678 anytime.

## 6. IBAN — regex-scanner only (checksum-validated, not in onnx-scanner's label set)

> Please wire the funds to GB82WEST12345698765432.

## 7. Password/PIN — ML-only (regex-scanner's secret pattern needs `password:`/`password=` syntax, not conversational text)

> My password is hunter2gogo and my PIN is 4471.

## 8. Credit card — full `block`, not just redact (regex-scanner: Luhn-valid PAN)

> Here's my card number 4111111111111111.

## Quick curl recipe (no OpenAI account needed)

Blocking/redaction happens at the proxy before the request reaches
OpenAI's servers, so you can test any of the above without valid API
credentials:

```bash
curl -s -X POST https://api.openai.com/v1/chat/completions \
  --proxy http://127.0.0.1:8080 \
  --cacert data/genai-security/forward-proxy-ca.cert.pem \
  -H "Content-Type: application/json" \
  -d '{"messages":[{"role":"user","content":"<paste a prompt from above here>"}]}' \
  -w "\nHTTP_STATUS:%{http_code}\n"
```

- `block` → curl gets back `{"error":"blocked by prompt-scanner",...}` with
  HTTP 403, straight from our proxy.
- `redact`/`allow` → curl gets back whatever OpenAI's real API returns
  (likely a 401 without valid credentials) — the interesting part is in the
  audit trail above, not the HTTP response.
