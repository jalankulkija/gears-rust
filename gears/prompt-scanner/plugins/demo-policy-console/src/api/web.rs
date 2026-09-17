//! The demo control-panel page: `GET /demo-policy-console/`.
//!
//! One inline `Html<String>` handler, no build step, no frontend framework -
//! same pattern `gears/system/api-gateway/src/web.rs`'s `serve_docs` uses.
//! This whole gear is temporary (see `docs/POLICY_PROVIDER_AND_DEMO_CONSOLE.md`),
//! so a build pipeline for the UI would be effort spent on the wrong thing.

use axum::response::Html;

/// `GET /demo-policy-console/` - the control panel.
pub async fn index() -> Html<&'static str> {
    Html(PAGE)
}

const PAGE: &str = r#"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8"/>
<title>GenAI Security - Demo Policy Console</title>
<style>
  :root { color-scheme: dark; }
  body {
    font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif;
    background: #0f1115; color: #e6e6e6; margin: 0; padding: 2rem;
  }
  h1 { font-size: 1.25rem; font-weight: 600; margin-bottom: 0.25rem; }
  .subtitle { color: #9aa0a6; margin-bottom: 2rem; font-size: 0.9rem; }
  .panel {
    background: #171a21; border: 1px solid #2a2e37; border-radius: 8px;
    padding: 1.25rem 1.5rem; margin-bottom: 1.5rem; max-width: 640px;
  }
  .panel h2 { font-size: 1rem; margin: 0 0 1rem; }
  .toggle-row { display: flex; align-items: center; gap: 0.75rem; margin-bottom: 1rem; }
  .switch { position: relative; width: 44px; height: 24px; flex-shrink: 0; }
  .switch input { opacity: 0; width: 0; height: 0; }
  .slider {
    position: absolute; inset: 0; background: #3a3f4b; border-radius: 24px;
    cursor: pointer; transition: background 0.15s;
  }
  .slider::before {
    content: ""; position: absolute; width: 18px; height: 18px; left: 3px; top: 3px;
    background: #e6e6e6; border-radius: 50%; transition: transform 0.15s;
  }
  input:checked + .slider { background: #3b82f6; }
  input:checked + .slider::before { transform: translateX(20px); }
  .radios { display: flex; flex-direction: column; gap: 0.5rem; }
  .radios label { display: flex; align-items: center; gap: 0.5rem; font-size: 0.9rem; cursor: pointer; }
  .radios label.disabled { opacity: 0.4; cursor: not-allowed; }
  table { width: 100%; border-collapse: collapse; font-size: 0.85rem; }
  th, td { text-align: left; padding: 0.4rem 0.6rem; border-bottom: 1px solid #2a2e37; }
  th { color: #9aa0a6; font-weight: 500; }
  .badge {
    display: inline-block; padding: 0.1rem 0.5rem; border-radius: 4px; font-size: 0.75rem;
    font-weight: 600; text-transform: uppercase;
  }
  .badge-allow { background: #1f3a2a; color: #4ade80; }
  .badge-audit { background: #2a2f1f; color: #d4d44a; }
  .badge-warn { background: #3a2f1f; color: #f0a63a; }
  .badge-redact { background: #1f2a3a; color: #6ab7f5; }
  .badge-block { background: #3a1f1f; color: #f56a6a; }
  .empty { color: #6a6f78; font-size: 0.85rem; padding: 0.5rem 0; }
  .status { font-size: 0.8rem; color: #6a6f78; margin-top: 0.75rem; min-height: 1em; }
</style>
</head>
<body>
  <h1>GenAI Security - Demo Policy Console</h1>

  <div class="panel">
    <h2>PII detection</h2>
    <div class="toggle-row">
      <label class="switch">
        <input type="checkbox" id="pii-enabled"/>
        <span class="slider"></span>
      </label>
      <span id="toggle-label">enabled</span>
    </div>
    <div class="radios" id="action-radios">
      <label><input type="radio" name="pii-action" value="allow"/> Allow - record only, don't change the verdict</label>
      <label><input type="radio" name="pii-action" value="audit"/> Audit - record for review</label>
      <label><input type="radio" name="pii-action" value="warn"/> Warn - surface a warning to the caller</label>
      <label><input type="radio" name="pii-action" value="redact"/> Redact - strip the matched content</label>
      <label><input type="radio" name="pii-action" value="block"/> Block - reject the content entirely</label>
    </div>
    <div class="status" id="status"></div>
  </div>

  <div class="panel">
    <h2>Audit log <span style="color:#6a6f78; font-weight:400;">(latest 50)</span></h2>
    <div id="audit-container"><div class="empty">Loading...</div></div>
  </div>

<script>
// Relative, not "/demo-policy-console/v1" - this page can be served behind
// a gateway path prefix (e.g. "/cf"), and an absolute path here would skip
// it, 404ing every fetch below. The page route itself ends in a trailing
// slash ("/demo-policy-console/"), so a relative "v1/..." resolves next to
// it correctly regardless of what prefix precedes it.
const API = "v1";

async function loadState() {
  const res = await fetch(`${API}/state`);
  if (!res.ok) return;
  const state = await res.json();
  document.getElementById("pii-enabled").checked = state.pii_enabled;
  document.getElementById("toggle-label").textContent = state.pii_enabled ? "enabled" : "disabled";
  document.querySelectorAll('input[name="pii-action"]').forEach(r => {
    r.checked = r.value === state.pii_action;
  });
  applyDisabledStyling(state.pii_enabled);
}

function applyDisabledStyling(enabled) {
  document.querySelectorAll('#action-radios input[name="pii-action"]').forEach(r => r.disabled = !enabled);
  document.querySelectorAll('#action-radios label').forEach(l => l.classList.toggle("disabled", !enabled));
}

async function pushState() {
  const pii_enabled = document.getElementById("pii-enabled").checked;
  const checked = document.querySelector('input[name="pii-action"]:checked');
  const pii_action = checked ? checked.value : "redact";
  applyDisabledStyling(pii_enabled);
  document.getElementById("toggle-label").textContent = pii_enabled ? "enabled" : "disabled";

  const status = document.getElementById("status");
  status.textContent = "saving...";
  try {
    const res = await fetch(`${API}/state`, {
      method: "PUT",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ pii_enabled, pii_action }),
    });
    status.textContent = res.ok ? "saved" : `save failed (${res.status})`;
  } catch (e) {
    status.textContent = `save failed: ${e}`;
  }
  setTimeout(() => { status.textContent = ""; }, 2000);
}

document.getElementById("pii-enabled").addEventListener("change", pushState);
document.querySelectorAll('input[name="pii-action"]').forEach(r => r.addEventListener("change", pushState));

function renderAudit(rows) {
  const container = document.getElementById("audit-container");
  if (!rows.length) {
    container.innerHTML = '<div class="empty">No scans recorded yet.</div>';
    return;
  }
  const rowsHtml = rows.map(r => `
    <tr>
      <td><code>${r.trace_id.slice(0, 8)}...</code></td>
      <td><span class="badge badge-${r.action}">${r.action}</span></td>
      <td>${r.decisive_detectors.length ? r.decisive_detectors.join(", ") : "-"}</td>
      <td>${r.categories.length ? r.categories.join(", ") : "-"}</td>
      <td>${new Date(r.created_at).toLocaleString()}</td>
    </tr>`).join("");
  container.innerHTML = `
    <table>
      <thead><tr><th>Trace</th><th>Action</th><th>Detector</th><th>Categories</th><th>When</th></tr></thead>
      <tbody>${rowsHtml}</tbody>
    </table>`;
}

async function loadAudit() {
  try {
    const res = await fetch(`${API}/audit`);
    if (!res.ok) return;
    renderAudit(await res.json());
  } catch (e) {
    // Transient fetch failure - leave the previous render in place; the
    // next poll tick will retry.
  }
}

loadState();
loadAudit();
setInterval(loadAudit, 4000);
</script>
</body>
</html>
"#;
