use crate::core::types::ProgressionMode;
use crate::observation::external_importers::ObservationHistoryRow;
use crate::packs::manifest::LoadedPack;
use crate::storage::state::{StatRecord, TrophyRecord};
use std::collections::BTreeMap;

fn esc_html(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

pub fn render_pack_browser_html(packs: &[&LoadedPack], errors: &[(String, String)]) -> String {
    render_pack_browser_with_state_html(packs, errors, &[], &[], &[], ProgressionMode::Standard)
}

pub fn render_pack_browser_with_state_html(
    packs: &[&LoadedPack],
    errors: &[(String, String)],
    observations: &[ObservationHistoryRow],
    trophies: &[TrophyRecord],
    stats: &[StatRecord],
    active_mode: ProgressionMode,
) -> String {
    let mut rows = String::new();
    let mut detail_sections = String::new();
    if packs.is_empty() {
        rows.push_str(
            "<tr><td colspan=\"8\">No validated .onehundo packs loaded.</td></tr>",
        );
    } else {
        for p in packs {
            let sum = p.summary();
            let signed = if sum.signed {
                "Signed (Ed25519)"
            } else {
                "Unsigned / Local"
            };
            rows.push_str(&format!(
                "<tr>\
                 <td><code>{}</code></td>\
                 <td>{}</td>\
                 <td>{}</td>\
                 <td><code>{}</code></td>\
                 <td><code>{}</code></td>\
                 <td>{}</td>\
                 <td class=\"num\">{}</td>\
                 <td><code>{}</code><div class=\"meta\">{}</div></td>\
                 </tr>",
                esc_html(&sum.pack_id),
                esc_html(sum.pack_version.as_deref().unwrap_or("—")),
                esc_html(&sum.title),
                esc_html(&sum.canonical_game_id),
                esc_html(sum.mapped_oap_edition.as_deref().unwrap_or("Unmapped")),
                esc_html(&sum.enabled_planes.join(", ")),
                sum.achievement_count,
                esc_html(&sum.pack_sha256),
                esc_html(signed),
            ));

            // Map persistent trophies + journal observations for this pack's canonical_game_id
            let mut trophy_by_key: BTreeMap<&str, &TrophyRecord> = BTreeMap::new();
            for t in trophies {
                if t.canonical_game_id == sum.canonical_game_id {
                    trophy_by_key.entry(t.canonical_achievement_id.as_str()).or_insert(t);
                }
            }
            let mut obs_ach_by_key: BTreeMap<&str, &ObservationHistoryRow> = BTreeMap::new();
            let mut obs_stat_by_key: BTreeMap<&str, &ObservationHistoryRow> = BTreeMap::new();
            for o in observations {
                if o.canonical_game_id != sum.canonical_game_id {
                    continue;
                }
                if o.record_kind == "ACHIEVEMENT_EVIDENCE" && o.unlocked == Some(true) {
                    obs_ach_by_key.entry(o.key.as_str()).or_insert(o);
                } else if o.record_kind == "STAT_SNAPSHOT" {
                    obs_stat_by_key.entry(o.key.as_str()).or_insert(o);
                }
            }
            let mut stat_by_key: BTreeMap<&str, String> = BTreeMap::new();
            for s in stats {
                if s.canonical_game_id == sum.canonical_game_id {
                    let val = s
                        .int_value
                        .map(|v| v.to_string())
                        .or_else(|| s.float_value.map(|v| v.to_string()))
                        .or_else(|| s.string_value.clone())
                        .unwrap_or_else(|| "—".to_string());
                    stat_by_key.insert(s.stat_key.as_str(), format!("{} ({})", val, s.progression_mode));
                }
            }
            for (k, o) in &obs_stat_by_key {
                stat_by_key.entry(k).or_insert_with(|| {
                    let v = o
                        .int_value
                        .map(|n| n.to_string())
                        .unwrap_or_else(|| "—".to_string());
                    format!("{} (Observed · {})", v, o.progression_mode)
                });
            }

            let mut earned_or_observed = 0usize;
            let mut ach_rows = String::new();
            for ach in &p.manifest.achievements {
                let id = ach.canonical_achievement_id.as_str();
                let status_html = if let Some(t) = trophy_by_key.get(id) {
                    earned_or_observed += 1;
                    format!(
                        "<span class=\"badge badge--inverse\">Unlocked · {}</span> <span class=\"meta\">{} / {}</span>",
                        esc_html(&t.progression_mode),
                        esc_html(&t.trust_tier),
                        esc_html(&t.verification_state)
                    )
                } else if let Some(o) = obs_ach_by_key.get(id) {
                    earned_or_observed += 1;
                    format!(
                        "<span class=\"badge\">Observed · {}</span> <span class=\"meta\">{} / {}</span>",
                        esc_html(&o.progression_mode),
                        esc_html(&o.evidence_class),
                        esc_html(&o.verification_state)
                    )
                } else {
                    "<span class=\"meta\">Locked</span>".to_string()
                };
                ach_rows.push_str(&format!(
                    "<tr>\
                     <td><code>{}</code></td>\
                     <td><strong>{}</strong><div class=\"meta\">{}</div></td>\
                     <td class=\"num\">{}</td>\
                     <td>{}</td>\
                     </tr>",
                    esc_html(id),
                    esc_html(&ach.title),
                    esc_html(&ach.description),
                    ach.points,
                    status_html,
                ));
            }

            let mut stats_html = String::new();
            if !stat_by_key.is_empty() {
                stats_html.push_str("<div class=\"stats-row\">");
                for (k, v) in stat_by_key {
                    stats_html.push_str(&format!(
                        "<div class=\"stat-chip\"><span class=\"meta\"><code>{}</code></span> <strong>{}</strong></div>",
                        esc_html(k),
                        esc_html(&v),
                    ));
                }
                stats_html.push_str("</div>");
            }

            detail_sections.push_str(&format!(
                "<section class=\"pack-detail\" aria-labelledby=\"pack-{id}\">\
                 <h3 id=\"pack-{id}\">{title} (<code>{pack_id}</code>) · {done} / {total}</h3>\
                 {stats_html}\
                 <div class=\"table-wrap\">\
                   <table>\
                     <thead>\
                       <tr>\
                         <th scope=\"col\">ID</th>\
                         <th scope=\"col\">Achievement</th>\
                         <th scope=\"col\" class=\"num\">Points</th>\
                         <th scope=\"col\">Local Status</th>\
                       </tr>\
                     </thead>\
                     <tbody>{ach_rows}</tbody>\
                   </table>\
                 </div>\
                 </section>",
                id = esc_html(&sum.pack_id.replace('.', "-")),
                title = esc_html(&sum.title),
                pack_id = esc_html(&sum.pack_id),
                done = earned_or_observed,
                total = sum.achievement_count,
                stats_html = stats_html,
                ach_rows = ach_rows,
            ));
        }
    }

    let mut err_html = String::new();
    if !errors.is_empty() {
        err_html.push_str("<section aria-labelledby=\"err-h\"><h2 id=\"err-h\">Rejected / Invalid Packs</h2><ul class=\"err-list\">");
        for (path, err) in errors {
            err_html.push_str(&format!(
                "<li><code>{}</code>: {}</li>",
                esc_html(path),
                esc_html(err)
            ));
        }
        err_html.push_str("</ul></section>");
    }

    format!(
        r##"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>OneHundo Pack Browser</title>
<style>
  :root {{
    --paper: #0D0D0E;
    --surface: #17181A;
    --surface-2: #202124;
    --line: #2C2E31;
    --line-strong: #6E6F72;
    --ink: #EDEBE6;
    --ink-2: #A9A7A1;
    --error: #FF6B5E;
  }}
  * {{ box-sizing: border-box; border-radius: 0; }}
  body {{
    margin: 0;
    background: var(--paper);
    color: var(--ink);
    font-family: Inter, system-ui, -apple-system, sans-serif;
    line-height: 1.5;
  }}
  .wrap {{ max-width: 1440px; margin: 0 auto; padding: 1.5rem clamp(16px, 4vw, 64px) 3rem; }}
  a {{ color: var(--ink); text-decoration: underline; text-underline-offset: 0.2em; }}
  :focus-visible {{ outline: 2px solid var(--ink); outline-offset: 2px; box-shadow: 0 0 0 4px var(--paper); }}
  header {{
    border-bottom: 1px solid var(--line);
    padding-bottom: 1rem;
    margin-bottom: 1.5rem;
    display: flex;
    flex-wrap: wrap;
    justify-content: space-between;
    align-items: flex-end;
    gap: 1rem;
  }}
  h1 {{ margin: 0; font-family: Archivo, system-ui, sans-serif; font-weight: 800; font-size: 2rem; }}
  h2 {{ margin: 1.5rem 0 0.75rem; font-family: Archivo, system-ui, sans-serif; font-weight: 700; font-size: 1.35rem; }}
  h3 {{ margin: 1.5rem 0 0.65rem; font-family: Archivo, system-ui, sans-serif; font-weight: 700; font-size: 1.1rem; }}
  nav {{ display: flex; flex-wrap: wrap; gap: 0.75rem; }}
  nav a {{ display: inline-flex; align-items: center; min-height: 44px; padding: 0.4rem 0.85rem; border: 1px solid var(--line-strong); text-decoration: none; font-weight: 600; }}
  nav a[aria-current="page"] {{ background: var(--ink); color: var(--paper); border-color: var(--ink); }}
  .table-wrap {{ overflow-x: auto; border: 1px solid var(--line); background: var(--surface); }}
  table {{ width: 100%; border-collapse: collapse; font-variant-numeric: tabular-nums; }}
  th, td {{ border-bottom: 1px solid var(--line); padding: 0.65rem 0.75rem; text-align: left; vertical-align: top; }}
  th {{ background: var(--surface-2); color: var(--ink-2); font-size: 0.875rem; border-bottom: 1px solid var(--line-strong); }}
  td.num, th.num {{ text-align: right; }}
  .meta {{ color: var(--ink-2); font-size: 0.8125rem; margin-top: 0.2rem; }}
  .badge {{ display: inline-block; padding: 0.15rem 0.5rem; border: 1px solid var(--line-strong); font-size: 0.8125rem; font-weight: 600; }}
  .badge--inverse {{ background: var(--ink); color: var(--paper); border-color: var(--ink); }}
  .stats-row {{ display: flex; flex-wrap: wrap; gap: 0.75rem; margin-bottom: 0.75rem; }}
  .stat-chip {{ background: var(--surface); border: 1px solid var(--line-strong); padding: 0.45rem 0.75rem; font-variant-numeric: tabular-nums; }}
  .err-list {{ background: var(--surface); border: 1px solid var(--error); padding: 0.75rem 1.25rem; color: var(--error); }}
  code {{ font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace; font-size: 0.875rem; }}
</style>
</head>
<body>
<div class="wrap">
<header>
  <div>
    <h1>OneHundo Pack Browser</h1>
    <div class="meta">Active progression mode: <code>{mode}</code></div>
  </div>
  <nav aria-label="Primary">
    <a href="/">Game Health</a>
    <a href="/ui/packs" aria-current="page">Pack Browser</a>
    <a href="/health">Health JSON</a>
    <a href="/packs">Packs JSON</a>
  </nav>
</header>
<main>
  <section aria-labelledby="packs-h">
    <h2 id="packs-h">Installed &amp; Security-Validated .onehundo Packs</h2>
    <div class="table-wrap">
      <table>
        <thead>
          <tr>
            <th scope="col">Pack ID</th>
            <th scope="col">Version</th>
            <th scope="col">Title</th>
            <th scope="col">Canonical Game ID</th>
            <th scope="col">Mapped OAP Edition</th>
            <th scope="col">Enabled Planes</th>
            <th scope="col" class="num">Achievements</th>
            <th scope="col">SHA-256 &amp; Signature</th>
          </tr>
        </thead>
        <tbody>{rows}</tbody>
      </table>
    </div>
  </section>
  {detail_sections}
  {err_html}
</main>
</div>
</body>
</html>"##,
        mode = esc_html(active_mode.as_str()),
        rows = rows,
        detail_sections = detail_sections,
        err_html = err_html
    )
}
