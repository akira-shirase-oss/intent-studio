//! Untrusted bytes in, against the real binaries (I1 security cases): an uploaded SVG carrying
//! script is stored sanitized and served sandboxed, malformed files are refused by name, a
//! crashing worker fails only its own upload while the server keeps serving, and a reference on
//! a private address is refused — and recorded — without a connection to it. Run by
//! `scripts/drills.sh`, which builds `intent-worker` first.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::too_many_lines,
    reason = "test code: a failed unwrap is a failed test, and a journey reads top to bottom"
)]

mod support;

use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

use intent_provider_fixture::Mode;
use serde_json::{Value, json};
use support::{ProviderFixture, Server, TestDb, client, drill_env};

const PASSWORD: &str = "assets test password 4";
const DEMO_TENANT: &str = "00000000-0000-4000-8000-000000000001";
const LICENCE: &str = "Owned by Quillfeather Roofing, drawn for them in 2024";
const SOURCE: &str = "Sent by the owner, Robin Quillfeather, by e-mail";

fn worker() -> PathBuf {
    let path = PathBuf::from(support::server_binary())
        .parent()
        .unwrap()
        .join("intent-worker");
    assert!(
        path.is_file(),
        "no intent-worker beside intent-server at {}: build it first (scripts/drills.sh does)",
        path.display()
    );
    path
}

fn asset_fixture(name: &str) -> Vec<u8> {
    std::fs::read(support::repo_root().join("fixtures/assets").join(name)).unwrap()
}

fn hash() -> String {
    use std::io::Write as _;
    let mut child = std::process::Command::new(support::server_binary())
        .arg("hash-password")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(format!("{PASSWORD}\n").as_bytes())
        .unwrap();
    String::from_utf8(child.wait_with_output().unwrap().stdout)
        .unwrap()
        .trim()
        .to_owned()
}

async fn sign_in(base: &str) -> String {
    let response = client()
        .post(format!("{base}/v1/operator/session"))
        .json(&json!({ "password": PASSWORD }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    response.headers()["set-cookie"]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned()
}

/// A `multipart/form-data` body, written by hand.
/// One form field: its name, for a file its name and media type, and its bytes.
type Field<'a> = (&'a str, Option<(&'a str, &'a str)>, &'a [u8]);

fn multipart(fields: &[Field<'_>]) -> (String, Vec<u8>) {
    let boundary = "intent-studio-test-boundary-7d1f";
    let mut body = Vec::new();
    for (name, file, data) in fields {
        body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
        match file {
            Some((filename, media_type)) => body.extend_from_slice(
                format!(
                    "Content-Disposition: form-data; name=\"{name}\"; filename=\"{filename}\"\r\nContent-Type: {media_type}\r\n\r\n"
                )
                .as_bytes(),
            ),
            None => body.extend_from_slice(
                format!("Content-Disposition: form-data; name=\"{name}\"\r\n\r\n").as_bytes(),
            ),
        }
        body.extend_from_slice(data);
        body.extend_from_slice(b"\r\n");
    }
    body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    (format!("multipart/form-data; boundary={boundary}"), body)
}

async fn upload(
    base: &str,
    cookie: &str,
    file: (&str, &str, &[u8]),
    licence: &str,
    with_header: bool,
) -> (u16, Value) {
    let (content_type, body) = multipart(&[
        ("licence", None, licence.as_bytes()),
        ("source", None, SOURCE.as_bytes()),
        ("file", Some((file.0, file.1)), file.2),
    ]);
    let mut request = client()
        .post(format!("{base}/v1/operator/tenants/{DEMO_TENANT}/assets"))
        .header("cookie", cookie)
        .header("content-type", content_type)
        .body(body);
    if with_header {
        request = request.header("x-intent-upload", "1");
    }
    let response = request.send().await.unwrap();
    let status = response.status().as_u16();
    (status, response.json().await.unwrap_or(Value::Null))
}

fn png() -> Vec<u8> {
    let image = image::RgbaImage::from_pixel(1, 1, image::Rgba([31, 78, 121, 255]));
    let mut out = Vec::new();
    image
        .write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
        .unwrap();
    out
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs the stack: scripts/stack.sh up (run by scripts/drills.sh)"]
async fn uploads_are_sanitized_or_refused_and_references_stay_public() {
    let db = TestDb::create(1).await;
    let fixture = ProviderFixture::start(Mode::Ok).await;
    let store = std::env::temp_dir().join(format!(
        "intent-store-drill-{}",
        uuid::Uuid::now_v7().simple()
    ));
    let mut env = drill_env(&db, &fixture.url);
    env.push(("INTENT_OPERATOR_PASSWORD_HASH".into(), hash()));
    env.push(("INTENT_STORE_DIR".into(), store.display().to_string()));
    env.push(("INTENT_WORKER_BIN".into(), worker().display().to_string()));
    let server = Server::start(env.clone()).await;
    let base = server.base.clone();
    let cookie = sign_in(&base).await;

    // Nothing is read before the session check, and the upload header is required. A
    // small body proves the refusal is 401; a megabyte the server never consumes proves
    // it without relying on the response surviving a mid-write close — the close itself
    // is the refusal working.
    let anonymous = client()
        .post(format!("{base}/v1/operator/tenants/{DEMO_TENANT}/assets"))
        .header("content-type", "multipart/form-data; boundary=x")
        .header("x-intent-upload", "1")
        .body(b"x".to_vec())
        .send()
        .await
        .unwrap();
    assert_eq!(anonymous.status(), 401);
    let big = client()
        .post(format!("{base}/v1/operator/tenants/{DEMO_TENANT}/assets"))
        .header("content-type", "multipart/form-data; boundary=x")
        .header("x-intent-upload", "1")
        .body(vec![b'x'; 1_000_000])
        .send()
        .await;
    match big {
        Ok(response) => assert_eq!(response.status(), 401),
        // EPIPE/reset writing the unread body is the same refusal reaching us early.
        Err(error) => assert!(
            error.is_request(),
            "the anonymous upload failed for a reason that is not the refusal: {error}"
        ),
    }
    let (status, _) = upload(
        &base,
        &cookie,
        (
            "logo.svg",
            "image/svg+xml",
            &asset_fixture("clean-logo.svg"),
        ),
        LICENCE,
        false,
    )
    .await;
    assert_eq!(
        status, 415,
        "without the upload header a cross-site form could post"
    );

    // The hostile SVG is stored sanitized, its original kept beside it.
    let (status, svg) = upload(
        &base,
        &cookie,
        ("logo.svg", "image/svg+xml", &asset_fixture("hostile.svg")),
        LICENCE,
        true,
    )
    .await;
    assert_eq!(status, 201, "{svg}");
    assert_eq!(svg["media_type"], "image/svg+xml");
    assert_eq!(svg["removed_parts"], 17);
    assert_eq!(svg["licence"], LICENCE);
    assert_eq!(svg["original"]["name"], "logo.svg");
    assert_ne!(svg["sha256"], svg["original"]["sha256"]);
    let asset_id = svg["id"].as_str().unwrap().to_owned();
    let content = client()
        .get(format!(
            "{base}/v1/operator/tenants/{DEMO_TENANT}/assets/{asset_id}/content"
        ))
        .header("cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(content.status(), 200);
    assert_eq!(content.headers()["content-type"], "image/svg+xml");
    let csp = content.headers()["content-security-policy"]
        .to_str()
        .unwrap()
        .to_owned();
    assert!(
        csp.contains("sandbox") && csp.contains("default-src 'none'"),
        "{csp}"
    );
    assert_eq!(content.headers()["x-content-type-options"], "nosniff");
    let body_served = content.text().await.unwrap().to_ascii_lowercase();
    for banned in [
        "<script",
        "onload",
        "javascript",
        "evil.example",
        "foreignobject",
    ] {
        assert!(!body_served.contains(banned), "{banned} was served");
    }

    // A PNG is decoded and re-encoded.
    let (status, image) = upload(
        &base,
        &cookie,
        ("dot.png", "image/png", &png()),
        LICENCE,
        true,
    )
    .await;
    assert_eq!(status, 201, "{image}");
    assert_eq!(
        (image["width"].clone(), image["height"].clone()),
        (json!(1), json!(1))
    );

    // Refusals, each named.
    for (file, reason) in [
        (
            ("not-a-png.png", "image/png", asset_fixture("not-a-png.png")),
            "type_mismatch",
        ),
        (
            ("photo.webp", "image/webp", b"RIFF\0\0\0\0WEBP".to_vec()),
            "unsupported_type",
        ),
        (
            (
                "bomb.svg",
                "image/svg+xml",
                asset_fixture("entity-bomb.svg"),
            ),
            "malformed",
        ),
        (("half.png", "image/png", png()[..30].to_vec()), "malformed"),
    ] {
        let (status, body) = upload(&base, &cookie, (file.0, file.1, &file.2), LICENCE, true).await;
        assert_eq!(status, 422, "{body}");
        assert_eq!(body["error"]["code"], "asset_refused");
        assert_eq!(body["error"]["problems"][0]["code"], reason, "{}", file.0);
    }
    let (status, body) = upload(
        &base,
        &cookie,
        (
            "logo.svg",
            "image/svg+xml",
            &asset_fixture("clean-logo.svg"),
        ),
        "",
        true,
    )
    .await;
    assert_eq!(status, 422);
    assert_eq!(body["error"]["problems"][0]["path"], "licence");

    let assets = client()
        .get(format!("{base}/v1/operator/tenants/{DEMO_TENANT}/assets"))
        .header("cookie", &cookie)
        .send()
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap();
    assert_eq!(
        assets["assets"].as_array().unwrap().len(),
        2,
        "refused uploads stored nothing"
    );

    // A brief may use the asset as its logo; another tenant may not.
    let brief = |logo: &str, references: Value| {
        json!({
            "business_name": "Quillfeather Roofing", "goal": "quote_requests",
            "intent": "More roof-repair enquiries", "audience": "Home owners",
            "services": [{ "name": "Roof repair" }],
            "brand": { "logo": logo },
            "references": references,
        })
    };
    // A listener on loopback that counts connections: the guard must never open one to it.
    let listener = std::net::TcpListener::bind(("127.0.0.1", support::free_port())).unwrap();
    let local = listener.local_addr().unwrap();
    listener.set_nonblocking(true).unwrap();
    let connections = Arc::new(AtomicUsize::new(0));
    let counter = connections.clone();
    let watcher = std::thread::spawn(move || {
        for _ in 0..100 {
            if listener.accept().is_ok() {
                counter.fetch_add(1, Ordering::SeqCst);
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    });
    let references = json!([
        { "url": format!("http://{local}/admin"), "permission": "inspiration_only" },
        { "url": "http://169.254.169.254/latest/meta-data/", "permission": "inspiration_only" },
        { "url": "http://localhost/", "permission": "own_content" },
    ]);
    let created = client()
        .post(format!("{base}/v1/operator/tenants/{DEMO_TENANT}/briefs"))
        .header("cookie", &cookie)
        .json(&brief(&asset_id, references.clone()))
        .send()
        .await
        .unwrap();
    assert_eq!(created.status(), 201);
    let brief_id = created.json::<Value>().await.unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let other = client()
        .post(format!("{base}/v1/operator/tenants"))
        .header("cookie", &cookie)
        .json(&json!({ "name": "Someone else" }))
        .send()
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap();
    let foreign = client()
        .post(format!(
            "{base}/v1/operator/tenants/{}/briefs",
            other["id"].as_str().unwrap()
        ))
        .header("cookie", &cookie)
        .json(&brief(&asset_id, json!([])))
        .send()
        .await
        .unwrap();
    assert_eq!(foreign.status(), 422);
    assert_eq!(
        foreign.json::<Value>().await.unwrap()["error"]["problems"][0]["code"],
        "asset_unknown"
    );

    // Every reference on a private target is refused by the guard, named, and recorded.
    for (index, port_reason) in [
        (0, "address_forbidden"),
        (1, "address_forbidden"),
        (2, "address_forbidden"),
    ] {
        let response = client()
            .post(format!(
                "{base}/v1/operator/tenants/{DEMO_TENANT}/briefs/{brief_id}/snapshots"
            ))
            .header("cookie", &cookie)
            .json(&json!({ "reference": index }))
            .send()
            .await
            .unwrap();
        let status = response.status().as_u16();
        let body: Value = response.json().await.unwrap();
        // The first reference names a non-standard port: refused for the port before the
        // address is even looked at.
        let expected = if index == 0 {
            "port_not_allowed"
        } else {
            port_reason
        };
        assert_eq!(
            (status, body["error"]["code"].clone()),
            (422, json!("fetch_refused")),
            "{body}"
        );
        assert_eq!(body["error"]["problems"][0]["code"], expected);
    }
    let snapshots = client()
        .get(format!(
            "{base}/v1/operator/tenants/{DEMO_TENANT}/briefs/{brief_id}/snapshots"
        ))
        .header("cookie", &cookie)
        .send()
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap();
    let rows = snapshots["snapshots"].as_array().unwrap();
    assert_eq!(rows.len(), 3);
    assert!(
        rows.iter()
            .all(|r| r["outcome"] == "refused" && r["sha256"].is_null())
    );
    watcher.join().unwrap();
    assert_eq!(
        connections.load(Ordering::SeqCst),
        0,
        "the guard connected to a refused address"
    );

    // A worker that crashes fails only its own upload: the server keeps answering.
    // The second server is a separate deployment: its own store root, since a
    // live store's lock (D-061) refuses a second opener on the same root.
    let crash_store = std::env::temp_dir().join(format!(
        "intent-store-drill-{}",
        uuid::Uuid::now_v7().simple()
    ));
    std::fs::create_dir_all(&crash_store).unwrap();
    let crash = crash_store.join("crash-worker.sh");
    std::fs::write(&crash, "#!/bin/sh\nkill -SEGV $$\n").unwrap();
    std::process::Command::new("chmod")
        .arg("+x")
        .arg(&crash)
        .status()
        .unwrap();
    let mut crash_env = env.clone();
    crash_env.retain(|(k, _)| k != "INTENT_WORKER_BIN" && k != "INTENT_STORE_DIR");
    crash_env.push(("INTENT_WORKER_BIN".into(), crash.display().to_string()));
    crash_env.push(("INTENT_STORE_DIR".into(), crash_store.display().to_string()));
    let crashing = Server::start(crash_env).await;
    let crash_cookie = sign_in(&crashing.base).await;
    let (status, body) = upload(
        &crashing.base,
        &crash_cookie,
        (
            "logo.svg",
            "image/svg+xml",
            &asset_fixture("clean-logo.svg"),
        ),
        LICENCE,
        true,
    )
    .await;
    assert_eq!(
        (status, body["error"]["problems"][0]["code"].clone()),
        (422, json!("worker_crashed")),
        "{body}"
    );
    let ready = client()
        .get(format!("{}/readyz", crashing.base))
        .send()
        .await
        .unwrap();
    assert_eq!(ready.status(), 200);
    let lead = client()
        .post(format!("{}/v1/sites/00000000-0000-4000-9000-000000000000/leads", crashing.base))
        .json(&json!({
            "submission_id": "after-crash-0001", "name": "After Crash", "email": "after.crash@example.test",
            "consent": true, "consent_notice": "privacy-v1"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(
        lead.status(),
        201,
        "intake still works after a worker crash"
    );

    let needles = [LICENCE, SOURCE, "evil.example", "After Crash"];
    support::assert_no_leaks(&server.log(), &needles);
    support::assert_no_leaks(&crashing.log(), &needles);
    drop(crashing);
    drop(server);
    let _ = std::fs::remove_dir_all(&crash_store);
    let _ = std::fs::remove_dir_all(&store);
    db.drop_database().await;
}
