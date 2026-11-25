use crate::packs::manifest::LoadedPack;

fn esc_html(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

pub fn render_pack_browser_html(packs: &[&LoadedPack], errors: &[(String, String)]) -> String {
    let mut rows = String::new();
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
  nav {{ display: flex; flex-wrap: wrap; gap: 0.75rem; }}
  nav a {{ display: inline-flex; align-items: center; min-height: 44px; padding: 0.4rem 0.85rem; border: 1px solid var(--line-strong); text-decoration: none; font-weight: 600; }}
  nav a[aria-current="page"] {{ background: var(--ink); color: var(--paper); border-color: var(--ink); }}
  .table-wrap {{ overflow-x: auto; border: 1px solid var(--line); background: var(--surface); }}
  table {{ width: 100%; border-collapse: collapse; font-variant-numeric: tabular-nums; }}
  th, td {{ border-bottom: 1px solid var(--line); padding: 0.65rem 0.75rem; text-align: left; vertical-align: top; }}
  th {{ background: var(--surface-2); color: var(--ink-2); font-size: 0.875rem; border-bottom: 1px solid var(--line-strong); }}
  td.num, th.num {{ text-align: right; }}
  .meta {{ color: var(--ink-2); font-size: 0.8125rem; margin-top: 0.2rem; }}
  .err-list {{ background: var(--surface); border: 1px solid var(--error); padding: 0.75rem 1.25rem; color: var(--error); }}
  code {{ font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace; font-size: 0.875rem; }}
</style>
</head>
<body>
<div class="wrap">
<header>
  <h1>OneHundo Pack Browser</h1>
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
  {err_html}
</main>
</div>
</body>
</html>"##,
        rows = rows,
        err_html = err_html
    )
}
