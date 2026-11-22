//! Notification transports: SMTP email/SMS, Pushover, Pushbullet, ntfy and
//! webhooks. Replaces the shell `alarm` script's msmtp/curl calls.

use crate::config::{PushConfig, PushProvider, SmtpConfig, SmtpSecurity};
use std::collections::BTreeMap;

pub type NResult = Result<String, String>;

/// Expand `$var` / `${var}` placeholders from `env`. Unknown variables expand
/// to an empty string (like the shell did). Backslashes that only escaped
/// shell syntax in old templates (`\(`, `\)`, `\<`, `\>`) are removed.
pub fn expand(template: &str, env: &BTreeMap<String, String>) -> String {
    let mut out = String::with_capacity(template.len());
    let chars: Vec<char> = template.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '\\' && i + 1 < chars.len() && "()<>$".contains(chars[i + 1]) {
            out.push(chars[i + 1]);
            i += 2;
            continue;
        }
        if c == '$' {
            let (name, next) = if chars.get(i + 1) == Some(&'{') {
                let end = chars[i + 2..].iter().position(|c| *c == '}').map(|p| i + 2 + p);
                match end {
                    Some(e) => (chars[i + 2..e].iter().collect::<String>(), e + 1),
                    None => (String::new(), i + 1),
                }
            } else {
                let mut j = i + 1;
                while j < chars.len() && (chars[j].is_ascii_alphanumeric() || chars[j] == '_') {
                    j += 1;
                }
                (chars[i + 1..j].iter().collect::<String>(), j)
            };
            if name.is_empty() {
                out.push('$');
                i += 1;
            } else {
                out.push_str(env.get(&name).map(String::as_str).unwrap_or(""));
                i = next;
            }
            continue;
        }
        out.push(c);
        i += 1;
    }
    out
}

pub fn smtp_configured(s: &SmtpConfig) -> bool {
    !s.host.is_empty()
}

pub async fn send_email(
    smtp: &SmtpConfig,
    from_name: &str,
    to: &str,
    to_name: &str,
    subject: &str,
    body: &str,
) -> NResult {
    use lettre::message::{Mailbox, header::ContentType};
    use lettre::transport::smtp::authentication::Credentials;
    use lettre::{AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};

    if !smtp_configured(smtp) {
        return Err("SMTP server not configured".into());
    }
    let from_addr = if smtp.from_address.is_empty() { smtp.username.clone() } else { smtp.from_address.clone() };
    let from: Mailbox = format!("{} <{}>", from_name, from_addr).parse().map_err(|e| format!("from address: {e}"))?;
    let to_mb: Mailbox = if to_name.is_empty() {
        to.parse().map_err(|e| format!("to address: {e}"))?
    } else {
        format!("{to_name} <{to}>").parse().map_err(|e| format!("to address: {e}"))?
    };
    let msg = Message::builder()
        .from(from)
        .to(to_mb)
        .subject(subject)
        .header(ContentType::TEXT_PLAIN)
        .body(body.to_string())
        .map_err(|e| e.to_string())?;
    let builder = match smtp.security {
        SmtpSecurity::Tls => AsyncSmtpTransport::<Tokio1Executor>::relay(&smtp.host).map_err(|e| e.to_string())?,
        SmtpSecurity::Starttls => {
            AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(&smtp.host).map_err(|e| e.to_string())?
        }
        SmtpSecurity::None => AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(&smtp.host),
    };
    let mut builder = builder.port(smtp.port).timeout(Some(std::time::Duration::from_secs(20)));
    if !smtp.username.is_empty() {
        builder = builder.credentials(Credentials::new(smtp.username.clone(), smtp.password.clone()));
    }
    builder.build().send(msg).await.map_err(|e| e.to_string())?;
    Ok(format!("email sent to {to}"))
}

async fn check(resp: Result<reqwest::Response, reqwest::Error>, what: &str) -> NResult {
    match resp {
        Ok(r) if r.status().is_success() => Ok(format!("{what} notification sent")),
        Ok(r) => {
            let st = r.status();
            let body = r.text().await.unwrap_or_default();
            Err(format!("{what} failed: {st} {}", body.chars().take(200).collect::<String>()))
        }
        Err(e) => Err(format!("{what} failed: {e}")),
    }
}

pub async fn send_push(
    http: &reqwest::Client,
    cfg: &PushConfig,
    env: &BTreeMap<String, String>,
    event: &serde_json::Value,
) -> NResult {
    let title = expand(&cfg.title, env);
    let message = expand(&cfg.message, env);
    match cfg.provider {
        PushProvider::None => Err("no push provider configured".into()),
        PushProvider::Pushover => {
            if cfg.pushover_user.is_empty() || cfg.pushover_token.is_empty() {
                return Err("Pushover user or token missing".into());
            }
            let form = [
                ("token", cfg.pushover_token.as_str()),
                ("user", cfg.pushover_user.as_str()),
                ("sound", cfg.pushover_sound.as_str()),
                ("title", title.as_str()),
                ("message", message.as_str()),
            ];
            check(http.post("https://api.pushover.net/1/messages.json").form(&form).send().await, "Pushover").await
        }
        PushProvider::Pushbullet => {
            if cfg.pushbullet_key.is_empty() {
                return Err("Pushbullet key missing".into());
            }
            let body = serde_json::json!({ "type": "note", "title": title, "body": message });
            check(
                http.post("https://api.pushbullet.com/v2/pushes")
                    .basic_auth(&cfg.pushbullet_key, Some(""))
                    .json(&body)
                    .send()
                    .await,
                "Pushbullet",
            )
            .await
        }
        PushProvider::Ntfy => {
            if cfg.ntfy_url.is_empty() {
                return Err("ntfy topic URL missing".into());
            }
            let mut req = http
                .post(&cfg.ntfy_url)
                .header("Title", title)
                .header("Priority", "high")
                .header("Tags", "fire")
                .body(message);
            if !cfg.ntfy_token.is_empty() {
                req = req.bearer_auth(&cfg.ntfy_token);
            }
            check(req.send().await, "ntfy").await
        }
        PushProvider::Webhook => {
            if cfg.webhook_url.is_empty() {
                return Err("webhook URL missing".into());
            }
            let body = serde_json::json!({ "title": title, "message": message, "event": event, "vars": env });
            check(http.post(&cfg.webhook_url).json(&body).send().await, "Webhook").await
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expansion() {
        let mut env = BTreeMap::new();
        env.insert("pn".to_string(), "Pit".to_string());
        env.insert("al_thresh".to_string(), "250".to_string());
        assert_eq!(expand("$pn above ${al_thresh}!", &env), "Pit above 250!");
        assert_eq!(expand("$pn Alarm \\($al_type\\) $", &env), "Pit Alarm () $");
        assert_eq!(expand("cost $5", &env), "cost ");
        assert_eq!(expand("$(rm -rf /)", &env), "$(rm -rf /)");
    }
}
