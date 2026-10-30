use super::expand;
use proc_macro2::TokenStream;
use quote::quote;

const FILE: &str = "src/jobs.rs";

fn expanded(attr: TokenStream, item: TokenStream) -> String {
    expand(attr, item, FILE)
        .map_err(|error| error.msg)
        .expect("a valid job")
        .to_string()
}

fn refused(attr: TokenStream, item: TokenStream, file: &str) -> String {
    match expand(attr, item, file) {
        Ok(_) => panic!("expected a refusal"),
        Err(error) => error.msg,
    }
}

#[test]
fn keeps_the_function_and_adds_a_typed_handle() {
    let out = expanded(
        TokenStream::new(),
        quote! { pub async fn send_receipt(order_id: i64) -> Result<(), JobError> { Ok(()) } },
    );

    assert!(out.contains("pub async fn send_receipt"), "{out}");
    assert!(out.contains("pub enum send_receipt { }"), "{out}");
    assert!(
        out.contains("pub async fn enqueue (order_id : i64)"),
        "{out}"
    );
    assert!(
        out.contains("pub async fn enqueue_unique (key : & str , order_id : i64)"),
        "{out}"
    );
    assert!(out.contains("pub fn request (order_id : i64)"), "{out}");
    assert!(out.contains("JobEntry"), "{out}");
    assert!(
        out.contains(r#"argument :: < i64 > (& __arguments , "send_receipt" , "order_id")"#),
        "{out}"
    );
}

#[test]
fn the_handle_takes_the_visibility_of_the_function() {
    let out = expanded(TokenStream::new(), quote! { pub(crate) async fn tidy() {} });
    assert!(out.contains("pub (crate) enum tidy { }"), "{out}");

    let out = expanded(TokenStream::new(), quote! { async fn tidy() {} });
    assert!(out.contains("enum tidy { }"), "{out}");
    assert!(!out.contains("pub enum tidy"), "{out}");
}

#[test]
fn options_reach_the_entry_and_a_name_replaces_the_stored_one() {
    let out = expanded(
        quote! { name = "search.reindex", attempts = 5, timeout_secs = 20, backoff_ms = 100, max_backoff_ms = 1_000 },
        quote! { pub async fn reindex() {} },
    );
    assert!(out.contains(r#"name : "search.reindex""#), "{out}");
    assert!(out.contains("Some (5u32)"), "{out}");
    assert!(out.contains("Some (20u64)"), "{out}");
    assert!(out.contains("Some (100u64)"), "{out}");
    assert!(out.contains("Some (1000u64)"), "{out}");
}

#[test]
fn a_mutable_parameter_is_read_and_forwarded_by_name() {
    let out = expanded(
        TokenStream::new(),
        quote! { pub async fn tidy(mut days: u32) { days += 1; let _ = days; } },
    );
    assert!(out.contains("tidy (days) . await"), "{out}");
    assert!(out.contains("pub fn payload (days : u32)"), "{out}");
}

#[test]
fn accepts_each_supported_return_shape() {
    for item in [
        quote! { pub async fn a() {} },
        quote! { pub async fn a() -> () {} },
        quote! { pub async fn a() -> Result<(), JobError> { Ok(()) } },
        quote! { pub async fn a() -> rahti::Result<()> { Ok(()) } },
    ] {
        expanded(TokenStream::new(), item);
    }
}

#[test]
fn refuses_a_function_a_worker_could_not_call() {
    let message = refused(TokenStream::new(), quote! { pub fn a() {} }, FILE);
    assert!(message.contains("not `async`"), "{message}");

    let message = refused(
        TokenStream::new(),
        quote! { pub async fn a(&self) {} },
        FILE,
    );
    assert!(message.contains("takes `self`"), "{message}");

    let message = refused(
        TokenStream::new(),
        quote! { pub async fn a<T>(value: T) {} },
        FILE,
    );
    assert!(message.contains("generic"), "{message}");

    let message = refused(
        TokenStream::new(),
        quote! { pub async fn a(name: &str) {} },
        FILE,
    );
    assert!(message.contains("`String` rather than `&str`"), "{message}");

    let message = refused(
        TokenStream::new(),
        quote! { pub async fn a() -> rahti::Result<String> { todo!() } },
        FILE,
    );
    assert!(message.contains("nobody to hand one"), "{message}");
}

#[test]
fn refuses_options_it_does_not_know() {
    for attr in [
        quote! { retries = 3 },
        quote! { attempts = 0 },
        quote! { name = "has space" },
        quote! { attempts },
    ] {
        let message = refused(attr, quote! { pub async fn a() {} }, FILE);
        assert!(message.starts_with("rahti:"), "{message}");
    }
}

#[test]
fn a_job_outside_the_jobs_module_is_refused_with_the_fix() {
    let message = refused(
        TokenStream::new(),
        quote! { pub async fn send_receipt() {} },
        "src/app/checkout/page.rs",
    );
    assert!(message.contains("src/jobs.rs"), "{message}");
    assert!(
        message.contains("cargo rahti generate job send_receipt"),
        "{message}"
    );
}

#[test]
fn the_jobs_module_is_recognised_on_every_platform() {
    for file in [
        "src/jobs.rs",
        "src/jobs/mail.rs",
        r"src\jobs\mail.rs",
        "tests/jobs/main.rs",
        "",
    ] {
        expand(TokenStream::new(), quote! { pub async fn a() {} }, file)
            .unwrap_or_else(|error| panic!("{file}: {}", error.msg));
    }
}
