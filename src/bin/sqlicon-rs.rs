//! sqlicon-rs - command-line interface and interactive shell for Informix.

use sqlicon_rs::{
    ConnectParams, Connection, Profile, ProfileStore, QueryResult,
};
use std::env;
use std::fs::{self, OpenOptions};
use std::io::{self, BufRead, IsTerminal, Read, Write};
use std::path::Path;
use std::process;
use std::time::Instant;

fn print_help() {
    println!(
        r#"sqlicon-rs - interactive shell for Informix (Rust)

Usage:
  sqlicon-rs [options]

Mode options:
  -c, --command <sql>      Execute inline SQL and exit
  -f, --file <path>        Execute script file and exit
      --finderr <code>     Look up Informix error code description and exit
  -h, --help               Show this help and exit
  -V, --version            Show the version and exit

Connection/profile options:
  -p, --profile <name>     Use named connection profile
      --profile-list       List available profiles
      --profile-show <n>   Show profile (masked password)
      --profile-test <n>   Test profile connection and exit
      --profile-create <n> Create profile from given connection flags
      --profile-update <n> Update profile fields from given flags
      --profile-delete <n> Delete profile (requires --yes)
      --profile-default <n> Set default profile name
      --show-secret        Show cleartext password in --profile-show
      --yes                Confirm destructive action (--profile-delete)
      --connect-uri <uri>  Connection URI (onsoctcp/onsocssl/onipcstr)
      --host <name>        Informix host name or IP
      --port <value>       Informix service/port
      --server <name>      Informix server name
      --database, --db <n> Database name
      --user <name>        User name
      --password <value>   Password
      --client-locale <v>  Client locale
      --db-locale <v>      Database locale

Environment (connection operations):
  SQLI_HOST, SQLI_PORT, SQLI_SERVER, SQLI_DATABASE (or SQLI_DB),
  SQLI_USER, SQLI_PASSWORD, SQLI_CLIENT_LOCALE, SQLI_DB_LOCALE
  Fallbacks: INFORMIXSERVER, CLIENT_LOCALE, DB_LOCALE
"#
    );
}

#[derive(Default, Debug)]
struct CliArgs {
    command: Option<String>,
    file: Option<String>,
    finderr: Option<i32>,
    connect_uri: Option<String>,
    profile: Option<String>,
    profile_list: bool,
    profile_show: Option<String>,
    profile_test: Option<String>,
    profile_create: Option<String>,
    profile_update: Option<String>,
    profile_delete: Option<String>,
    profile_default: Option<String>,
    show_secret: bool,
    yes: bool,

    host: Option<String>,
    port: Option<String>,
    server: Option<String>,
    database: Option<String>,
    user: Option<String>,
    password: Option<String>,
    client_locale: Option<String>,
    db_locale: Option<String>,
}

fn parse_cli_args() -> Result<CliArgs, String> {
    let mut args = CliArgs::default();
    let mut iter = env::args().skip(1);

    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "-h" | "--help" => {
                print_help();
                process::exit(0);
            }
            "-V" | "--version" => {
                println!("sqlicon-rs {}", env!("CARGO_PKG_VERSION"));
                process::exit(0);
            }
            "-c" | "--command" => {
                args.command = Some(iter.next().ok_or("Missing argument for --command")?);
            }
            "-f" | "--file" => {
                args.file = Some(iter.next().ok_or("Missing argument for --file")?);
            }
            "--finderr" => {
                let code_str = iter.next().ok_or("Missing argument for --finderr")?;
                let code: i32 = code_str.parse().map_err(|_| {
                    format!("Invalid error code '{code_str}': must be an integer")
                })?;
                args.finderr = Some(code);
            }
            "--connect-uri" => {
                args.connect_uri = Some(iter.next().ok_or("Missing argument for --connect-uri")?);
            }
            "-p" | "--profile" => {
                args.profile = Some(iter.next().ok_or("Missing argument for --profile")?);
            }
            "--profile-list" => args.profile_list = true,
            "--profile-show" => {
                args.profile_show = Some(iter.next().ok_or("Missing argument for --profile-show")?);
            }
            "--profile-test" => {
                args.profile_test = Some(iter.next().ok_or("Missing argument for --profile-test")?);
            }
            "--profile-create" => {
                args.profile_create = Some(iter.next().ok_or("Missing argument for --profile-create")?);
            }
            "--profile-update" => {
                args.profile_update = Some(iter.next().ok_or("Missing argument for --profile-update")?);
            }
            "--profile-delete" => {
                args.profile_delete = Some(iter.next().ok_or("Missing argument for --profile-delete")?);
            }
            "--profile-default" => {
                args.profile_default = Some(iter.next().ok_or("Missing argument for --profile-default")?);
            }
            "--show-secret" => args.show_secret = true,
            "--yes" => args.yes = true,

            "--host" => args.host = Some(iter.next().ok_or("Missing argument for --host")?),
            "--port" => args.port = Some(iter.next().ok_or("Missing argument for --port")?),
            "--server" => args.server = Some(iter.next().ok_or("Missing argument for --server")?),
            "--database" | "--db" => {
                args.database = Some(iter.next().ok_or("Missing argument for --database")?);
            }
            "--user" => args.user = Some(iter.next().ok_or("Missing argument for --user")?),
            "--password" => args.password = Some(iter.next().ok_or("Missing argument for --password")?),
            "--client-locale" => {
                args.client_locale = Some(iter.next().ok_or("Missing argument for --client-locale")?);
            }
            "--db-locale" => {
                args.db_locale = Some(iter.next().ok_or("Missing argument for --db-locale")?);
            }
            other => {
                if let Some(val) = other.strip_prefix("--command=") {
                    args.command = Some(val.to_string());
                } else if let Some(val) = other.strip_prefix("--file=") {
                    args.file = Some(val.to_string());
                } else if let Some(val) = other.strip_prefix("--finderr=") {
                    let code: i32 = val.parse().map_err(|_| {
                        format!("Invalid error code '{val}': must be an integer")
                    })?;
                    args.finderr = Some(code);
                } else if let Some(val) = other.strip_prefix("--connect-uri=") {
                    args.connect_uri = Some(val.to_string());
                } else if let Some(val) = other.strip_prefix("--profile=") {
                    args.profile = Some(val.to_string());
                } else if let Some(val) = other.strip_prefix("--host=") {
                    args.host = Some(val.to_string());
                } else if let Some(val) = other.strip_prefix("--port=") {
                    args.port = Some(val.to_string());
                } else if let Some(val) = other.strip_prefix("--server=") {
                    args.server = Some(val.to_string());
                } else if let Some(val) = other.strip_prefix("--database=") {
                    args.database = Some(val.to_string());
                } else if let Some(val) = other.strip_prefix("--db=") {
                    args.database = Some(val.to_string());
                } else if let Some(val) = other.strip_prefix("--user=") {
                    args.user = Some(val.to_string());
                } else if let Some(val) = other.strip_prefix("--password=") {
                    args.password = Some(val.to_string());
                } else if let Some(val) = other.strip_prefix("--client-locale=") {
                    args.client_locale = Some(val.to_string());
                } else if let Some(val) = other.strip_prefix("--db-locale=") {
                    args.db_locale = Some(val.to_string());
                } else {
                    return Err(format!("Unknown option: {other}"));
                }
            }
        }
    }

    Ok(args)
}

fn build_connect_params(args: &CliArgs, store: &ProfileStore) -> Result<ConnectParams, String> {
    if let Some(ref uri) = args.connect_uri {
        return ConnectParams::from_uri(uri).map_err(|e| e.to_string());
    }

    let profile_target = args
        .profile_test
        .as_deref()
        .or(args.profile.as_deref())
        .or(store.default_name.as_deref());
    let prof = profile_target.and_then(|name| store.get(name));

    let host = args
        .host
        .clone()
        .or_else(|| env::var("SQLI_HOST").ok())
        .or_else(|| prof.and_then(|p| p.host.clone()))
        .unwrap_or_else(|| "127.0.0.1".into());

    let port = args
        .port
        .clone()
        .or_else(|| env::var("SQLI_PORT").ok())
        .or_else(|| prof.and_then(|p| p.port.clone()))
        .unwrap_or_else(|| "9088".into());

    let server = args
        .server
        .clone()
        .or_else(|| env::var("SQLI_SERVER").ok())
        .or_else(|| env::var("INFORMIXSERVER").ok())
        .or_else(|| prof.and_then(|p| p.server.clone()))
        .ok_or("Informix server name not specified (use --server, SQLI_SERVER, or a profile)")?;

    let database = args
        .database
        .clone()
        .or_else(|| env::var("SQLI_DATABASE").ok())
        .or_else(|| env::var("SQLI_DB").ok())
        .or_else(|| prof.and_then(|p| p.database.clone()));

    let user = args
        .user
        .clone()
        .or_else(|| env::var("SQLI_USER").ok())
        .or_else(|| prof.and_then(|p| p.user.clone()));

    let password = args
        .password
        .clone()
        .or_else(|| env::var("SQLI_PASSWORD").ok())
        .or_else(|| prof.and_then(|p| p.password.clone()));

    let client_locale = args
        .client_locale
        .clone()
        .or_else(|| env::var("SQLI_CLIENT_LOCALE").ok())
        .or_else(|| env::var("CLIENT_LOCALE").ok())
        .or_else(|| prof.and_then(|p| p.client_locale.clone()))
        .unwrap_or_else(|| "en_US.utf8".into());

    let db_locale = args
        .db_locale
        .clone()
        .or_else(|| env::var("SQLI_DB_LOCALE").ok())
        .or_else(|| env::var("DB_LOCALE").ok())
        .or_else(|| prof.and_then(|p| p.db_locale.clone()))
        .unwrap_or_else(|| "en_US.8859-1".into());

    let mut params = ConnectParams::new(host, port, server);
    if let Some(db) = database {
        params = params.database(db);
    }
    if let Some(u) = user {
        let p = password.unwrap_or_default();
        params = params.credentials(u, p);
    }
    params = params.locales(client_locale, db_locale);

    Ok(params)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OutputMode {
    Aligned,
    Csv,
    Line,
    Json,
    Markdown,
}

impl OutputMode {
    fn name(&self) -> &'static str {
        match self {
            Self::Aligned => "aligned",
            Self::Csv => "csv",
            Self::Line => "line",
            Self::Json => "json",
            Self::Markdown => "markdown",
        }
    }
}

struct ShellRuntime {
    mode: OutputMode,
    headers: bool,
    bail_on_error: bool,
    timer_on: bool,
    null_value: String,
    output_path: Option<String>,
    once_path: Option<String>,
    width_overrides: Vec<usize>,
    read_depth: usize,
    conn_profile: Option<String>,
}

impl Default for ShellRuntime {
    fn default() -> Self {
        Self {
            mode: OutputMode::Aligned,
            headers: true,
            bail_on_error: false,
            timer_on: false,
            null_value: "NULL".into(),
            output_path: None,
            once_path: None,
            width_overrides: Vec::new(),
            read_depth: 0,
            conn_profile: None,
        }
    }
}

impl ShellRuntime {
    fn get_writer(&mut self) -> Result<Box<dyn Write>, io::Error> {
        if let Some(path) = self.once_path.take() {
            let file = OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)?;
            Ok(Box::new(file))
        } else if let Some(ref path) = self.output_path {
            let file = OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)?;
            Ok(Box::new(file))
        } else {
            Ok(Box::new(io::stdout()))
        }
    }
}

fn escape_csv_value(s: &str) -> String {
    if s.contains(',') || s.contains('"') || s.contains('\n') || s.contains('\r') {
        let escaped = s.replace('"', "\"\"");
        format!("\"{escaped}\"")
    } else {
        s.to_string()
    }
}

fn escape_json_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\x08' => out.push_str("\\b"),
            '\x0C' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn escape_markdown_cell(s: &str) -> String {
    s.replace('|', "\\|").replace(['\n', '\r'], "<br>")
}

fn render_result(out: &mut dyn Write, rt: &ShellRuntime, result: &QueryResult) -> io::Result<()> {
    if result.columns.is_empty() {
        writeln!(out, "{} row(s) affected", result.rows_affected)?;
        return Ok(());
    }

    match rt.mode {
        OutputMode::Aligned => {
            let mut col_widths: Vec<usize> = result
                .columns
                .iter()
                .map(|c| c.name.len())
                .collect();

            // Apply manual width overrides
            for (idx, &w) in rt.width_overrides.iter().enumerate() {
                if idx < col_widths.len() && w > 0 {
                    col_widths[idx] = col_widths[idx].max(w);
                }
            }

            let mut formatted_rows = Vec::with_capacity(result.rows.len());
            for row in &result.rows {
                let mut row_strs = Vec::with_capacity(result.columns.len());
                for (col_idx, _) in result.columns.iter().enumerate() {
                    let val_str = if row.is_null(col_idx) {
                        rt.null_value.clone()
                    } else {
                        row.get_display_value(col_idx).unwrap_or_else(|_| rt.null_value.clone())
                    };
                    col_widths[col_idx] = col_widths[col_idx].max(val_str.len());
                    row_strs.push(val_str);
                }
                formatted_rows.push(row_strs);
            }

            if rt.headers {
                let header_line = result
                    .columns
                    .iter()
                    .enumerate()
                    .map(|(i, c)| format!("{:<width$}", c.name, width = col_widths[i]))
                    .collect::<Vec<_>>()
                    .join(" | ");
                writeln!(out, "{header_line}")?;

                let sep_line = col_widths
                    .iter()
                    .map(|&w| "-".repeat(w))
                    .collect::<Vec<_>>()
                    .join("-+-");
                writeln!(out, "{sep_line}")?;
            }

            for row in formatted_rows {
                let row_line = row
                    .iter()
                    .enumerate()
                    .map(|(i, val)| format!("{:<width$}", val, width = col_widths[i]))
                    .collect::<Vec<_>>()
                    .join(" | ");
                writeln!(out, "{row_line}")?;
            }

            writeln!(out, "{} row(s) affected", result.rows.len().max(result.rows_affected as usize))?;
        }
        OutputMode::Csv => {
            if rt.headers {
                let hdr = result
                    .columns
                    .iter()
                    .map(|c| escape_csv_value(&c.name))
                    .collect::<Vec<_>>()
                    .join(",");
                writeln!(out, "{hdr}")?;
            }
            for row in &result.rows {
                let mut line_parts = Vec::with_capacity(result.columns.len());
                for (col_idx, _) in result.columns.iter().enumerate() {
                    if row.is_null(col_idx) {
                        line_parts.push(rt.null_value.clone());
                    } else {
                        let s = row.get_display_value(col_idx).unwrap_or_default();
                        line_parts.push(escape_csv_value(&s));
                    }
                }
                writeln!(out, "{}", line_parts.join(","))?;
            }
        }
        OutputMode::Line => {
            for row in &result.rows {
                for (col_idx, col) in result.columns.iter().enumerate() {
                    let val = if row.is_null(col_idx) {
                        &rt.null_value
                    } else {
                        &row.get_display_value(col_idx).unwrap_or_default()
                    };
                    writeln!(out, "{} = {}", col.name, val)?;
                }
                writeln!(out)?;
            }
        }
        OutputMode::Json => {
            write!(out, "[")?;
            for (r_idx, row) in result.rows.iter().enumerate() {
                if r_idx > 0 {
                    write!(out, ",")?;
                }
                write!(out, "{{")?;
                for (c_idx, col) in result.columns.iter().enumerate() {
                    if c_idx > 0 {
                        write!(out, ",")?;
                    }
                    write!(out, "{}:", escape_json_string(&col.name))?;
                    if row.is_null(c_idx) {
                        write!(out, "null")?;
                    } else {
                        let s = row.get_display_value(c_idx).unwrap_or_default();
                        write!(out, "{}", escape_json_string(&s))?;
                    }
                }
                write!(out, "}}")?;
            }
            writeln!(out, "]")?;
        }
        OutputMode::Markdown => {
            if rt.headers {
                let mut hdr_parts = Vec::with_capacity(result.columns.len());
                let mut sep_parts = Vec::with_capacity(result.columns.len());
                for col in &result.columns {
                    hdr_parts.push(escape_markdown_cell(&col.name));
                    sep_parts.push("---".to_string());
                }
                writeln!(out, "| {} |", hdr_parts.join(" | "))?;
                writeln!(out, "| {} |", sep_parts.join(" | "))?;
            }
            for row in &result.rows {
                let mut cells = Vec::with_capacity(result.columns.len());
                for (col_idx, _) in result.columns.iter().enumerate() {
                    if row.is_null(col_idx) {
                        cells.push(escape_markdown_cell(&rt.null_value));
                    } else {
                        let s = row.get_display_value(col_idx).unwrap_or_default();
                        cells.push(escape_markdown_cell(&s));
                    }
                }
                writeln!(out, "| {} |", cells.join(" | "))?;
            }
        }
    }

    Ok(())
}

fn is_query_sql(sql: &str) -> bool {
    let s = sql.trim_start();
    let first_word = s.split_whitespace().next().unwrap_or("").to_uppercase();
    matches!(
        first_word.as_str(),
        "SELECT" | "WITH" | "SHOW" | "DESCRIBE" | "EXPLAIN" | "VALUES"
    )
}

async fn execute_statement(
    conn: &mut Connection,
    rt: &mut ShellRuntime,
    sql: &str,
) -> Result<(), sqlicon_rs::Error> {
    let trimmed = sql.trim();
    if trimmed.is_empty() {
        return Ok(());
    }

    let start = Instant::now();
    let mut writer = match rt.get_writer() {
        Ok(w) => w,
        Err(e) => {
            eprintln!("error: cannot open output destination: {e}");
            return Ok(());
        }
    };

    if is_query_sql(trimmed) {
        let res = conn.query(trimmed).await?;
        let elapsed = start.elapsed();
        if let Err(e) = render_result(&mut *writer, rt, &res) {
            eprintln!("error: write failed: {e}");
        }
        if rt.timer_on {
            eprintln!("Run Time: {:0.3}s", elapsed.as_secs_f64());
        }
    } else {
        let affected = conn.execute(trimmed).await?;
        let elapsed = start.elapsed();
        let _ = writeln!(writer, "{affected} row(s) affected");
        if rt.timer_on {
            eprintln!("Run Time: {:0.3}s", elapsed.as_secs_f64());
        }
    }

    Ok(())
}

async fn execute_script(
    conn: &mut Connection,
    rt: &mut ShellRuntime,
    script: &str,
) -> Result<(), sqlicon_rs::Error> {
    let mut pending = String::new();
    for line in script.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with("--") || trimmed.starts_with('#') {
            continue;
        }

        if pending.trim().is_empty() && trimmed.starts_with('.') {
            let _ = handle_dot_command(conn, rt, trimmed).await;
            continue;
        }

        if !pending.is_empty() {
            pending.push(' ');
        }
        pending.push_str(trimmed);

        while let Some(semi_pos) = pending.find(';') {
            let stmt = pending[..semi_pos].trim().to_string();
            pending = pending[semi_pos + 1..].to_string();
            if !stmt.is_empty() {
                if stmt.starts_with('.') {
                    let _ = handle_dot_command(conn, rt, &stmt).await;
                } else if let Err(e) = execute_statement(conn, rt, &stmt).await {
                    eprintln!("error: {e}");
                    if rt.bail_on_error {
                        return Err(e);
                    }
                }
            }
        }
    }

    let remainder = pending.trim();
    if !remainder.is_empty() {
        if remainder.starts_with('.') {
            let _ = handle_dot_command(conn, rt, remainder).await;
        } else if let Err(e) = execute_statement(conn, rt, remainder).await {
            eprintln!("error: {e}");
            if rt.bail_on_error {
                return Err(e);
            }
        }
    }

    Ok(())
}

fn format_sql_type(coltype: i32, collength: i32) -> String {
    let base = coltype & 0xFF;
    match base {
        0 => format!("CHAR({collength})"),
        1 => "SMALLINT".into(),
        2 => "INTEGER".into(),
        3 => "FLOAT".into(),
        4 => "SMALLFLOAT".into(),
        5 => {
            let m = collength / 256;
            let n = collength % 256;
            if n == 255 || n == 0 {
                format!("DECIMAL({m})")
            } else {
                format!("DECIMAL({m},{n})")
            }
        }
        6 => "SERIAL".into(),
        7 => "DATE".into(),
        8 => {
            let m = collength / 256;
            let n = collength % 256;
            if n == 255 || n == 0 {
                format!("MONEY({m})")
            } else {
                format!("MONEY({m},{n})")
            }
        }
        9 => "NULL".into(),
        10 => {
            let hi = (collength >> 8) & 0xFF;
            let lo = collength & 0xFF;
            let dt_units = [
                "YEAR", "", "MONTH", "", "DAY", "", "HOUR", "", "MINUTE", "",
                "SECOND", "FRACTION(1)", "FRACTION", "FRACTION", "FRACTION(4)", "FRACTION(5)",
            ];
            let hi_unit = if (0..dt_units.len()).contains(&(hi as usize)) && !dt_units[hi as usize].is_empty() {
                dt_units[hi as usize]
            } else {
                "UNKNOWN"
            };
            let lo_unit = if (0..dt_units.len()).contains(&(lo as usize)) && !dt_units[lo as usize].is_empty() {
                dt_units[lo as usize]
            } else {
                "UNKNOWN"
            };
            format!("DATETIME {hi_unit} TO {lo_unit}")
        }
        11 => "BYTE".into(),
        12 => "TEXT".into(),
        13 => {
            let c = if collength < 0 { collength + 65536 } else { collength };
            let m = c % 256;
            let n = c / 256;
            if n == 255 || n == 0 {
                format!("VARCHAR({m})")
            } else {
                format!("VARCHAR({m},{n})")
            }
        }
        14 => {
            let hi = (collength >> 8) & 0xFF;
            let lo = collength & 0xFF;
            let iv_units = [
                "YEAR", "", "MONTH", "", "DAY", "", "HOUR", "", "MINUTE", "",
                "SECOND", "FRACTION(1)", "FRACTION", "FRACTION", "FRACTION(4)", "FRACTION(5)",
            ];
            let hi_unit = if (0..iv_units.len()).contains(&(hi as usize)) && !iv_units[hi as usize].is_empty() {
                iv_units[hi as usize]
            } else {
                "UNKNOWN"
            };
            let lo_unit = if (0..iv_units.len()).contains(&(lo as usize)) && !iv_units[lo as usize].is_empty() {
                iv_units[lo as usize]
            } else {
                "UNKNOWN"
            };
            format!("INTERVAL {hi_unit} TO {lo_unit}")
        }
        15 => format!("NCHAR({collength})"),
        16 => {
            let c = if collength < 0 { collength + 65536 } else { collength };
            let m = c % 256;
            let n = c / 256;
            if n == 255 || n == 0 {
                format!("NVARCHAR({m})")
            } else {
                format!("NVARCHAR({m},{n})")
            }
        }
        17 => "INT8".into(),
        18 => "SERIAL8".into(),
        40 => format!("LVARCHAR({collength})"),
        41 | 45 => "BOOLEAN".into(),
        52 => "BIGINT".into(),
        53 => "BIGSERIAL".into(),
        101 => format!("CLOB({collength})"),
        102 => format!("BLOB({collength})"),
        other => format!("TYPE({other})"),
    }
}

fn parse_csv_line(line: &str) -> Vec<String> {
    let mut fields = Vec::new();
    let chars: Vec<char> = line.chars().collect();
    let mut i = 0;
    let n = chars.len();

    while i < n {
        if chars[i] == '"' {
            i += 1;
            let mut field = String::new();
            while i < n {
                if chars[i] == '"' {
                    if i + 1 < n && chars[i + 1] == '"' {
                        field.push('"');
                        i += 2;
                    } else {
                        i += 1;
                        break;
                    }
                } else {
                    field.push(chars[i]);
                    i += 1;
                }
            }
            fields.push(field);
            if i < n && chars[i] == ',' {
                i += 1;
            }
        } else {
            let mut field = String::new();
            while i < n && chars[i] != ',' {
                field.push(chars[i]);
                i += 1;
            }
            fields.push(field);
            if i < n && chars[i] == ',' {
                i += 1;
            }
        }
    }
    fields
}

async fn dump_table_inserts(conn: &mut Connection, out: &mut dyn Write, table: &str) -> Result<(), sqlicon_rs::Error> {
    let sql = format!("SELECT * FROM {table}");
    let res = conn.query(&sql).await?;

    writeln!(out, "-- table: {table}")?;
    for row in &res.rows {
        write!(out, "INSERT INTO {table} VALUES (")?;
        for (i, _) in res.columns.iter().enumerate() {
            if i > 0 {
                write!(out, ", ")?;
            }
            if row.is_null(i) {
                write!(out, "NULL")?;
            } else {
                let val = row.get_display_value(i).unwrap_or_default();
                let col = &res.columns[i];
                match col.col_type {
                    sqlicon_rs::ColumnType::SmallInt
                    | sqlicon_rs::ColumnType::Int
                    | sqlicon_rs::ColumnType::BigInt
                    | sqlicon_rs::ColumnType::Serial
                    | sqlicon_rs::ColumnType::Serial8
                    | sqlicon_rs::ColumnType::BigSerial
                    | sqlicon_rs::ColumnType::Float
                    | sqlicon_rs::ColumnType::SmallFloat => {
                        write!(out, "{val}")?;
                    }
                    _ => {
                        let escaped = val.replace('\'', "''");
                        write!(out, "'{escaped}'")?;
                    }
                }
            }
        }
        writeln!(out, ");")?;
    }
    Ok(())
}

fn handle_dot_command<'a>(
    conn: &'a mut Connection,
    rt: &'a mut ShellRuntime,
    line: &'a str,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<bool, Box<dyn std::error::Error + Send + Sync>>> + 'a>> {
    Box::pin(async move {
        let mut parts = line.split_whitespace();
        let cmd = parts.next().unwrap_or("");
        let arg = line.strip_prefix(cmd).unwrap_or("").trim();

        match cmd {
            ".quit" | ".exit" => return Ok(true),
            ".help" => {
                println!("Dot commands:");
                println!("  .help                 Show this help");
            println!("  .quit | .exit         Exit shell");
            println!("  .mode MODE            Set output mode: aligned|csv|line|json|markdown");
            println!("  .width IDX WIDTH      Set minimum column width for IDX (1-based)");
            println!("  .width off            Reset width overrides");
            println!("  .headers on|off       Toggle table headers");
            println!("  .bail on|off          Stop or continue on SQL errors");
            println!("  .timer on|off         Show statement execution time");
            println!("  .nullvalue TEXT       Set NULL display text");
            println!("  .output [FILE|stdout] Redirect output persistently to FILE");
            println!("  .once FILE            Redirect next query output once to FILE");
            println!("  .read FILE            Execute SQL commands from script file");
            println!("  .explain on|off       Toggle SET EXPLAIN ON/OFF");
            println!("  .use DATABASE         Switch active database");
            println!("  .databases            List available databases");
            println!("  .status               Show session and connection status");
            println!("  .schema TABLE         Show table column types and nullability");
            println!("  .indexes TABLE        Show indexes and index columns");
            println!("  .views [TABLE]        Show view DDL definition");
            println!("  .constraints TABLE    Show constraints (PK, FK, UNIQUE, CHECK)");
            println!("  .dump [TABLE]         Export table rows as INSERT statements");
            println!("  .import FILE TABLE    Import CSV file into table");
            println!("  .tables               List user tables");
            println!("  .finderr CODE         Show Informix error message description");
        }
        ".mode" => {
            match arg.to_lowercase().as_str() {
                "aligned" => rt.mode = OutputMode::Aligned,
                "csv" => rt.mode = OutputMode::Csv,
                "line" => rt.mode = OutputMode::Line,
                "json" => rt.mode = OutputMode::Json,
                "markdown" | "md" => rt.mode = OutputMode::Markdown,
                _ => eprintln!("error: .mode expects aligned|csv|line|json|markdown"),
            }
        }
        ".width" => {
            if arg.eq_ignore_ascii_case("off") || arg.eq_ignore_ascii_case("reset") {
                rt.width_overrides.clear();
            } else {
                let mut tokens = arg.split_whitespace();
                let idx_s = tokens.next().unwrap_or("");
                let w_s = tokens.next().unwrap_or("");
                if let (Ok(idx), Ok(w)) = (idx_s.parse::<usize>(), w_s.parse::<usize>()) {
                    if idx >= 1 {
                        if rt.width_overrides.len() < idx {
                            rt.width_overrides.resize(idx, 0);
                        }
                        rt.width_overrides[idx - 1] = w;
                    } else {
                        eprintln!("error: .width index must be >= 1");
                    }
                } else {
                    eprintln!("error: .width expects 'IDX WIDTH' or 'off'");
                }
            }
        }
        ".headers" => {
            match arg.to_lowercase().as_str() {
                "on" => rt.headers = true,
                "off" => rt.headers = false,
                _ => eprintln!("error: .headers expects on|off"),
            }
        }
        ".bail" => {
            match arg.to_lowercase().as_str() {
                "on" => rt.bail_on_error = true,
                "off" => rt.bail_on_error = false,
                _ => eprintln!("error: .bail expects on|off"),
            }
        }
        ".timer" => {
            match arg.to_lowercase().as_str() {
                "on" => rt.timer_on = true,
                "off" => rt.timer_on = false,
                _ => eprintln!("error: .timer expects on|off"),
            }
        }
        ".nullvalue" => {
            rt.null_value = arg.to_string();
        }
        ".output" => {
            if arg.is_empty() || arg.eq_ignore_ascii_case("stdout") {
                rt.output_path = None;
            } else {
                rt.output_path = Some(arg.to_string());
            }
        }
        ".once" => {
            if arg.is_empty() {
                eprintln!("error: .once requires a file path");
            } else {
                rt.once_path = Some(arg.to_string());
            }
        }
        ".read" => {
            if arg.is_empty() {
                eprintln!("error: .read requires a file path");
            } else if rt.read_depth >= 8 {
                eprintln!("error: .read nesting limit reached");
            } else {
                match fs::read_to_string(arg) {
                    Ok(content) => {
                        rt.read_depth += 1;
                        let _ = Box::pin(execute_script(conn, rt, &content)).await;
                        rt.read_depth -= 1;
                    }
                    Err(e) => eprintln!("error: cannot open script file '{arg}': {e}"),
                }
            }
        }
        ".explain" => {
            match arg.to_lowercase().as_str() {
                "on" => {
                    let _ = conn.execute("SET EXPLAIN ON").await;
                }
                "off" => {
                    let _ = conn.execute("SET EXPLAIN OFF").await;
                }
                _ => eprintln!("error: .explain expects on|off"),
            }
        }
        ".use" => {
            if arg.is_empty() {
                eprintln!("error: .use requires database name");
            } else {
                let sql = format!("DATABASE {arg}");
                match conn.execute(&sql).await {
                    Ok(_) => {
                        conn.set_current_database(arg.to_string());
                        println!("Active database switched to: {arg}");
                    }
                    Err(e) => eprintln!("error: {e}"),
                }
            }
        }
        ".status" => {
            let mut writer = match rt.get_writer() {
                Ok(w) => w,
                Err(e) => {
                    eprintln!("error: output writer: {e}");
                    return Ok(false);
                }
            };
            let cp = conn.params();
            writeln!(writer, "mode={}", rt.mode.name())?;
            writeln!(writer, "headers={}", if rt.headers { "on" } else { "off" })?;
            writeln!(writer, "bail={}", if rt.bail_on_error { "on" } else { "off" })?;
            writeln!(writer, "timer={}", if rt.timer_on { "on" } else { "off" })?;
            writeln!(writer, "nullvalue={}", rt.null_value)?;
            writeln!(writer, "output={}", rt.output_path.as_deref().unwrap_or("stdout"))?;
            writeln!(writer, "once={}", rt.once_path.as_deref().unwrap_or("(none)"))?;
            writeln!(writer, "host={}", cp.hostname)?;
            writeln!(writer, "port={}", cp.service)?;
            writeln!(writer, "server={}", cp.server)?;
            writeln!(writer, "database={}", cp.database.as_deref().unwrap_or("(none)"))?;
            writeln!(writer, "user={}", cp.username)?;
            writeln!(writer, "client_locale={}", cp.client_locale)?;
            writeln!(writer, "db_locale={}", cp.db_locale)?;
            writeln!(writer, "profile={}", rt.conn_profile.as_deref().unwrap_or("(none)"))?;
        }
        ".schema" => {
            if arg.is_empty() {
                eprintln!("error: .schema requires table name");
            } else {
                let sql = format!(
                    "SELECT c.colno, c.colname, c.coltype, c.collength \
                     FROM syscolumns c, systables t \
                     WHERE t.tabid = c.tabid \
                     AND t.tabname = '{arg}' \
                     ORDER BY c.colno"
                );
                match conn.query(&sql).await {
                    Ok(res) if !res.rows.is_empty() => {
                        let mut writer = match rt.get_writer() {
                            Ok(w) => w,
                            Err(e) => {
                                eprintln!("error: output writer: {e}");
                                return Ok(false);
                            }
                        };
                        writeln!(writer, "{:<20} {:<30} Nullable", "Column", "Type")?;
                        writeln!(writer, "{:<20} {:<30} --------", "--------------------", "------------------------------")?;
                        for row in &res.rows {
                            let colname = row.get_display_value(1).unwrap_or_default();
                            let coltype: i32 = row.get_i32(2).ok().flatten()
                                .or_else(|| row.get_i16(2).ok().flatten().map(|v| v as i32))
                                .or_else(|| row.get_display_value(2).ok().and_then(|s| s.parse().ok()))
                                .unwrap_or(0);
                            let collength: i32 = row.get_i32(3).ok().flatten()
                                .or_else(|| row.get_i16(3).ok().flatten().map(|v| v as i32))
                                .or_else(|| row.get_display_value(3).ok().and_then(|s| s.parse().ok()))
                                .unwrap_or(0);
                            let type_str = format_sql_type(coltype, collength);
                            let is_nullable = (coltype & 0x100) == 0;
                            writeln!(writer, "{:<20} {:<30} {}", colname, type_str, if is_nullable { "YES" } else { "NO" })?;
                        }
                    }
                    Ok(_) => eprintln!("error: table '{arg}' not found"),
                    Err(e) => eprintln!("error: {e}"),
                }
            }
        }
        ".indexes" => {
            if arg.is_empty() {
                eprintln!("error: .indexes requires table name");
            } else {
                let tab_sql = format!("SELECT tabid FROM systables WHERE tabname = '{arg}'");
                let tab_res = match conn.query(&tab_sql).await {
                    Ok(r) => r,
                    Err(e) => {
                        eprintln!("error: {e}");
                        return Ok(false);
                    }
                };
                if tab_res.rows.is_empty() {
                    eprintln!("error: table '{arg}' not found");
                    return Ok(false);
                }
                let tabid: i32 = tab_res.rows[0].get_i32(0).ok().flatten()
                    .or_else(|| tab_res.rows[0].get_display_value(0).ok().and_then(|s| s.parse().ok()))
                    .unwrap_or(0);

                let idx_sql = format!(
                    "SELECT i.idxname, i.idxtype, i.part1, i.part2, i.part3, i.part4, \
                     i.part5, i.part6, i.part7, i.part8, i.part9, i.part10, \
                     i.part11, i.part12, i.part13, i.part14, i.part15, i.part16 \
                     FROM sysindexes i, systables t \
                     WHERE t.tabid = i.tabid \
                     AND t.tabname = '{arg}' \
                     AND i.idxname IS NOT NULL \
                     ORDER BY i.idxname"
                );
                match conn.query(&idx_sql).await {
                    Ok(res) => {
                        let mut writer = match rt.get_writer() {
                            Ok(w) => w,
                            Err(e) => {
                                eprintln!("error: output writer: {e}");
                                return Ok(false);
                            }
                        };
                        for row in &res.rows {
                            let idxname = row.get_display_value(0).unwrap_or_default();
                            let idxtype = row.get_display_value(1).unwrap_or_default();
                            writeln!(writer, "{idxname} ({idxtype}):")?;

                            for p in 2..18 {
                                let part_opt: Option<i32> = row.get_i32(p).ok().flatten()
                                    .or_else(|| row.get_i16(p).ok().flatten().map(|v| v as i32))
                                    .or_else(|| row.get_display_value(p).ok().and_then(|s| s.parse().ok()));
                                if let Some(part) = part_opt
                                    && part != 0 {
                                        let abs_part = part.abs();
                                        let col_sql = format!("SELECT colname FROM syscolumns WHERE tabid = {tabid} AND colno = {abs_part}");
                                        if let Ok(cres) = conn.query(&col_sql).await
                                            && !cres.rows.is_empty() {
                                                let cname = cres.rows[0].get_display_value(0).unwrap_or_default();
                                                let desc = if part < 0 { " DESC" } else { "" };
                                                writeln!(writer, "  {cname}{desc}")?;
                                        }
                                }
                            }
                            writeln!(writer)?;
                        }
                    }
                    Err(e) => eprintln!("error: {e}"),
                }
            }
        }
        ".views" => {
            let sql = if !arg.is_empty() {
                format!(
                    "SELECT t.tabname, v.seqno, v.viewtext \
                     FROM sysviews v, systables t \
                     WHERE t.tabid = v.tabid \
                     AND t.tabname = '{arg}' \
                     ORDER BY v.seqno"
                )
            } else {
                "SELECT t.tabname, v.seqno, v.viewtext \
                 FROM sysviews v, systables t \
                 WHERE t.tabid = v.tabid \
                 AND t.tabid >= 100 \
                 ORDER BY t.tabname, v.seqno".into()
            };

            match conn.query(&sql).await {
                Ok(res) => {
                    let mut writer = match rt.get_writer() {
                        Ok(w) => w,
                        Err(e) => {
                            eprintln!("error: output writer: {e}");
                            return Ok(false);
                        }
                    };
                    let mut cur_view = String::new();
                    let mut view_body = String::new();

                    for row in &res.rows {
                        let tabname = row.get_display_value(0).unwrap_or_default();
                        let seqno = row.get_i32(1).unwrap_or(Some(1)).unwrap_or(1);
                        let text = row.get_display_value(2).unwrap_or_default();

                        if tabname != cur_view {
                            if !cur_view.is_empty() {
                                writeln!(writer, "\n{view_body}\n")?;
                            }
                            cur_view = tabname.clone();
                            view_body.clear();
                            writeln!(writer, "CREATE VIEW {tabname} AS")?;
                        }

                        if seqno != 1 {
                            view_body.push(' ');
                        }
                        view_body.push_str(&text);
                    }
                    if !cur_view.is_empty() {
                        writeln!(writer, "\n{view_body}")?;
                    }
                }
                Err(e) => eprintln!("error: {e}"),
            }
        }
        ".constraints" => {
            if arg.is_empty() {
                eprintln!("error: .constraints requires table name");
            } else {
                let sql = format!(
                    "SELECT sc.constrname, \
                     CASE sc.constrtype \
                       WHEN 'P' THEN 'PRIMARY KEY' \
                       WHEN 'R' THEN 'FOREIGN KEY' \
                       WHEN 'U' THEN 'UNIQUE' \
                       WHEN 'C' THEN 'CHECK' \
                       WHEN 'N' THEN 'NOT NULL' \
                       ELSE 'OTHER' END AS kind, \
                     sc.idxname, rt.tabname AS ref_table, sr.updrule, sr.delrule \
                     FROM sysconstraints sc, systables t, OUTER sysreferences sr, OUTER systables rt \
                     WHERE t.tabid = sc.tabid \
                     AND sr.constrid = sc.constrid \
                     AND rt.tabid = sr.ptabid \
                     AND t.tabname = '{arg}' \
                     ORDER BY sc.constrname"
                );
                match conn.query(&sql).await {
                    Ok(res) => {
                        let mut writer = match rt.get_writer() {
                            Ok(w) => w,
                            Err(e) => {
                                eprintln!("error: output writer: {e}");
                                return Ok(false);
                            }
                        };
                        let _ = render_result(&mut *writer, rt, &res);
                    }
                    Err(e) => eprintln!("error: {e}"),
                }
            }
        }
        ".dump" => {
            let mut writer = match rt.get_writer() {
                Ok(w) => w,
                Err(e) => {
                    eprintln!("error: output writer: {e}");
                    return Ok(false);
                }
            };

            let cur_db = conn.params().database.clone().unwrap_or_default();
            let has_logging = if !cur_db.is_empty() {
                let check_sql = format!("SELECT is_logging FROM sysmaster:sysdatabases WHERE name = '{cur_db}'");
                conn.query(&check_sql).await.ok()
                    .and_then(|r| r.rows.first().and_then(|row| row.get_display_value(0).ok()))
                    .map(|v| v != "0")
                    .unwrap_or(false)
            } else {
                false
            };

            if !arg.is_empty() {
                if has_logging {
                    writeln!(writer, "BEGIN WORK;")?;
                }
                if let Err(e) = dump_table_inserts(conn, &mut *writer, arg).await {
                    eprintln!("error: .dump failed: {e}");
                    if has_logging {
                        writeln!(writer, "ROLLBACK WORK;")?;
                    }
                } else if has_logging {
                    writeln!(writer, "COMMIT WORK;")?;
                }
            } else {
                let sql = "SELECT tabname FROM systables WHERE tabid >= 100 AND tabtype = 'T' AND owner = USER ORDER BY tabname";
                match conn.query(sql).await {
                    Ok(t_res) => {
                        writeln!(writer, "-- sqlicon dump (all user tables)")?;
                        if has_logging {
                            writeln!(writer, "BEGIN WORK;")?;
                        }
                        let mut success = true;
                        for r in &t_res.rows {
                            let tname = r.get_display_value(0).unwrap_or_default();
                            if let Err(e) = dump_table_inserts(conn, &mut *writer, &tname).await {
                                eprintln!("error: .dump table '{tname}' failed: {e}");
                                success = false;
                                break;
                            }
                        }
                        if has_logging {
                            if success {
                                writeln!(writer, "COMMIT WORK;")?;
                            } else {
                                writeln!(writer, "ROLLBACK WORK;")?;
                            }
                        }
                    }
                    Err(e) => eprintln!("error: {e}"),
                }
            }
        }
        ".import" => {
            let mut tokens = arg.split_whitespace();
            let file_path = tokens.next().unwrap_or("");
            let table_name = tokens.next().unwrap_or("");

            if file_path.is_empty() || table_name.is_empty() {
                eprintln!("usage: .import <file.csv> <table>");
            } else if !Path::new(file_path).exists() {
                eprintln!("error: .import cannot open file '{file_path}'");
            } else {
                match fs::read_to_string(file_path) {
                    Ok(content) => {
                        let mut lines = content.lines();
                        if let Some(header_line) = lines.next() {
                            let headers = parse_csv_line(header_line);
                            if headers.is_empty() {
                                eprintln!("error: .import CSV header is empty");
                            } else {
                                let col_list = headers.join(", ");
                                let cur_db = conn.params().database.clone().unwrap_or_default();
                                let has_logging = if !cur_db.is_empty() {
                                    let check_sql = format!("SELECT is_logging FROM sysmaster:sysdatabases WHERE name = '{cur_db}'");
                                    conn.query(&check_sql).await.ok()
                                        .and_then(|r| r.rows.first().and_then(|row| row.get_display_value(0).ok()))
                                        .map(|v| v != "0")
                                        .unwrap_or(false)
                                } else {
                                    false
                                };

                                let in_txn = if has_logging { conn.begin().await.is_ok() } else { false };
                                let mut row_count = 0;
                                let mut has_err = false;

                                for (line_num, line) in lines.enumerate() {
                                    if line.trim().is_empty() {
                                        continue;
                                    }
                                    let fields = parse_csv_line(line);
                                    let mut vals = Vec::with_capacity(headers.len());
                                    for i in 0..headers.len() {
                                        if i < fields.len() {
                                            let f = &fields[i];
                                            if f.eq_ignore_ascii_case("null") {
                                                vals.push("NULL".to_string());
                                            } else {
                                                let escaped = f.replace('\'', "''");
                                                vals.push(format!("'{escaped}'"));
                                            }
                                        } else {
                                            vals.push("NULL".to_string());
                                        }
                                    }
                                    let val_list = vals.join(", ");
                                    let insert_sql = format!("INSERT INTO {table_name} ({col_list}) VALUES ({val_list})");
                                    if let Err(e) = conn.execute(&insert_sql).await {
                                        eprintln!("error: .import failed at line {}: {e}", line_num + 2);
                                        has_err = true;
                                        break;
                                    }
                                    row_count += 1;
                                }

                                if has_err {
                                    if in_txn {
                                        let _ = conn.rollback().await;
                                        eprintln!("Import failed: transaction rolled back.");
                                    } else {
                                        eprintln!("Import failed.");
                                    }
                                } else {
                                    if in_txn {
                                        let _ = conn.commit().await;
                                    }
                                    println!("Imported {row_count} row(s) into {table_name}");
                                }
                            }
                        }
                    }
                    Err(e) => eprintln!("error: cannot read file '{file_path}': {e}"),
                }
            }
        }
        ".tables" => {
            let sql = "SELECT tabname FROM systables WHERE tabid >= 100 AND tabtype = 'T' ORDER BY tabname";
            match conn.query(sql).await {
                Ok(res) => {
                    let mut writer = match rt.get_writer() {
                        Ok(w) => w,
                        Err(e) => {
                            eprintln!("error: output writer: {e}");
                            return Ok(false);
                        }
                    };
                    let _ = render_result(&mut *writer, rt, &res);
                }
                Err(e) => eprintln!("error: {e}"),
            }
        }
        ".databases" => {
            let sql = "SELECT name FROM sysmaster:sysdatabases ORDER BY name";
            match conn.query(sql).await {
                Ok(res) => {
                    let mut writer = match rt.get_writer() {
                        Ok(w) => w,
                        Err(e) => {
                            eprintln!("error: output writer: {e}");
                            return Ok(false);
                        }
                    };
                    let _ = render_result(&mut *writer, rt, &res);
                }
                Err(e) => eprintln!("error: {e}"),
            }
        }
        ".finderr" => {
            if arg.is_empty() {
                eprintln!("usage: .finderr <error_code>");
            } else if let Ok(code) = arg.parse::<i32>() {
                if let Some(msg) = sqlicon_rs::lookup_error_message(code) {
                    println!("[{code}] {msg}");
                } else {
                    println!("[{code}] Unknown error code.");
                }
            } else {
                eprintln!("error: error code must be an integer");
            }
        }
        other => {
            eprintln!("error: unknown dot-command '{other}'. Type '.help' for available commands.");
        }
    }
    Ok(false)
    })
}

fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // Windows default main thread stack is 1MB. In unoptimized debug builds,
    // large async state machines can exceed 1MB. Spawn a thread with 4MB stack.
    std::thread::Builder::new()
        .stack_size(4 * 1024 * 1024)
        .spawn(|| {
            let rt = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .thread_stack_size(4 * 1024 * 1024)
                .build()
                .expect("failed to build tokio runtime");
            rt.block_on(async_main())
        })?
        .join()
        .map_err(|_| io::Error::new(io::ErrorKind::Other, "main worker thread panicked"))?
}

async fn async_main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let args = match parse_cli_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("error: {e}");
            process::exit(1);
        }
    };

    if let Some(code) = args.finderr {
        if let Some(desc) = sqlicon_rs::lookup_error_message(code) {
            println!("[{code}] {desc}");
        } else {
            println!("[{code}] Unknown error code.");
        }
        return Ok(());
    }

    let mut store = ProfileStore::load().unwrap_or_default();

    if args.profile_list {
        for name in store.profiles.keys() {
            println!("{name}");
        }
        return Ok(());
    }

    if let Some(ref name) = args.profile_show {
        match store.get(name) {
            Some(p) => {
                println!("name={}", p.name);
                if let Some(ref v) = p.host { println!("host={v}"); }
                if let Some(ref v) = p.port { println!("port={v}"); }
                if let Some(ref v) = p.server { println!("server={v}"); }
                if let Some(ref v) = p.database { println!("database={v}"); }
                if let Some(ref v) = p.user { println!("user={v}"); }
                if let Some(ref v) = p.password {
                    if args.show_secret {
                        println!("password={v}");
                    } else {
                        println!("password=***");
                    }
                }
                if let Some(ref v) = p.client_locale { println!("client_locale={v}"); }
                if let Some(ref v) = p.db_locale { println!("db_locale={v}"); }
                return Ok(());
            }
            None => {
                eprintln!("error: profile '{name}' not found");
                process::exit(1);
            }
        }
    }

    if let Some(ref name) = args.profile_default {
        if !store.profiles.contains_key(name) {
            eprintln!("error: profile '{name}' not found");
            process::exit(1);
        }
        store.default_name = Some(name.clone());
        store.save()?;
        println!("Default profile set to: {name}");
        return Ok(());
    }

    if let Some(ref name) = args.profile_delete {
        if !args.yes {
            eprintln!("error: deleting profile requires --yes");
            process::exit(1);
        }
        if store.profiles.remove(name).is_some() {
            if store.default_name.as_deref() == Some(name) {
                store.default_name = None;
            }
            store.save()?;
            println!("Profile '{name}' deleted.");
            return Ok(());
        } else {
            eprintln!("error: profile '{name}' not found");
            process::exit(1);
        }
    }

    if let Some(ref name) = args.profile_create {
        let mut p = if let Some(ref uri) = args.connect_uri {
            let cp = ConnectParams::from_uri(uri)?;
            let mut prof = Profile::new(name);
            prof.host = Some(cp.hostname);
            prof.port = Some(cp.service);
            prof.server = Some(cp.server);
            prof.database = cp.database;
            prof.user = Some(cp.username);
            prof.password = Some(cp.password);
            prof.client_locale = Some(cp.client_locale);
            prof.db_locale = Some(cp.db_locale);
            prof
        } else {
            let mut prof = Profile::new(name);
            prof.host = args.host.clone();
            prof.port = args.port.clone();
            prof.server = args.server.clone();
            prof.database = args.database.clone();
            prof.user = args.user.clone();
            prof.password = args.password.clone();
            prof.client_locale = args.client_locale.clone();
            prof.db_locale = args.db_locale.clone();
            prof
        };

        if let Some(ref v) = args.host { p.host = Some(v.clone()); }
        if let Some(ref v) = args.port { p.port = Some(v.clone()); }
        if let Some(ref v) = args.server { p.server = Some(v.clone()); }
        if let Some(ref v) = args.database { p.database = Some(v.clone()); }
        if let Some(ref v) = args.user { p.user = Some(v.clone()); }
        if let Some(ref v) = args.password { p.password = Some(v.clone()); }
        if let Some(ref v) = args.client_locale { p.client_locale = Some(v.clone()); }
        if let Some(ref v) = args.db_locale { p.db_locale = Some(v.clone()); }

        store.profiles.insert(name.clone(), p);
        store.save()?;
        println!("Profile '{name}' created.");
        return Ok(());
    }

    if let Some(ref name) = args.profile_update {
        let p = match store.profiles.get_mut(name) {
            Some(p) => p,
            None => {
                eprintln!("error: profile '{name}' not found");
                process::exit(1);
            }
        };
        if let Some(ref v) = args.host { p.host = Some(v.clone()); }
        if let Some(ref v) = args.port { p.port = Some(v.clone()); }
        if let Some(ref v) = args.server { p.server = Some(v.clone()); }
        if let Some(ref v) = args.database { p.database = Some(v.clone()); }
        if let Some(ref v) = args.user { p.user = Some(v.clone()); }
        if let Some(ref v) = args.password { p.password = Some(v.clone()); }
        if let Some(ref v) = args.client_locale { p.client_locale = Some(v.clone()); }
        if let Some(ref v) = args.db_locale { p.db_locale = Some(v.clone()); }

        store.save()?;
        println!("Profile '{name}' updated.");
        return Ok(());
    }

    let connect_params = match build_connect_params(&args, &store) {
        Ok(cp) => cp,
        Err(e) => {
            eprintln!("error: {e}");
            process::exit(1);
        }
    };

    if let Some(ref test_name) = args.profile_test
        && store.get(test_name).is_none() {
            eprintln!("error: profile '{test_name}' not found");
            process::exit(1);
        }

    let mut conn = match Connection::connect(connect_params).await {
        Ok(c) => c,
        Err(e) => {
            eprintln!("error: connection failed: {e}");
            process::exit(1);
        }
    };

    let mut rt = ShellRuntime {
        conn_profile: args.profile.clone().or_else(|| store.default_name.clone()),
        ..ShellRuntime::default()
    };

    if args.profile_test.is_some() {
        match conn.query("SELECT FIRST 1 tabid FROM systables").await {
            Ok(res) => {
                let mut stdout = io::stdout();
                let _ = render_result(&mut stdout, &rt, &res);
                return Ok(());
            }
            Err(e) => {
                eprintln!("error: profile test query failed: {e}");
                process::exit(1);
            }
        }
    }

    // Mode: Inline command (-c)
    if let Some(ref sql) = args.command {
        if let Err(e) = execute_script(&mut conn, &mut rt, sql).await {
            eprintln!("error: {e}");
            process::exit(1);
        }
        return Ok(());
    }

    // Mode: Script file (-f)
    if let Some(ref path) = args.file {
        let content = fs::read_to_string(path)?;
        if let Err(e) = execute_script(&mut conn, &mut rt, &content).await {
            eprintln!("error: {e}");
            process::exit(1);
        }
        return Ok(());
    }

    // Mode: Pipe from stdin
    if !io::stdin().is_terminal() {
        let mut content = String::new();
        io::stdin().read_to_string(&mut content)?;
        if let Err(e) = execute_script(&mut conn, &mut rt, &content).await {
            eprintln!("error: {e}");
            process::exit(1);
        }
        return Ok(());
    }

    // Mode: Interactive REPL
    println!("sqlicon-rs {} - interactive shell for Informix", env!("CARGO_PKG_VERSION"));
    println!("Type '.help' for dot commands or 'exit' / 'quit' to quit.\n");

    let stdin = io::stdin();
    let mut reader = stdin.lock();

    loop {
        print!("sqlicon> ");
        io::stdout().flush()?;

        let mut line = String::new();
        if reader.read_line(&mut line)? == 0 {
            println!();
            break;
        }

        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        if trimmed.starts_with('.') {
            if handle_dot_command(&mut conn, &mut rt, trimmed).await? {
                break;
            }
            continue;
        }

        if trimmed.eq_ignore_ascii_case("exit") || trimmed.eq_ignore_ascii_case("quit") {
            break;
        }

        if let Err(e) = execute_script(&mut conn, &mut rt, trimmed).await {
            eprintln!("error: {e}");
        }
    }

    Ok(())
}
