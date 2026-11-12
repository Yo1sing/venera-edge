use crate::{api::cookies::StoredCookie, cache::now_ms};
use anyhow::{Context, Result};
use rusqlite::{params, Connection};
use url::Url;

pub struct Cookies {
    db: Connection,
}

fn domain_matches(host: &str, domain: &str) -> bool {
    if let Some(domain) = domain.strip_prefix('.') {
        host == domain || host.ends_with(&format!(".{domain}"))
    } else {
        host == domain
    }
}

fn path_matches(request: &str, cookie: &str) -> bool {
    request == cookie
        || request.starts_with(cookie)
            && (cookie.ends_with('/') || request.as_bytes().get(cookie.len()) == Some(&b'/'))
}

impl Cookies {
    pub fn open(path: &str) -> Result<Self> {
        let db = Connection::open(path)?;
        db.busy_timeout(std::time::Duration::from_secs(5))?;
        db.execute_batch("CREATE TABLE IF NOT EXISTS cookies (name TEXT NOT NULL, value TEXT NOT NULL, domain TEXT NOT NULL, path TEXT, expires INTEGER, secure INTEGER, httpOnly INTEGER, PRIMARY KEY(name,domain,path));")?;
        Ok(Self { db })
    }

    pub fn save(&mut self, url: &str, cookies: Vec<StoredCookie>) -> Result<()> {
        let uri = Url::parse(url)?;
        let host = uri
            .host_str()
            .context("cookie URL has no host")?
            .to_ascii_lowercase();
        let transaction = self.db.transaction()?;
        for cookie in cookies {
            let domain = match cookie.domain {
                Some(domain) => {
                    let domain = domain.trim_start_matches('.').to_ascii_lowercase();
                    if domain.is_empty()
                        || !(host == domain || host.ends_with(&format!(".{domain}")))
                    {
                        continue;
                    }
                    format!(".{domain}")
                }
                None => host.clone(),
            };
            let path = cookie
                .path
                .filter(|p| p.starts_with('/'))
                .unwrap_or_else(|| {
                    uri.path()
                        .rsplit_once('/')
                        .map(|(p, _)| if p.is_empty() { "/" } else { p })
                        .unwrap_or("/")
                        .to_owned()
                });
            let expires = cookie
                .max_age
                .map(|seconds| now_ms().saturating_add(seconds.saturating_mul(1000)))
                .or(cookie.expires_ms);
            if expires.is_some_and(|t| t <= now_ms()) {
                transaction.execute(
                    "DELETE FROM cookies WHERE name=? AND domain=? AND path=?",
                    params![cookie.name, domain, path],
                )?;
                continue;
            }
            transaction.execute("INSERT OR REPLACE INTO cookies (name,value,domain,path,expires,secure,httpOnly) VALUES (?,?,?,?,?,?,?)", params![cookie.name,cookie.value,domain,path,expires,cookie.secure,cookie.http_only])?;
        }
        transaction.commit()?;
        Ok(())
    }

    pub fn save_headers(&mut self, url: &str, headers: Vec<String>) -> Result<()> {
        let cookies = headers
            .into_iter()
            .filter_map(|header| cookie::Cookie::parse(header).ok())
            .map(|cookie| StoredCookie {
                name: cookie.name().to_owned(),
                value: cookie.value().to_owned(),
                domain: cookie.domain().map(str::to_owned),
                path: cookie.path().map(str::to_owned),
                expires_ms: cookie
                    .expires_datetime()
                    .map(|d| (d.unix_timestamp_nanos() / 1_000_000) as i64),
                max_age: cookie.max_age().map(|d| d.whole_seconds()),
                secure: cookie.secure().unwrap_or(false),
                http_only: cookie.http_only().unwrap_or(false),
            })
            .collect();
        self.save(url, cookies)
    }

    pub fn load(&mut self, url: &str) -> Result<Vec<StoredCookie>> {
        let uri = Url::parse(url)?;
        let host = uri.host_str().context("cookie URL has no host")?;
        self.db.execute(
            "DELETE FROM cookies WHERE expires IS NOT NULL AND expires <= ?",
            [now_ms()],
        )?;
        let mut statement = self.db.prepare("SELECT name,value,domain,path,expires,secure,httpOnly FROM cookies ORDER BY length(path) DESC, rowid ASC")?;
        let rows = statement.query_map([], |r| {
            Ok(StoredCookie {
                name: r.get(0)?,
                value: r.get(1)?,
                domain: r.get(2)?,
                path: r.get(3)?,
                expires_ms: r.get(4)?,
                secure: r.get::<_, Option<bool>>(5)?.unwrap_or(false),
                http_only: r.get::<_, Option<bool>>(6)?.unwrap_or(false),
                max_age: None,
            })
        })?;
        Ok(rows
            .collect::<rusqlite::Result<Vec<_>>>()?
            .into_iter()
            .filter(|c| {
                domain_matches(host, c.domain.as_deref().unwrap_or(""))
                    && path_matches(uri.path(), c.path.as_deref().unwrap_or("/"))
                    && (!c.secure || uri.scheme() == "https")
            })
            .collect())
    }

    pub fn delete(&mut self, url: &str, name: Option<&str>) -> Result<()> {
        let uri = Url::parse(url)?;
        let host = uri.host_str().context("cookie URL has no host")?;
        let domains = {
            let mut statement = self.db.prepare("SELECT DISTINCT domain FROM cookies")?;
            let rows = statement.query_map([], |r| r.get::<_, String>(0))?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        };
        for domain in domains.into_iter().filter(|d| domain_matches(host, d)) {
            if let Some(name) = name {
                self.db.execute(
                    "DELETE FROM cookies WHERE domain=? AND name=? AND path=?",
                    params![domain, name, uri.path()],
                )?;
            } else {
                self.db
                    .execute("DELETE FROM cookies WHERE domain=?", [domain])?;
            }
        }
        Ok(())
    }

    pub fn clear(&mut self) -> Result<()> {
        self.db.execute("DELETE FROM cookies", [])?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cookie_scope_expiry_and_persistence() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("cookie.db");
        let path = path.to_str().unwrap();
        let mut jar = Cookies::open(path)?;
        jar.save_headers(
            "https://example.com/account/login",
            vec![
                "host=1; Path=/; Secure".into(),
                "shared=2; Domain=example.com; Path=/account; HttpOnly".into(),
                "evil=3; Domain=other.com; Path=/".into(),
                "expired=4; Max-Age=0".into(),
            ],
        )?;
        assert_eq!(jar.load("https://example.com/account")?.len(), 2);
        assert_eq!(jar.load("https://child.example.com/account")?.len(), 1);
        assert!(jar.load("https://child.example.com/accounting")?.is_empty());
        assert!(jar.load("http://example.com/")?.is_empty());
        drop(jar);
        let mut jar = Cookies::open(path)?;
        assert_eq!(jar.load("https://example.com/")?.len(), 1);
        jar.clear()?;
        assert!(jar.load("https://example.com/")?.is_empty());
        Ok(())
    }
}
