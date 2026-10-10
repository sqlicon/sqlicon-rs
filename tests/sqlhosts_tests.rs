use sqlicon_rs::connection::{find_sqlhosts_entry, parse_sqlhosts};
use sqlicon_rs::ConnectParams;
use std::io::Write;

fn write_temp(name: &str, contents: &str) -> std::path::PathBuf {
    let mut path = std::env::temp_dir();
    path.push(name);
    let mut f = std::fs::File::create(&path).unwrap();
    f.write_all(contents.as_bytes()).unwrap();
    path
}

#[test]
fn resolves_connect_params_from_sqlhosts_tcp_entry() {
    let path = write_temp(
        "sqlicon_test_sqlhosts_tcp.tmp",
        "ol_tcp onsoctcp 127.0.0.1 9088\n",
    );

    let params = ConnectParams::from_sqlhosts("ol_tcp", Some(path.to_str().unwrap())).unwrap();
    std::fs::remove_file(&path).ok();

    assert_eq!(params.hostname, "127.0.0.1");
    assert_eq!(params.service, "9088");
    assert_eq!(params.server, "ol_tcp");
    assert!(!params.ssl_enable);
}

#[test]
fn resolves_connect_params_from_sqlhosts_ssl_entry() {
    let path = write_temp(
        "sqlicon_test_sqlhosts_ssl.tmp",
        "ol_ssl onsocssl db.example.com 9089\n",
    );

    let params = ConnectParams::from_sqlhosts("ol_ssl", Some(path.to_str().unwrap())).unwrap();
    std::fs::remove_file(&path).ok();

    assert_eq!(params.hostname, "db.example.com");
    assert_eq!(params.service, "9089");
    assert!(params.ssl_enable);
}

#[test]
fn resolves_connect_params_from_sqlhosts_unix_entry() {
    let path = write_temp(
        "sqlicon_test_sqlhosts_ipc.tmp",
        "ol_ipc onipcstr unused /tmp/ol_ipc.str\n",
    );

    let params = ConnectParams::from_sqlhosts("ol_ipc", Some(path.to_str().unwrap())).unwrap();
    std::fs::remove_file(&path).ok();

    assert_eq!(params.hostname, "");
    assert_eq!(params.service, "/tmp/ol_ipc.str");
}

#[test]
fn unknown_server_name_errors() {
    let path = write_temp(
        "sqlicon_test_sqlhosts_unknown.tmp",
        "ol_tcp onsoctcp 127.0.0.1 9088\n",
    );

    let result = ConnectParams::from_sqlhosts("does_not_exist", Some(path.to_str().unwrap()));
    std::fs::remove_file(&path).ok();

    assert!(result.is_err());
}

#[test]
fn public_parse_and_find_api_round_trips() {
    let path = write_temp(
        "sqlicon_test_sqlhosts_public_api.tmp",
        "# a comment\nol_a onsoctcp 10.0.0.1 1001\nol_b onsoctcp 10.0.0.2 1002\n",
    );

    let entries = parse_sqlhosts(Some(path.to_str().unwrap())).unwrap();
    std::fs::remove_file(&path).ok();

    assert_eq!(entries.len(), 2);
    let found = find_sqlhosts_entry(&entries, "OL_B").expect("case-insensitive lookup");
    assert_eq!(found.hostname, "10.0.0.2");
    assert!(find_sqlhosts_entry(&entries, "missing").is_none());
}
