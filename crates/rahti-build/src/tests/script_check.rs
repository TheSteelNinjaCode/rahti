use super::*;

fn project(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("rahti-scripts-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(root.join("public/js")).unwrap();
    fs::write(root.join(CONFIG_PATH), "{}").unwrap();
    root
}

/// A project-local `tsc` that prints `output` and exits with `code`, the way
/// `npm install --save-dev typescript` puts one in `node_modules/.bin`.
fn fake_tsc(root: &Path, output: &str, code: i32) {
    let bin = root.join("node_modules/.bin");
    fs::create_dir_all(&bin).unwrap();
    fs::write(root.join("tsc-output.txt"), output).unwrap();
    if cfg!(windows) {
        fs::write(
            bin.join("tsc.cmd"),
            format!("@type tsc-output.txt\r\n@exit /b {code}\r\n"),
        )
        .unwrap();
    } else {
        let script = bin.join("tsc");
        fs::write(
            &script,
            format!("#!/bin/sh\ncat tsc-output.txt\nexit {code}\n"),
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
        }
    }
}

#[test]
fn a_located_error_is_read_with_its_position() {
    let errors =
        parse("public/js/drive-store.js(10,48): error TS2300: Duplicate identifier 'Storage'.\n");
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0]["file"], "public/js/drive-store.js");
    assert_eq!(errors[0]["line"], 10);
    assert_eq!(errors[0]["column"], 48);
    assert_eq!(errors[0]["code"], "TS2300");
    assert_eq!(errors[0]["message"], "Duplicate identifier 'Storage'.");
}

#[test]
fn an_indented_line_continues_the_message_before_it() {
    let errors = parse(
        "public/js/a.js(2,5): error TS2322: Type 'string' is not assignable.\n  \
         Type 'string' is not assignable to type 'number'.\n",
    );
    assert_eq!(errors.len(), 1);
    assert_eq!(
        errors[0]["message"],
        "Type 'string' is not assignable.\nType 'string' is not assignable to type 'number'."
    );
}

#[test]
fn an_error_without_a_file_is_the_projects() {
    let errors = parse("error TS5023: Unknown compiler option 'nope'.\n");
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0]["file"], CONFIG_PATH);
    assert_eq!(errors[0]["code"], "TS5023");
}

#[test]
fn a_parenthesised_directory_does_not_confuse_the_position() {
    let errors = parse("public/js (old)/a.js(3,1): error TS1005: ';' expected.\n");
    assert_eq!(errors[0]["file"], "public/js (old)/a.js");
    assert_eq!(errors[0]["line"], 3);
}

#[test]
fn without_a_jsconfig_there_is_nothing_to_check() {
    let root = project("no-config");
    fs::remove_file(root.join(CONFIG_PATH)).unwrap();
    assert_eq!(report(&root)["status"], "skipped");
    assert!(!needs_run(&root));
}

#[test]
fn a_project_never_checked_needs_a_check() {
    let root = project("unchecked");
    assert_eq!(report(&root)["status"], "unchecked");
    assert!(needs_run(&root));
}

#[test]
fn errors_are_recorded_and_go_stale_when_a_script_changes() {
    let root = project("failed");
    fake_tsc(
        &root,
        "public/js/a.js(1,7): error TS2300: Duplicate identifier 'Storage'.\n",
        2,
    );
    let result = run(&root);
    assert_eq!(result["status"], "failed", "{result}");
    assert_eq!(result["errorCount"], 1);
    assert_eq!(result["stale"], false);
    assert_eq!(
        result["tool"]
            .as_str()
            .unwrap()
            .starts_with("node_modules/.bin/tsc"),
        true
    );

    // Read back by another process, the way `status` and MCP read it.
    let recorded = report(&root);
    assert_eq!(recorded["errors"][0]["file"], "public/js/a.js");
    assert!(!needs_run(&root));

    std::thread::sleep(std::time::Duration::from_millis(20));
    fs::write(root.join("public/js/a.js"), "// @ts-check\n").unwrap();
    assert_eq!(report(&root)["stale"], true);
    assert!(needs_run(&root));
}

#[test]
fn a_clean_check_passes() {
    let root = project("passed");
    fake_tsc(&root, "", 0);
    let result = run(&root);
    assert_eq!(result["status"], "passed", "{result}");
    assert_eq!(result["errorCount"], 0);
}

#[test]
fn a_failure_tsc_cannot_place_is_still_an_error() {
    let root = project("crashed");
    fake_tsc(&root, "something went badly wrong\n", 1);
    let result = run(&root);
    assert_eq!(result["status"], "failed");
    assert_eq!(result["errors"][0]["file"], CONFIG_PATH);
    assert!(
        result["errors"][0]["message"]
            .as_str()
            .unwrap()
            .contains("something went badly wrong")
    );
}
