//! sqlicon-rs - command-line interface and interactive shell for Informix.

use sqlicon_rs::{
    ConnectParams, Connection, Profile, ProfileStore, QueryResult,
};
use std::env;
use std::fs;
use std::io::{self, BufRead, IsTerminal, Read, Write};
use std::process;

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

    // 2. Resolve values: CLI arg > Env var > Profile > Fallback default
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

fn print_table(result: &QueryResult) {
    if result.columns.is_empty() {
        println!("{} row(s) affected", result.rows_affected);
        return;
    }

    let mut col_widths: Vec<usize> = result
        .columns
        .iter()
        .map(|c| c.name.len())
        .collect();

    let mut formatted_rows = Vec::with_capacity(result.rows.len());
    for row in &result.rows {
        let mut row_strs = Vec::with_capacity(result.columns.len());
        for (col_idx, _) in result.columns.iter().enumerate() {
            let val_str = row.get_display_value(col_idx).unwrap_or_else(|_| "NULL".into());
            col_widths[col_idx] = col_widths[col_idx].max(val_str.len());
            row_strs.push(val_str);
        }
        formatted_rows.push(row_strs);
    }

    // Print header
    let header_line = result
        .columns
        .iter()
        .enumerate()
        .map(|(i, c)| format!("{:<width$}", c.name, width = col_widths[i]))
        .collect::<Vec<_>>()
        .join(" | ");
    println!("{header_line}");

    // Print separator
    let sep_line = col_widths
        .iter()
        .map(|&w| "-".repeat(w))
        .collect::<Vec<_>>()
        .join("-+-");
    println!("{sep_line}");

    // Print rows
    for row in formatted_rows {
        let row_line = row
            .iter()
            .enumerate()
            .map(|(i, val)| format!("{:<width$}", val, width = col_widths[i]))
            .collect::<Vec<_>>()
            .join(" | ");
        println!("{row_line}");
    }

    println!("{} row(s) affected", result.rows.len().max(result.rows_affected as usize));
}

fn is_query_sql(sql: &str) -> bool {
    let s = sql.trim_start();
    let first_word = s.split_whitespace().next().unwrap_or("").to_uppercase();
    matches!(
        first_word.as_str(),
        "SELECT" | "WITH" | "SHOW" | "DESCRIBE" | "EXPLAIN" | "VALUES"
    )
}

async fn execute_statement(conn: &mut Connection, sql: &str) -> Result<(), sqlicon_rs::Error> {
    let trimmed = sql.trim();
    if trimmed.is_empty() {
        return Ok(());
    }

    if is_query_sql(trimmed) {
        let res = conn.query(trimmed).await?;
        print_table(&res);
    } else {
        let affected = conn.execute(trimmed).await?;
        println!("{affected} row(s) affected");
    }

    Ok(())
}

async fn execute_script(conn: &mut Connection, script: &str) -> Result<(), sqlicon_rs::Error> {
    // Split into individual statements by semicolon
    for raw in script.split(';') {
        let stmt = raw.trim();
        if !stmt.is_empty() {
            execute_statement(conn, stmt).await?;
        }
    }
    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
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

    // Profile actions
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
        let mut p = Profile::new(name);
        p.host = args.host.clone();
        p.port = args.port.clone();
        p.server = args.server.clone();
        p.database = args.database.clone();
        p.user = args.user.clone();
        p.password = args.password.clone();
        p.client_locale = args.client_locale.clone();
        p.db_locale = args.db_locale.clone();

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

    // Connect parameters
    let connect_params = match build_connect_params(&args, &store) {
        Ok(cp) => cp,
        Err(e) => {
            eprintln!("error: {e}");
            process::exit(1);
        }
    };

    if let Some(ref test_name) = args.profile_test {
        if store.get(test_name).is_none() {
            eprintln!("error: profile '{test_name}' not found");
            process::exit(1);
        }
    }

    // Connect to server
    let mut conn = match Connection::connect(connect_params).await {
        Ok(c) => c,
        Err(e) => {
            eprintln!("error: connection failed: {e}");
            process::exit(1);
        }
    };

    // If --profile-test, run probe query
    if args.profile_test.is_some() {
        match conn.query("SELECT FIRST 1 tabid FROM systables").await {
            Ok(res) => {
                print_table(&res);
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
        if let Err(e) = execute_script(&mut conn, sql).await {
            eprintln!("error: {e}");
            process::exit(1);
        }
        return Ok(());
    }

    // Mode: Script file (-f)
    if let Some(ref path) = args.file {
        let content = fs::read_to_string(path)?;
        if let Err(e) = execute_script(&mut conn, &content).await {
            eprintln!("error: {e}");
            process::exit(1);
        }
        return Ok(());
    }

    // Mode: Pipe from stdin
    if !io::stdin().is_terminal() {
        let mut content = String::new();
        io::stdin().read_to_string(&mut content)?;
        if let Err(e) = execute_script(&mut conn, &content).await {
            eprintln!("error: {e}");
            process::exit(1);
        }
        return Ok(());
    }

    // Mode: Interactive REPL
    println!("sqlicon-rs {} - interactive shell for Informix", env!("CARGO_PKG_VERSION"));
    println!("Type 'exit' or 'quit' to quit.\n");

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
        if trimmed.eq_ignore_ascii_case("exit") || trimmed.eq_ignore_ascii_case("quit") {
            break;
        }
        if trimmed.is_empty() {
            continue;
        }

        if let Err(e) = execute_script(&mut conn, trimmed).await {
            eprintln!("error: {e}");
        }
    }

    Ok(())
}
