use crate::diagnostics::GameHealthSnapshot;

fn esc_html(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

const SHARED_STYLES: &str = r#"
  :root {
    --paper: #0D0D0E;
    --surface: #17181A;
    --surface-2: #202124;
    --line: #2C2E31;
    --line-strong: #6E6F72;
    --ink: #EDEBE6;
    --ink-2: #A9A7A1;
    --ink-3: #8A8883;
    --gilt: #E3C46A;
    --error: #FF6B5E;
    --warning: #FF8A3D;
    --tape-1: #3FA9A4;
    --tape-2: #E3C46A;
    --tape-3: #FF8A3D;
    --tape-4: #FF6B5E;
    --tape-5: #E0408F;
    --tape-6: #7B3F8F;
  }
  * { box-sizing: border-box; border-radius: 0; }
  html { scroll-padding-top: 2rem; }
  body {
    margin: 0;
    background: var(--paper);
    color: var(--ink);
    font-family: Inter, system-ui, -apple-system, Segoe UI, Roboto, sans-serif;
    font-size: 1rem;
    line-height: 1.5;
  }
  .wrap {
    max-width: 1440px;
    margin: 0 auto;
    padding: 1.5rem clamp(16px, 4vw, 64px) 3rem;
  }
  a {
    color: var(--ink);
    text-decoration: underline;
    text-decoration-thickness: 1px;
    text-underline-offset: 0.2em;
  }
  a:hover { text-decoration-thickness: 2px; }
  :focus-visible {
    outline: 2px solid var(--ink);
    outline-offset: 2px;
    box-shadow: 0 0 0 4px var(--paper);
  }
  @media (prefers-reduced-motion: reduce) {
    * { animation: none !important; transition: none !important; }
  }
  .skip-link {
    position: absolute;
    left: -9999px;
  }
  .skip-link:focus {
    position: static;
    display: inline-block;
    padding: 0.5rem 1rem;
    background: var(--ink);
    color: var(--paper);
    font-weight: 700;
    margin-bottom: 1rem;
  }
  header {
    border-bottom: 1px solid var(--line);
    padding-bottom: 1rem;
    margin-bottom: 1.5rem;
    display: flex;
    flex-wrap: wrap;
    align-items: flex-end;
    justify-content: space-between;
    gap: 1rem;
  }
  .wordmark {
    display: flex;
    align-items: center;
    gap: 0.75rem;
  }
  .mark-square {
    width: 24px;
    height: 24px;
    background: var(--ink);
    flex-shrink: 0;
  }
  h1 {
    margin: 0;
    font-family: Archivo, system-ui, sans-serif;
    font-weight: 800;
    font-size: clamp(1.75rem, 1.2rem + 1.8vw, 2.5rem);
    line-height: 1.05;
    letter-spacing: -0.03em;
  }
  h2 {
    margin: 2rem 0 0.75rem;
    font-family: Archivo, system-ui, sans-serif;
    font-weight: 700;
    font-size: 1.35rem;
    line-height: 1.15;
    letter-spacing: -0.015em;
  }
  h3 {
    margin: 0 0 0.35rem;
    font-size: 0.875rem;
    font-weight: 600;
    color: var(--ink-2);
  }
  nav {
    display: flex;
    flex-wrap: wrap;
    gap: 0.75rem;
  }
  nav a {
    display: inline-flex;
    align-items: center;
    min-height: 44px;
    padding: 0.4rem 0.85rem;
    border: 1px solid var(--line-strong);
    text-decoration: none;
    font-weight: 600;
  }
  nav a[aria-current="page"] {
    background: var(--ink);
    color: var(--paper);
    border-color: var(--ink);
  }
  .grid {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(220px, 1fr));
    gap: 1rem;
    margin-bottom: 1.25rem;
  }
  .card {
    background: var(--surface);
    border: 1px solid var(--line);
    padding: 1rem;
  }
  .card--strong {
    border-color: var(--line-strong);
  }
  .val {
    font-size: 1.35rem;
    font-weight: 700;
    font-variant-numeric: tabular-nums;
    word-break: break-word;
  }
  .meta {
    margin-top: 0.35rem;
    font-size: 0.875rem;
    color: var(--ink-2);
    font-variant-numeric: tabular-nums;
  }
  .badge {
    display: inline-block;
    padding: 0.15rem 0.5rem;
    border: 1px solid var(--line-strong);
    font-size: 0.8125rem;
    font-weight: 600;
    font-variant-numeric: tabular-nums;
  }
  .badge--inverse {
    background: var(--ink);
    color: var(--paper);
    border-color: var(--ink);
  }
  .badge--warn {
    border-color: var(--warning);
    color: var(--warning);
  }
  .badge--err {
    border-color: var(--error);
    color: var(--error);
  }
  .controls-bar {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(300px, 1fr));
    gap: 1rem;
    margin-bottom: 1.5rem;
  }
  .panel {
    background: var(--surface);
    border: 1px solid var(--line-strong);
    padding: 1rem;
  }
  .btn-row {
    display: flex;
    flex-wrap: wrap;
    gap: 0.5rem;
    margin-top: 0.5rem;
  }
  button {
    min-height: 44px;
    padding: 0.5rem 1rem;
    font-family: inherit;
    font-size: 0.9375rem;
    font-weight: 600;
    cursor: pointer;
    border: 1px solid var(--line-strong);
    background: transparent;
    color: var(--ink);
  }
  button.primary, button[aria-pressed="true"] {
    background: var(--ink);
    color: var(--paper);
    border-color: var(--ink);
  }
  label {
    display: block;
    font-size: 0.875rem;
    font-weight: 600;
    margin-top: 0.6rem;
    margin-bottom: 0.25rem;
  }
  input[type="text"] {
    width: 100%;
    min-height: 44px;
    padding: 0.5rem 0.75rem;
    background: var(--surface-2);
    color: var(--ink);
    border: 1px solid var(--line);
    border-bottom: 2px solid var(--line-strong);
    font-family: inherit;
    font-size: 0.9375rem;
  }
  .checkbox-row {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    margin-top: 0.6rem;
  }
  .checkbox-row input {
    width: 20px;
    height: 20px;
  }
  .checkbox-row label {
    margin: 0;
  }
  .status-line {
    margin-top: 0.75rem;
    padding: 0.5rem 0.75rem;
    background: var(--surface-2);
    border-left: 3px solid var(--line-strong);
    font-size: 0.875rem;
    color: var(--ink-2);
    min-height: 2.25rem;
    font-variant-numeric: tabular-nums;
  }
  .status-line--err {
    border-left-color: var(--error);
    color: var(--error);
  }
  .table-wrap {
    overflow-x: auto;
    border: 1px solid var(--line);
    background: var(--surface);
    margin-bottom: 1.5rem;
  }
  table {
    width: 100%;
    border-collapse: collapse;
    font-variant-numeric: tabular-nums;
  }
  th, td {
    border-bottom: 1px solid var(--line);
    padding: 0.65rem 0.75rem;
    text-align: left;
    vertical-align: top;
  }
  th {
    background: var(--surface-2);
    color: var(--ink-2);
    font-weight: 600;
    font-size: 0.875rem;
    border-bottom: 1px solid var(--line-strong);
  }
  td.num, th.num {
    text-align: right;
  }
  code {
    font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
    font-size: 0.875rem;
  }
  .notif-list {
    list-style: none;
    padding: 0;
    margin: 0;
    border: 1px solid var(--line);
    background: var(--surface);
  }
  .notif-list li {
    padding: 0.6rem 0.75rem;
    border-bottom: 1px solid var(--line);
    font-variant-numeric: tabular-nums;
  }
  .notif-list li:last-child { border-bottom: none; }
  .tape-bar {
    display: grid;
    grid-template-columns: repeat(6, 1fr);
    height: 12px;
    width: 100%;
    margin-top: 2rem;
  }
"#;

/// Render a Swiss/Grid dark-only, WCAG 2.2 AAA compliant Game Health local control center.
/// - Zero secrets exposed (`api_key` and `device_token` are never present in `GameHealthSnapshot`).
/// - Semantic HTML landmarks, visible focus rings, `prefers-reduced-motion`, tabular numerals.
/// - Interactive controls for `POST /session/mode`, `POST /sync-now`, and `POST /v1/observation/import`.
pub fn render_game_health_html(health: &GameHealthSnapshot) -> String {
    let primary_session = health
        .sessions
        .iter()
        .find(|s| s.active)
        .or_else(|| health.sessions.first());

    let (active_trust_str, downgrade_text, active_game_label) = match primary_session {
        Some(s) => (
            s.active_trust_tier.as_str().to_string(),
            s.downgrade_reason
                .as_deref()
                .map(|r| format!("Downgraded to LOCAL: {}", r))
                .unwrap_or_else(|| "None (nominal)".to_string()),
            s.canonical_game_id.clone(),
        ),
        None => (
            "LOCAL (idle)".to_string(),
            "None".to_string(),
            "No active game session".to_string(),
        ),
    };

    let mut session_rows = String::new();
    if health.sessions.is_empty() {
        session_rows.push_str(
            "<tr><td colspan=\"8\">No active or historical runtime sessions recorded yet.</td></tr>",
        );
    } else {
        for s in &health.sessions {
            let status_badge = if s.evaluation_incomplete {
                r#"<span class="badge badge--warn">Warning: incomplete (frame gap)</span>"#
            } else if s.is_downgraded() {
                r#"<span class="badge badge--warn">Downgraded to LOCAL</span>"#
            } else if s.active {
                r#"<span class="badge badge--inverse">Active</span>"#
            } else {
                r#"<span class="badge">Ended</span>"#
            };
            session_rows.push_str(&format!(
                "<tr>\
                 <td><code>{}</code></td>\
                 <td>{}</td>\
                 <td>{}</td>\
                 <td><code>{}</code></td>\
                 <td><code>{}</code></td>\
                 <td>{}</td>\
                 <td class=\"num\">{} (gaps: {})</td>\
                 <td>{}</td>\
                 </tr>",
                esc_html(&s.canonical_game_id),
                esc_html(s.matched_pack_id.as_deref().unwrap_or("No pack matched")),
                esc_html(s.ingestion_plane.as_str()),
                esc_html(s.progression_mode.as_str()),
                esc_html(s.active_trust_tier.as_str()),
                esc_html(s.downgrade_reason.as_deref().unwrap_or("None")),
                s.frame_snapshots_evaluated,
                s.frame_gaps_recorded,
                status_badge,
            ));
        }
    }

    let mut ring_rows = String::new();
    if health.plane_a.rings.is_empty() {
        ring_rows.push_str(
            "<tr><td colspan=\"8\">No Plane A shared-memory rings registered yet.</td></tr>",
        );
    } else {
        for r in &health.plane_a.rings {
            let state_label = if r.active { "Active" } else { "Closed" };
            ring_rows.push_str(&format!(
                "<tr>\
                 <td><code>{}</code></td>\
                 <td><code>{}</code></td>\
                 <td>{}</td>\
                 <td class=\"num\">{} (seq {})</td>\
                 <td class=\"num\">shm_gaps: {} / ipc_gaps: {} / seq_gaps: {}</td>\
                 <td class=\"num\">{}</td>\
                 <td class=\"num\">poll: {}us (max {}us) / eval: {}us (max {}us)</td>\
                 <td class=\"num\">{} / {}</td>\
                 </tr>",
                esc_html(&r.session_id),
                esc_html(if r.shm_name.is_empty() { "in-process" } else { &r.shm_name }),
                esc_html(state_label),
                r.snapshots_consumed,
                r.last_sequence_number,
                r.shm_frame_gaps_reported,
                r.ipc_frame_gaps_reported,
                r.sequence_gaps_detected,
                r.crc_errors,
                r.last_poll_latency_us,
                r.max_poll_latency_us,
                r.last_eval_latency_us,
                r.max_eval_latency_us,
                r.unlocked_rules_count,
                r.total_rules_count,
            ));
        }
    }

    let mut pack_rows = String::new();
    if health.installed_packs.is_empty() {
        pack_rows.push_str(
            "<tr><td colspan=\"6\">No .onehundo packs discovered in configured pack directories.</td></tr>",
        );
    } else {
        for p in &health.installed_packs {
            pack_rows.push_str(&format!(
                "<tr>\
                 <td><code>{}</code></td>\
                 <td>{}</td>\
                 <td><code>{}</code></td>\
                 <td class=\"num\">{}</td>\
                 <td>{}</td>\
                 <td><code>{}</code></td>\
                 </tr>",
                esc_html(&p.pack_id),
                esc_html(p.pack_version.as_deref().unwrap_or("—")),
                esc_html(p.mapped_oap_edition.as_deref().unwrap_or("Unmapped")),
                p.achievement_count,
                esc_html(&p.enabled_planes.join(", ")),
                esc_html(&p.pack_sha256),
            ));
        }
    }

    let mut obs_rows = String::new();
    if health.recent_observations.is_empty() {
        obs_rows.push_str(
            "<tr><td colspan=\"7\">No Plane C external observations imported yet.</td></tr>",
        );
    } else {
        for o in &health.recent_observations {
            let ts_cell = match o.source_timestamp.as_deref() {
                Some(ts) => format!("<code>{}</code>", esc_html(ts)),
                None => "<span class=\"badge\">null (not invented)</span>".to_string(),
            };
            obs_rows.push_str(&format!(
                "<tr>\
                 <td><code>{}</code></td>\
                 <td><code>{}</code></td>\
                 <td>{}</td>\
                 <td><code>{}</code></td>\
                 <td>{}</td>\
                 <td><code>{}</code> / <code>{}</code></td>\
                 <td><code>{}</code></td>\
                 </tr>",
                esc_html(&o.canonical_game_id),
                esc_html(&o.key),
                esc_html(&o.source_type),
                esc_html(&o.progression_mode),
                ts_cell,
                esc_html(&o.evidence_class),
                esc_html(&o.trust_tier),
                esc_html(&o.verification_state),
            ));
        }
    }

    let mut notif_items = String::new();
    if health.recent_notifications.is_empty() {
        notif_items.push_str("<li>No notifications recorded yet.</li>");
    } else {
        for n in health.recent_notifications.iter().rev().take(15) {
            notif_items.push_str(&format!(
                "<li><strong>[{}] {}</strong> — {}</li>",
                esc_html(&n.timestamp),
                esc_html(&n.title),
                esc_html(&n.body)
            ));
        }
    }

    let active_mode = health.active_progression_mode.as_str();
    let mode_btn = |m: &str, label: &str| -> String {
        let pressed = if active_mode == m { "true" } else { "false" };
        format!(
            r#"<button type="button" class="mode-btn" data-mode="{}" aria-pressed="{}">{}</button>"#,
            m, pressed, label
        )
    };

    let sync_err_html = match health.sync.last_sync_error.as_deref() {
        Some(err) => format!(r#"<span class="badge badge--err">Error: {}</span>"#, esc_html(err)),
        None => "None".to_string(),
    };

    let shm_gaps_total: u64 = health
        .plane_a
        .rings
        .iter()
        .map(|r| r.shm_frame_gaps_reported)
        .sum();
    let ipc_gaps_total: u64 = health
        .plane_a
        .rings
        .iter()
        .map(|r| r.ipc_frame_gaps_reported)
        .sum();

    format!(
        r##"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>OneHundo Game Health Diagnostics</title>
<style>{styles}</style>
</head>
<body>
<div class="wrap">
<a class="skip-link" href="#main-content">Skip to main content</a>
<header>
  <div class="wordmark">
    <div class="mark-square" aria-hidden="true"></div>
    <div>
      <h1>OneHundo Game Health</h1>
      <div class="meta">Local daemon control center · v{version} · Uptime {uptime}s</div>
    </div>
  </div>
  <nav aria-label="Primary">
    <a href="/" aria-current="page">Game Health</a>
    <a href="/ui/packs">Pack Browser</a>
    <a href="/health">Health JSON</a>
    <a href="/packs">Packs JSON</a>
    <a href="/progression">Progression JSON</a>
  </nav>
</header>

<main id="main-content">
  <section aria-labelledby="session-card-heading">
    <h2 id="session-card-heading">Active Session &amp; Telemetry Summary</h2>
    <div class="grid" aria-live="polite">
      <div class="card card--strong">
        <h3>Active Progression Mode</h3>
        <div class="val" id="active-mode-val"><code>{active_mode}</code></div>
        <div class="meta">Game: {active_game}</div>
      </div>
      <div class="card card--strong">
        <h3>Trust Tier &amp; Fair Play</h3>
        <div class="val"><code>{active_trust}</code></div>
        <div class="meta">Downgrade: {downgrade_reason}</div>
      </div>
      <div class="card">
        <h3>Plane A &amp; B Diagnostics</h3>
        <div class="val">{rings_active} rings · {eval_max}us max eval</div>
        <div class="meta">shm_frame_gaps_reported: {shm_gaps} · ipc_frame_gaps_reported: {ipc_gaps} · seq_gaps: {seq_gaps} · plane_b_commits: {plane_b_commits}</div>
      </div>
      <div class="card">
        <h3>Sync Queue &amp; Platform</h3>
        <div class="val">{queue_depth} pending / {synced_count} synced</div>
        <div class="meta">platform_configured: {platform_ok} · device_registered: {device_ok} · receipt: {receipt} / {acceptance}</div>
        <div class="meta">last_sync_error: {last_sync_error}</div>
      </div>
      <div class="card">
        <h3>Installed Packs &amp; Plane C</h3>
        <div class="val">{packs_count} packs · {obs_count} observations</div>
        <div class="meta">pack_errors: {pack_errors} · watched_dirs: {watched_count} · total_frame_gaps: {total_gaps}</div>
      </div>
    </div>
  </section>

  <section aria-labelledby="controls-heading">
    <h2 id="controls-heading">Local Daemon Controls</h2>
    <div class="controls-bar">
      <div class="panel">
        <h3>Session Progression Mode (<code>POST /session/mode</code>)</h3>
        <p class="meta"><code>SPECTATOR</code> never mutates persistent progression; <code>ENCORE</code> preserves historical timestamps.</p>
        <div class="btn-row" role="group" aria-label="Select progression mode">
          {btn_standard}
          {btn_encore}
          {btn_spectator}
        </div>
        <div id="mode-status" class="status-line" role="status" aria-live="polite">Current mode: {active_mode}</div>
      </div>

      <div class="panel">
        <h3>Cloud Sync Outbox (<code>POST /sync-now</code>)</h3>
        <p class="meta">Flush durable SQLite WAL events to <code>onehundo-platform</code>.</p>
        <div class="btn-row">
          <button type="button" id="sync-now-btn" class="primary">Sync Now</button>
        </div>
        <div id="sync-status" class="status-line" role="status" aria-live="polite">Pending queue depth: {queue_depth}</div>
      </div>

      <div class="panel">
        <h3>Import Plane C Observation (<code>POST /v1/observation/import</code>)</h3>
        <form id="import-obs-form">
          <label for="obs-path">File or trophy directory path</label>
          <input id="obs-path" name="path" type="text" placeholder="/path/to/achievements.json or TROPUSR.DAT dir" required>
          <label for="obs-game-id">Canonical game ID override (optional)</label>
          <input id="obs-game-id" name="canonical_game_id" type="text" placeholder="e.g. sony.bloodborne or steam.400">
          <div class="checkbox-row">
            <input id="obs-dry-run" name="dry_run" type="checkbox">
            <label for="obs-dry-run">Dry-run preview (no SQLite write)</label>
          </div>
          <div class="btn-row">
            <button type="submit" class="primary">Import Observation</button>
          </div>
        </form>
        <div id="import-status" class="status-line" role="status" aria-live="polite">Ready to import Goldberg/GSE, RPCS3/shadPS4, or .oh-obs.json files.</div>
      </div>
    </div>
  </section>

  <section aria-labelledby="sessions-heading">
    <h2 id="sessions-heading">Runtime Sessions</h2>
    <div class="table-wrap">
      <table>
        <thead>
          <tr>
            <th scope="col">Game ID</th>
            <th scope="col">Pack</th>
            <th scope="col">Plane</th>
            <th scope="col">Mode</th>
            <th scope="col">Trust Tier</th>
            <th scope="col">Downgrade Reason</th>
            <th scope="col" class="num">Frames / Gaps</th>
            <th scope="col">Status</th>
          </tr>
        </thead>
        <tbody>{session_rows}</tbody>
      </table>
    </div>
  </section>

  <section aria-labelledby="plane-a-heading">
    <h2 id="plane-a-heading">Plane A Shared-Memory Rings</h2>
    <div class="table-wrap">
      <table>
        <thead>
          <tr>
            <th scope="col">Session ID</th>
            <th scope="col">SHM Ring</th>
            <th scope="col">State</th>
            <th scope="col" class="num">Snapshots Consumed</th>
            <th scope="col" class="num">FRAME_GAP Counters</th>
            <th scope="col" class="num">CRC Errors</th>
            <th scope="col" class="num">Poll / Eval Latency</th>
            <th scope="col" class="num">Rules Unlocked</th>
          </tr>
        </thead>
        <tbody>{ring_rows}</tbody>
      </table>
    </div>
  </section>

  <section aria-labelledby="packs-heading">
    <h2 id="packs-heading">Installed .onehundo Packs</h2>
    <div class="table-wrap">
      <table>
        <thead>
          <tr>
            <th scope="col">Pack ID</th>
            <th scope="col">Version</th>
            <th scope="col">Mapped OAP Edition</th>
            <th scope="col" class="num">Achievements</th>
            <th scope="col">Enabled Planes</th>
            <th scope="col">SHA-256</th>
          </tr>
        </thead>
        <tbody>{pack_rows}</tbody>
      </table>
    </div>
  </section>

  <section aria-labelledby="obs-heading">
    <h2 id="obs-heading">Recent Plane C External Observations</h2>
    <div class="table-wrap">
      <table>
        <thead>
          <tr>
            <th scope="col">Canonical Game ID</th>
            <th scope="col">Key</th>
            <th scope="col">Source</th>
            <th scope="col">Mode</th>
            <th scope="col">Source Timestamp</th>
            <th scope="col">Evidence / Trust</th>
            <th scope="col">Verification</th>
          </tr>
        </thead>
        <tbody>{obs_rows}</tbody>
      </table>
    </div>
  </section>

  <section aria-labelledby="notifications-heading">
    <h2 id="notifications-heading">Recent Notifications</h2>
    <ul class="notif-list">{notif_items}</ul>
  </section>
</main>

<div class="tape-bar" aria-hidden="true">
  <div style="background: var(--tape-1);"></div>
  <div style="background: var(--tape-2);"></div>
  <div style="background: var(--tape-3);"></div>
  <div style="background: var(--tape-4);"></div>
  <div style="background: var(--tape-5);"></div>
  <div style="background: var(--tape-6);"></div>
</div>
</div>

<script>
(function() {{
  const modeBtns = document.querySelectorAll('.mode-btn');
  const modeStatus = document.getElementById('mode-status');
  const activeModeVal = document.getElementById('active-mode-val');
  modeBtns.forEach((btn) => {{
    btn.addEventListener('click', async () => {{
      const mode = btn.getAttribute('data-mode');
      modeStatus.className = 'status-line';
      modeStatus.textContent = 'Updating progression mode...';
      try {{
        const res = await fetch('/session/mode', {{
          method: 'POST',
          headers: {{ 'Content-Type': 'application/json' }},
          body: JSON.stringify({{ mode }})
        }});
        const data = await res.json();
        if (!res.ok) throw new Error(data.error || 'Mode update failed');
        modeBtns.forEach((b) => b.setAttribute('aria-pressed', String(b.getAttribute('data-mode') === data.active_progression_mode)));
        if (activeModeVal) activeModeVal.innerHTML = '<code>' + data.active_progression_mode + '</code>';
        modeStatus.textContent = 'Mode updated to ' + data.active_progression_mode + ' (' + (data.updated_sessions ? data.updated_sessions.length : 0) + ' active session(s)).';
      }} catch (err) {{
        modeStatus.className = 'status-line status-line--err';
        modeStatus.textContent = 'Error: ' + err.message;
      }}
    }});
  }});

  const syncBtn = document.getElementById('sync-now-btn');
  const syncStatus = document.getElementById('sync-status');
  if (syncBtn) {{
    syncBtn.addEventListener('click', async () => {{
      syncStatus.className = 'status-line';
      syncStatus.textContent = 'Syncing pending outbox events...';
      try {{
        const res = await fetch('/sync-now', {{
          method: 'POST',
          headers: {{ 'Content-Type': 'application/json' }},
          body: '{{}}'
        }});
        const data = await res.json();
        if (!res.ok) throw new Error(data.error || 'Sync failed');
        syncStatus.textContent = 'Synced ' + (data.synced_count || 0) + ' event(s), receipt=' + (data.batch_receipt_status || 'NONE');
      }} catch (err) {{
        syncStatus.className = 'status-line status-line--err';
        syncStatus.textContent = 'Sync error: ' + err.message;
      }}
    }});
  }}

  const obsForm = document.getElementById('import-obs-form');
  const obsStatus = document.getElementById('import-status');
  if (obsForm) {{
    obsForm.addEventListener('submit', async (ev) => {{
      ev.preventDefault();
      obsStatus.className = 'status-line';
      obsStatus.textContent = 'Importing external observation...';
      const path = document.getElementById('obs-path').value.trim();
      const canonicalGameId = document.getElementById('obs-game-id').value.trim();
      const dryRun = document.getElementById('obs-dry-run').checked;
      const payload = {{ path, dry_run: dryRun }};
      if (canonicalGameId) payload.canonical_game_id = canonicalGameId;
      try {{
        const res = await fetch('/v1/observation/import', {{
          method: 'POST',
          headers: {{ 'Content-Type': 'application/json' }},
          body: JSON.stringify(payload)
        }});
        const data = await res.json();
        if (!res.ok) throw new Error(data.error || 'Import failed');
        obsStatus.textContent = data.dry_run
          ? 'Dry-run preview: ' + (data.records ? data.records.length : 0) + ' unlocked record(s), ' + (data.skipped_locked || 0) + ' locked skipped.'
          : 'Imported ' + data.imported + ' record(s), deduplicated ' + data.deduplicated + ' (' + (data.source_kind || 'observation') + ').';
      }} catch (err) {{
        obsStatus.className = 'status-line status-line--err';
        obsStatus.textContent = 'Import error: ' + err.message;
      }}
    }});
  }}
}})();
</script>
</body>
</html>"##,
        styles = SHARED_STYLES,
        version = esc_html(&health.daemon_version),
        uptime = health.uptime_seconds,
        active_mode = esc_html(active_mode),
        active_game = esc_html(&active_game_label),
        active_trust = esc_html(&active_trust_str),
        downgrade_reason = esc_html(&downgrade_text),
        rings_active = health.plane_a.active_rings_count,
        eval_max = health.plane_a.max_eval_latency_us,
        shm_gaps = shm_gaps_total,
        ipc_gaps = ipc_gaps_total,
        seq_gaps = health.plane_a.total_sequence_gaps,
        plane_b_commits = health.plane_b_events_committed,
        queue_depth = health.sync_queue_depth,
        synced_count = health.synced_events_count,
        platform_ok = health.sync.platform_configured,
        device_ok = health.sync.device_registered,
        receipt = esc_html(health.last_receipt_status.as_deref().unwrap_or("NONE")),
        acceptance = esc_html(health.last_acceptance_status.as_deref().unwrap_or("NONE")),
        last_sync_error = sync_err_html,
        packs_count = health.loaded_packs_count,
        obs_count = health.observations_imported,
        pack_errors = health.pack_load_errors_count,
        watched_count = health.watched_dirs.len(),
        total_gaps = health.total_frame_gaps,
        btn_standard = mode_btn("STANDARD", "STANDARD"),
        btn_encore = mode_btn("ENCORE", "ENCORE"),
        btn_spectator = mode_btn("SPECTATOR", "SPECTATOR"),
        session_rows = session_rows,
        ring_rows = ring_rows,
        pack_rows = pack_rows,
        obs_rows = obs_rows,
        notif_items = notif_items,
    )
}
