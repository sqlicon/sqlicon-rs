//! `sqlhosts` file parsing and lookup.
//!
//! Ports libsqli's `sqli_parse_sqlhosts`/`sqli_find_sqlhosts_entry`
//! (`libsqli/src/sqli_sqlhosts.c`): a standalone utility, not something libsqli's own
//! `sqli_connect`/`sqli_connect_uri` ever calls automatically — callers resolve a
//! `dbservername` themselves and feed the result into their own connect parameters.
//! `ConnectParams::from_sqlhosts` below is sqlicon-rs's convenience wrapper for exactly
//! that pattern.

use crate::error::{Error, Result};

/// Maximum number of entries read from a single sqlhosts file, matching libsqli's
/// `MAX_ENTRIES`. Extra lines beyond this are silently ignored, as in the C parser.
const MAX_ENTRIES: usize = 256;

/// A single entry from an Informix `sqlhosts` file:
/// `dbservername  nettype  hostname  servicename  [options...]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SqlhostsEntry {
    pub server_name: String,
    /// e.g. "onsoctcp", "onsocssl", "onipcstr", "tliv2".
    pub protocol: String,
    pub hostname: String,
    pub service: String,
    /// Raw trailing text after the 4 required fields, if any (e.g. group/options tokens).
    pub options: String,
}

fn trim(s: &str) -> &str {
    s.trim_matches(|c: char| c == ' ' || c == '\t' || c == '\r' || c == '\n')
}

/// Resolves the sqlhosts file path the same way libsqli does: the explicit `filepath` if
/// given, else `$INFORMIXDIR/etc/sqlhosts`, else `/etc/sqlhosts`.
fn resolve_path(filepath: Option<&str>) -> String {
    if let Some(p) = filepath {
        return p.to_string();
    }
    match std::env::var("INFORMIXDIR") {
        Ok(dir) => format!("{dir}/etc/sqlhosts"),
        Err(_) => "/etc/sqlhosts".to_string(),
    }
}

/// Parses an Informix `sqlhosts` file into a list of entries.
///
/// Matches `sqli_parse_sqlhosts`: comment lines (`#`) and include directives (`@...`) are
/// skipped (the latter are not actually followed, same as the C implementation), blank
/// lines are skipped, and a data line needs at least 4 whitespace-separated fields
/// (`server_name protocol hostname service`); anything after the 4th field is captured
/// verbatim as `options`. A malformed line (fewer than 4 fields) is skipped with a
/// warning, same as C's `sqli_log(SQLI_LOG_WARN, ...)` path. A missing file is **not**
/// an error — it returns an empty list, matching C's "no file is not an error" comment.
pub fn parse_sqlhosts(filepath: Option<&str>) -> Result<Vec<SqlhostsEntry>> {
    let path = resolve_path(filepath);

    let meta = match std::fs::metadata(&path) {
        Ok(m) => m,
        Err(_) => return Ok(Vec::new()),
    };
    if !meta.is_file() {
        return Ok(Vec::new());
    }

    let contents = std::fs::read_to_string(&path).map_err(Error::Io)?;

    let mut entries = Vec::new();
    for line in contents.lines() {
        if entries.len() >= MAX_ENTRIES {
            break;
        }
        let s = trim(line);
        if s.is_empty() || s.starts_with('#') || s.starts_with('@') {
            continue;
        }

        let mut fields = s.split_whitespace();
        let server_name = fields.next();
        let protocol = fields.next();
        let hostname = fields.next();
        let service = fields.next();
        let (server_name, protocol, hostname, service) =
            match (server_name, protocol, hostname, service) {
                (Some(a), Some(b), Some(c), Some(d)) => (a, b, c, d),
                _ => {
                    tracing::warn!("sqlhosts: malformed line: {line}");
                    continue;
                }
            };

        // Whatever remains after the 4 required fields, re-joined as libsqli's
        // `%255[^\n]` captures the rest of the line verbatim (not just the next token).
        let rest_offset = {
            // Find where `service`'s slice ends within `s` so we can slice the remainder.
            let service_ptr = service.as_ptr() as usize;
            let s_ptr = s.as_ptr() as usize;
            (service_ptr - s_ptr) + service.len()
        };
        let options = trim(&s[rest_offset..]).to_string();

        entries.push(SqlhostsEntry {
            server_name: server_name.to_string(),
            protocol: protocol.to_string(),
            hostname: hostname.to_string(),
            service: service.to_string(),
            options,
        });
    }

    tracing::debug!("sqlhosts: parsed {} entries from {}", entries.len(), path);
    Ok(entries)
}

/// Finds the first entry whose `server_name` case-insensitively matches `server_name`,
/// matching `sqli_find_sqlhosts_entry`'s linear `strcasecmp` scan (first match wins).
pub fn find_sqlhosts_entry<'a>(
    entries: &'a [SqlhostsEntry],
    server_name: &str,
) -> Option<&'a SqlhostsEntry> {
    entries
        .iter()
        .find(|e| e.server_name.eq_ignore_ascii_case(server_name))
}

/// Convenience: parse `filepath` (or the default sqlhosts location) and look up
/// `server_name` in one call.
pub fn lookup(filepath: Option<&str>, server_name: &str) -> Result<Option<SqlhostsEntry>> {
    let entries = parse_sqlhosts(filepath)?;
    Ok(find_sqlhosts_entry(&entries, server_name).cloned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_temp(contents: &str) -> std::path::PathBuf {
        let mut path = std::env::temp_dir();
        path.push(format!(
            "sqlicon_sqlhosts_test_{}_{}.tmp",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(contents.as_bytes()).unwrap();
        path
    }

    #[test]
    fn parses_multiple_entries() {
        let path = write_temp(
            "# comment\n\
             ol_tcp onsoctcp 127.0.0.1 9088\n\
             \n\
             ol_ssl\tonsocssl\thost.example.com\t9089\n\
             @include /etc/other\n\
             ol_ipc onipcstr unused /tmp/.sock\n",
        );
        let entries = parse_sqlhosts(Some(path.to_str().unwrap())).unwrap();
        std::fs::remove_file(&path).ok();

        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].server_name, "ol_tcp");
        assert_eq!(entries[0].protocol, "onsoctcp");
        assert_eq!(entries[0].hostname, "127.0.0.1");
        assert_eq!(entries[0].service, "9088");
        assert_eq!(entries[1].server_name, "ol_ssl");
        assert_eq!(entries[1].hostname, "host.example.com");
        assert_eq!(entries[2].protocol, "onipcstr");
    }

    #[test]
    fn trailing_options_are_captured() {
        let path = write_temp("ol_opt onsoctcp 127.0.0.1 9088 g_base client=1\n");
        let entries = parse_sqlhosts(Some(path.to_str().unwrap())).unwrap();
        std::fs::remove_file(&path).ok();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].options, "g_base client=1");
    }

    #[test]
    fn malformed_line_is_skipped() {
        let path = write_temp("too_few_fields onsoctcp\nol_ok onsoctcp 127.0.0.1 9088\n");
        let entries = parse_sqlhosts(Some(path.to_str().unwrap())).unwrap();
        std::fs::remove_file(&path).ok();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].server_name, "ol_ok");
    }

    #[test]
    fn missing_file_is_not_an_error() {
        let entries = parse_sqlhosts(Some("/nonexistent/path/does/not/exist/sqlhosts")).unwrap();
        assert!(entries.is_empty());
    }

    #[test]
    fn lookup_is_case_insensitive_and_first_match_wins() {
        let path = write_temp(
            "OL_Dup onsoctcp host1 1\n\
             ol_dup onsoctcp host2 2\n",
        );
        let entries = parse_sqlhosts(Some(path.to_str().unwrap())).unwrap();
        std::fs::remove_file(&path).ok();

        let found = find_sqlhosts_entry(&entries, "ol_DUP").unwrap();
        assert_eq!(found.hostname, "host1");
    }

    #[test]
    fn lookup_not_found_returns_none() {
        let path = write_temp("ol_tcp onsoctcp 127.0.0.1 9088\n");
        let entries = parse_sqlhosts(Some(path.to_str().unwrap())).unwrap();
        std::fs::remove_file(&path).ok();

        assert!(find_sqlhosts_entry(&entries, "unknown_server").is_none());
    }

    #[test]
    fn default_path_uses_informixdir_env_var() {
        // resolve_path is private; exercise it indirectly via a nonexistent INFORMIXDIR
        // to confirm no panic and an empty (not erroring) result.
        let prev = std::env::var("INFORMIXDIR").ok();
        unsafe {
            std::env::set_var("INFORMIXDIR", "/nonexistent/informixdir_for_test");
        }
        let entries = parse_sqlhosts(None).unwrap();
        assert!(entries.is_empty());
        unsafe {
            match prev {
                Some(v) => std::env::set_var("INFORMIXDIR", v),
                None => std::env::remove_var("INFORMIXDIR"),
            }
        }
    }
}
