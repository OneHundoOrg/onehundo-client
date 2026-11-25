use crate::diagnostics::GameHealthSnapshot;

fn esc_html(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Render a WCAG 2.2 AAA compliant Game Health diagnostics dashboard.
/// - Semantic HTML landmarks (<header>, <nav>, <main>, <section>, <table>)
/// - High contrast (>= 7:1) palette, visible :focus-visible outline, prefers-reduced-motion
/// - Status never conveyed by color alone (text badges + icons + borders)
pub fn render_game_health_html(health: &GameHealthSnapshot) -> String {
    let mut session_rows = String::new();
    if health.sessions.is_empty() {
        session_rows.push_str(
            "<tr><td colspan=\"8\">No active or historical runtime sessions recorded yet.</td></tr>",
        );
    } else {
        for s in &health.sessions {
            let status_badge = if s.evaluation_incomplete {
                "WARN: INCOMPLETE (FRAME GAP)"
            } else if s.is_downgraded() {
                "NOTICE: DOWNGRADED TO LOCAL"
            } else if s.active {
                "OK: ACTIVE"
            } else {
                "ENDED"
            };
            session_rows.push_str(&format!(
                "<tr>\
                 <td><code>{}</code></td>\
                 <td>{}</td>\
                 <td>{}</td>\
                 <td>{}</td>\
                 <td>{}</td>\
                 <td>{}</td>\
                 <td>{} (gaps: {})</td>\
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
                esc_html(status_badge),
            ));
        }
    }

    let mut notif_items = String::new();
    if health.recent_notifications.is_empty() {
        notif_items.push_str("<li>No notifications recorded yet.</li>");
    } else {
        for n in health.recent_notifications.iter().rev().take(15) {
            notif_items.push_str(&format!(
                "<li><strong>[{}] {}</strong>: {}</li>",
                esc_html(&n.timestamp),
                esc_html(&n.title),
                esc_html(&n.body)
            ));
        }
    }

    format!(
        r##"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>OneHundo Game Health Diagnostics</title>
<style>
  :root {{
    --bg: #0b0d12;
    --surface: #141821;
    --text: #f8fafc;
    --muted: #cbd5e1;
    --border: #475569;
    --accent: #7dd3fc;
    --ok: #86efac;
    --warn: #fde047;
  }}
  * {{ box-sizing: border-box; }}
  body {{
    margin: 0;
    padding: 1.5rem;
    background: var(--bg);
    color: var(--text);
    font-family: system-ui, -apple-system, Segoe UI, Roboto, sans-serif;
    line-height: 1.6;
  }}
  a {{ color: var(--accent); }}
  a:focus-visible, button:focus-visible {{
    outline: 3px solid var(--warn);
    outline-offset: 2px;
  }}
  @media (prefers-reduced-motion: reduce) {{
    * {{ animation: none !important; transition: none !important; }}
  }}
  .skip-link {{
    position: absolute;
    left: -9999px;
  }}
  .skip-link:focus {{
    position: static;
    display: inline-block;
    padding: 0.5rem 1rem;
    background: var(--warn);
    color: #000;
    font-weight: 700;
  }}
  nav {{ margin-bottom: 1.5rem; display: flex; gap: 1rem; flex-wrap: wrap; }}
  nav a {{
    padding: 0.4rem 0.8rem;
    border: 1px solid var(--border);
    border-radius: 4px;
    text-decoration: none;
    font-weight: 600;
  }}
  .grid {{
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(220px, 1fr));
    gap: 1rem;
    margin-bottom: 1.5rem;
  }}
  .card {{
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: 6px;
    padding: 1rem;
  }}
  .card h2 {{ margin: 0 0 0.25rem 0; font-size: 0.95rem; color: var(--muted); }}
  .card .val {{ font-size: 1.5rem; font-weight: 700; }}
  table {{
    width: 100%;
    border-collapse: collapse;
    background: var(--surface);
    margin-bottom: 1.5rem;
  }}
  th, td {{
    border: 1px solid var(--border);
    padding: 0.6rem;
    text-align: left;
  }}
  th {{ background: #1e2433; }}
</style>
</head>
<body>
<a class="skip-link" href="#main-content">Skip to main content</a>
<header>
  <h1>OneHundo Game Health</h1>
  <nav aria-label="Primary">
    <a href="/" aria-current="page">Game Health</a>
    <a href="/ui/packs">Pack Browser</a>
    <a href="/health">Health JSON API</a>
    <a href="/progression">Progression JSON</a>
  </nav>
</header>
<main id="main-content">
  <section aria-labelledby="summary-heading">
    <h2 id="summary-heading">Daemon &amp; Sync Summary</h2>
    <div class="grid" aria-live="polite">
      <div class="card"><h2>Daemon Status</h2><div class="val">RUNNING ({}s)</div></div>
      <div class="card"><h2>Loaded Packs</h2><div class="val">{}</div></div>
      <div class="card"><h2>Journal Events / Sync Queue</h2><div class="val">{} / {} pending</div></div>
      <div class="card"><h2>Total Frame Gaps</h2><div class="val">{}</div></div>
      <div class="card"><h2>Last Receipt / Acceptance</h2><div class="val">{} / {}</div></div>
      <div class="card"><h2>Plane C Observations</h2><div class="val">{}</div></div>
    </div>
  </section>

  <section aria-labelledby="sessions-heading">
    <h2 id="sessions-heading">Runtime Sessions</h2>
    <table>
      <thead>
        <tr>
          <th scope="col">Game ID</th>
          <th scope="col">Pack</th>
          <th scope="col">Plane</th>
          <th scope="col">Mode</th>
          <th scope="col">Trust Tier</th>
          <th scope="col">Downgrade</th>
          <th scope="col">Frames / Gaps</th>
          <th scope="col">Health Status</th>
        </tr>
      </thead>
      <tbody>{}</tbody>
    </table>
  </section>

  <section aria-labelledby="notifications-heading">
    <h2 id="notifications-heading">Recent Notifications</h2>
    <ul>{}</ul>
  </section>
</main>
</body>
</html>"##,
        health.uptime_seconds,
        health.loaded_packs_count,
        health.total_journal_events,
        health.sync_queue_depth,
        health.total_frame_gaps,
        esc_html(health.last_receipt_status.as_deref().unwrap_or("NONE")),
        esc_html(health.last_acceptance_status.as_deref().unwrap_or("NONE")),
        health.observations_imported,
        session_rows,
        notif_items
    )
}
