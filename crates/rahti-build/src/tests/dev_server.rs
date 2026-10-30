use super::*;
use std::net::TcpListener;

fn project(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("rahti-dev-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(root.join(".rahti")).unwrap();
    fs::create_dir_all(root.join("src/app")).unwrap();
    root
}

fn info_json(address: &str) -> String {
    json!({
        "pid": 7,
        "url": "http://localhost:3000",
        "address": address,
        "boot": "boot-1",
        "token": "secret-token",
        "startedAtMs": 1,
    })
    .to_string()
}

/// A server that answers one request with `body`, and hands back what it read.
fn answer_once(body: &'static str) -> (String, std::thread::JoinHandle<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap().to_string();
    let handle = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = [0_u8; 1024];
        let read = stream.read(&mut request).unwrap();
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        )
        .unwrap();
        String::from_utf8_lossy(&request[..read]).into_owned()
    });
    (address, handle)
}

#[test]
fn a_live_server_is_asked_with_its_token() {
    let root = project("live");
    let (address, server) = answer_once(r#"{"boot":"boot-1","jobs":null}"#);
    fs::write(root.join(INFO_PATH), info_json(&address)).unwrap();

    let info = read_info(&root).expect("the file");
    assert_eq!(info.boot, "boot-1");
    assert!(live_status(&info).is_some());
    let request = server.join().unwrap();
    assert!(
        request.ends_with("\r\n\r\n") && !request.replace("\r\n", "").contains('\n'),
        "every line of a request ends in CRLF: {request:?}"
    );
    assert!(request.contains("x-rahti-dev-token: secret-token"));
}

#[test]
fn another_boot_on_the_same_port_is_not_this_server() {
    let root = project("other-boot");
    let (address, server) = answer_once(r#"{"boot":"someone-else"}"#);
    fs::write(root.join(INFO_PATH), info_json(&address)).unwrap();

    assert!(live_status(&read_info(&root).unwrap()).is_none());
    server.join().unwrap();
}

#[test]
fn a_file_from_a_crash_reads_as_not_running() {
    let root = project("crashed");
    let free = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = free.local_addr().unwrap().to_string();
    drop(free);
    fs::write(root.join(INFO_PATH), info_json(&address)).unwrap();

    let report = status(&root);
    assert_eq!(report["running"], false);
    assert!(
        report["next"]
            .as_str()
            .unwrap()
            .contains("cargo rahti restart")
    );
}

#[test]
fn no_status_output_carries_the_token() {
    let root = project("secret");
    let (address, server) = answer_once(r#"{"boot":"boot-1","jobs":null}"#);
    fs::write(root.join(INFO_PATH), info_json(&address)).unwrap();

    let report = status(&root).to_string();
    server.join().unwrap();
    assert!(report.contains("boot-1"), "{report}");
    assert!(!report.contains("secret-token"), "{report}");
}

#[test]
fn edits_after_the_boot_are_reported_and_public_files_are_not() {
    let root = project("stale");
    fs::create_dir_all(root.join("public")).unwrap();
    fs::write(root.join("src/app/page.rs"), "").unwrap();
    fs::write(root.join("public/site.css"), "").unwrap();

    let changed = changed_since(&root, 0);
    assert_eq!(changed, vec!["src/app/page.rs".to_string()]);
    assert!(changed_since(&root, u64::MAX).is_empty());
}

#[test]
fn the_browser_report_counts_this_boot_only() {
    let root = project("browser");
    let lines = [
        json!({"kind":"browser","boot":"old","level":"error","url":"http://x/old","message":"stale"}),
        json!({"kind":"browser","boot":"new","level":"load","url":"http://x/todo?a=1#b","receivedAtMs":5}),
        json!({"kind":"browser","boot":"new","level":"error","url":"http://x/todo","message":"broken"}),
        json!({"kind":"browser","boot":"new","level":"warn","url":"http://x/","message":"odd"}),
        json!({"kind":"server","boot":"new","level":"error","message":"not a browser"}),
    ];
    let log = lines.map(|line| line.to_string()).join("\n");
    fs::write(root.join(LOG_PATH), log).unwrap();

    let report = browser_report(&root, Some("new"));
    assert_eq!(report["errors"], 1);
    assert_eq!(report["warnings"], 1);
    let routes = report["routes"].as_array().unwrap();
    let todo = routes.iter().find(|r| r["route"] == "/todo").unwrap();
    assert_eq!(todo["loads"], 1);
    assert_eq!(todo["lastLoadAtMs"], 5);
    assert_eq!(todo["errors"], 1);
    assert_eq!(report["problems"][0]["message"], "broken");
    assert!(!report.to_string().contains("stale"));
}

#[test]
fn an_empty_project_reports_no_routes_rather_than_failing() {
    let root = project("empty");
    let report = browser_report(&root, None);
    assert_eq!(report["errors"], 0);
    assert_eq!(report["routes"].as_array().unwrap().len(), 0);
}
