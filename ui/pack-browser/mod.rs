use crate::packs::manifest::LoadedPack;

fn esc_html(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

pub fn render_pack_browser_html(packs: &[&LoadedPack], errors: &[(String, String)]) -> String {
    let mut rows = String::new();
    if packs.is_empty() {
        rows.push_str("<tr><td colspan=\"6\">No validated .onehundo packs loaded.</td></tr>");
    } else {
        for p in packs {
            let mut profiles = Vec::new();
            if p.manifest.profiles.memory.as_ref().map(|m| m.enabled).unwrap_or(false) {
                profiles.push("Plane A (Memory)");
            }
            if p.manifest.profiles.semantic.as_ref().map(|s| s.enabled).unwrap_or(false) {
                profiles.push("Plane B (Semantic)");
            }
            if p.manifest.profiles.observation.as_ref().map(|o| o.enabled).unwrap_or(false) {
                profiles.push("Plane C (Observation)");
            }
            let signed = if p.manifest.signature.is_some() {
                "Signed (Ed25519)"
            } else {
                "Unsigned / Local"
            };
            rows.push_str(&format!(
                "<tr>\
                 <td><code>{}</code></td>\
                 <td>{}</td>\
                 <td><code>{}</code></td>\
                 <td>{}</td>\
                 <td>{} achievements</td>\
                 <td>{}</td>\
                 </tr>",
                esc_html(&p.manifest.pack_id),
                esc_html(&p.manifest.title),
                esc_html(&p.manifest.canonical_game_id),
                esc_html(&profiles.join(", ")),
                p.manifest.achievements.len(),
                esc_html(signed),
            ));
        }
    }

    let mut err_html = String::new();
    if !errors.is_empty() {
        err_html.push_str("<section aria-labelledby=\"err-h\"><h2>Rejected / Invalid Packs</h2><ul>");
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
        r#"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>OneHundo Pack Browser</title>
<style>
  body {{ margin: 0; padding: 1.5rem; background: #0b0d12; color: #f8fafc; font-family: system-ui, sans-serif; line-height: 1.6; }}
  a {{ color: #7dd3fc; }}
  a:focus-visible {{ outline: 3px solid #fde047; outline-offset: 2px; }}
  nav {{ margin-bottom: 1.5rem; display: flex; gap: 1rem; }}
  nav a {{ padding: 0.4rem 0.8rem; border: 1px solid #475569; border-radius: 4px; text-decoration: none; font-weight: 600; }}
  table {{ width: 100%; border-collapse: collapse; background: #141821; }}
  th, td {{ border: 1px solid #475569; padding: 0.6rem; text-align: left; }}
  th {{ background: #1e2433; }}
</style>
</head>
<body>
<header>
  <h1>OneHundo Pack Browser</h1>
  <nav aria-label="Primary">
    <a href="/">Game Health</a>
    <a href="/ui/packs" aria-current="page">Pack Browser</a>
    <a href="/packs">Packs JSON</a>
  </nav>
</header>
<main>
  <section aria-labelledby="packs-h">
    <h2 id="packs-h">Discovered &amp; Security-Validated Packs</h2>
    <table>
      <thead>
        <tr>
          <th scope="col">Pack ID</th>
          <th scope="col">Title</th>
          <th scope="col">Canonical Game ID</th>
          <th scope="col">Enabled Planes</th>
          <th scope="col">Achievements</th>
          <th scope="col">Signature</th>
        </tr>
      </thead>
      <tbody>{}</tbody>
    </table>
  </section>
  {}
</main>
</body>
</html>"#,
        rows, err_html
    )
}
